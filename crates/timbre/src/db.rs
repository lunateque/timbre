//! The `timbre db` commands: check, backup, prune. The command table calls
//! these through `commands.rs`; every one runs on the database the process
//! opened and migrated once, so a `db` command never opens the file again.

use crate::failures::TimbreFailure;
use crate::log_events::{DB_BACKUP, DB_CHECK, DB_PRUNE, USAGE};
use std::path::Path;
use timbre_kernel::{Ctx, Failure, Fallible, Kernel, log};

/// A bad word -- an unknown verb, a missing target, an existing destination
/// without `--force` -- is a usage error: one `timbre.usage` line, exit 2,
/// never a `db_check`/`db_backup` line and never `timbre.command_failed`.
fn usage(ctx: &Ctx, reason: &str) -> Fallible {
    log!(ctx, error, USAGE, reason = reason);
    Err(Box::new(TimbreFailure::Usage) as Box<dyn Failure>)
}

/// The verb under `db` that the command table does not know: the bare `db`
/// entry receives it.
pub(crate) fn unknown_verb(ctx: &Ctx, verb: &str) -> Fallible {
    if verb.is_empty() {
        return usage(ctx, "usage: timbre db <check|backup|prune>");
    }
    usage(
        ctx,
        format!("unknown db command `{verb}`; try check, backup or prune").as_str(),
    )
}

/// `timbre db check`: the integrity check, reported under `timbre.db_check`.
/// A check that ran and found corruption exits 1 like any failed command; a
/// bad word after `check` is a usage error.
pub(crate) fn check(
    ctx: &Ctx,
    database: &timbre_edge_sqlite::Database,
    rest: &[String],
) -> Fallible {
    if let Some(word) = rest.first() {
        return usage(
            ctx,
            format!("`db check` takes no argument, not `{word}`").as_str(),
        );
    }
    match database.check() {
        Ok(()) => {
            log!(ctx, info, DB_CHECK, ok = true);
            Ok(())
        }
        Err(failure) => {
            log!(
                ctx,
                error,
                DB_CHECK,
                ok = false,
                code = failure.code().as_str()
            );
            // The check ran and reported; `timbre.db_check` is the line and
            // the exit is 1 without a second `command_failed`.
            Err(Box::new(TimbreFailure::Reported) as Box<dyn Failure>)
        }
    }
}

/// `timbre db backup <file> [--force]`: `--force` is accepted in any
/// position; a destination that already exists without it, or a missing
/// target, is a usage error before anything is written.
pub(crate) fn backup(
    ctx: &Ctx,
    database: &timbre_edge_sqlite::Database,
    rest: &[String],
) -> Fallible {
    if has(rest, "--yes") {
        return usage(ctx, "`db backup` takes `--force`, not `--yes`");
    }
    let force = has(rest, "--force");
    // The target is the one word that is not a flag, in any position.
    let Some(target) = rest.iter().find(|word| !word.starts_with("--")) else {
        return usage(ctx, "usage: timbre db backup <file> [--force]");
    };
    let path = Path::new(target);
    // An existing destination without `--force` is a usage mistake, not a
    // failed copy (the edge refuses one too, for direct callers).
    if !force && std::fs::symlink_metadata(path).is_ok() {
        return usage(
            ctx,
            format!("{} exists; pass --force to replace it", path.display()).as_str(),
        );
    }
    database.backup(path, force)?;
    log!(ctx, info, DB_BACKUP, to = path.display().to_string());
    Ok(())
}

/// `timbre db prune [--yes]`: `--yes` is accepted in any position; anything
/// else under `prune` is a usage error.
pub(crate) fn prune(
    ctx: &Ctx,
    kernel: &Kernel,
    database: &timbre_edge_sqlite::Database,
    rest: &[String],
) -> Fallible {
    if has(rest, "--force") {
        return usage(ctx, "`db prune` takes `--yes`, not `--force`");
    }
    let confirmed = has(rest, "--yes");
    let positional = rest.iter().filter(|word| !word.starts_with("--")).count();
    if positional > 0 {
        return usage(ctx, "usage: timbre db prune [--yes]");
    }
    let registered: Vec<&str> = kernel
        .migrations()
        .iter()
        .map(timbre_kernel::Migration::module)
        .collect();
    let unowned = database.owned_tables(&registered)?;
    if !confirmed {
        log!(
            ctx,
            info,
            DB_PRUNE,
            tables = unowned.join(",").as_str(),
            dropped = false
        );
        return Ok(());
    }
    database.drop_tables(&unowned)?;
    log!(
        ctx,
        info,
        DB_PRUNE,
        tables = unowned.join(",").as_str(),
        dropped = true
    );
    Ok(())
}

/// Whether any word is exactly `flag`: `--force` and `--yes` are accepted in
/// any position, so no typo silently changes what runs.
fn has(words: &[String], flag: &str) -> bool {
    words.iter().any(|word| word == flag)
}
