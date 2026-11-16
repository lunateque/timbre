//! The `timbre db` commands: check, backup, prune.

use crate::log_events::{DB_BACKUP, DB_CHECK, DB_PRUNE};
use std::path::Path;
use std::process::ExitCode;
use timbre_kernel::{Ctx, Fallible, Kernel, log};

/// Dispatch `db <check|backup|prune>` on the database the caller already opened
/// and migrated, so these commands never open the file a second time.
pub(crate) fn run(
    ctx: &Ctx,
    kernel: &std::sync::Arc<Kernel>,
    database: &timbre_edge_sqlite::Database,
    args: &[String],
) -> Fallible<ExitCode> {
    let second = args.get(1).map_or("", String::as_str);
    match second {
        "check" => match database.check() {
            Ok(()) => {
                log!(ctx, info, DB_CHECK, ok = true);
                Ok(ExitCode::SUCCESS)
            }
            Err(failure) => {
                log!(
                    ctx,
                    error,
                    DB_CHECK,
                    ok = false,
                    code = failure.code().as_str()
                );
                Ok(ExitCode::FAILURE)
            }
        },
        "backup" => {
            let Some(target) = args.get(2) else {
                log!(
                    ctx,
                    error,
                    DB_BACKUP,
                    reason = "usage: timbre db backup <file>"
                );
                return Ok(ExitCode::from(2));
            };
            let path = Path::new(target);
            database.backup(path)?;
            log!(ctx, info, DB_BACKUP, to = path.display().to_string());
            Ok(ExitCode::SUCCESS)
        }
        "prune" => {
            let registered: Vec<&str> = kernel
                .migrations()
                .iter()
                .map(timbre_kernel::Migration::module)
                .collect();
            let unowned = database.owned_tables(&registered)?;
            let confirmed = args.get(2).is_some_and(|flag| flag == "--yes");
            if !confirmed {
                log!(
                    ctx,
                    info,
                    DB_PRUNE,
                    tables = unowned.join(",").as_str(),
                    dropped = false
                );
                return Ok(ExitCode::SUCCESS);
            }
            database.drop_tables(&unowned)?;
            log!(
                ctx,
                info,
                DB_PRUNE,
                tables = unowned.join(",").as_str(),
                dropped = true
            );
            Ok(ExitCode::SUCCESS)
        }
        other => {
            log!(
                ctx,
                error,
                DB_CHECK,
                reason = format!("unknown db command `{other}`").as_str()
            );
            Ok(ExitCode::from(2))
        }
    }
}
