//! `timbre audit`: reading the journal back.

use crate::config::shared_db;
use crate::log_events::INVALID_ARGUMENT;
use crate::store::{self, Query};
use timbre_kernel::identity::UserName;
use timbre_kernel::{BoxFuture, Ctx, Fallible, clock, failures::KernelFailure, log};

/// How many entries are reported when the command does not say.
const DEFAULT_LIMIT: usize = 50;
/// The most that can be asked for in one go.
const MAX_LIMIT: usize = 10_000;

pub(crate) fn audit(ctx: Ctx, args: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let worker = ctx.clone();
        ctx.blocking(move || report(&worker, &args)).await?
    })
}

fn report(ctx: &Ctx, args: &[String]) -> Fallible {
    let query = parse(ctx, args)?;
    // Queued entries are written first, so reading the journal right after
    // a command shows what that command did.
    crate::jobs::drain(ctx)?;
    let shared = shared_db(ctx)?;
    shared.get(ctx)?.read(|sql| {
        for record in store::read(sql, &query)? {
            ctx.say(format_args!(
                "{}  kind={}  user={:?}  client={:?}  count={}  detail={}",
                clock::iso(clock::from_unix_millis(record.time)),
                record.kind,
                record.user.as_deref().unwrap_or_default(),
                record.client.as_deref().unwrap_or_default(),
                record.count,
                record.detail,
            ))?;
        }
        Ok(())
    })
}

fn parse(ctx: &Ctx, args: &[String]) -> Fallible<Query> {
    let mut query = Query {
        limit: DEFAULT_LIMIT,
        ..Query::default()
    };
    let mut rest = args;
    while let [flag, value, tail @ ..] = rest {
        match flag.as_str() {
            "--since" => {
                let day = clock::parse_date(value)
                    .ok_or_else(|| invalid(ctx, flag, value, "Expected a date as YYYY-MM-DD."))?;
                query.since = Some(clock::unix_millis(day));
            }
            "--user" => {
                let name = UserName::normalize(value)
                    .map_err(|_| invalid(ctx, flag, value, "Expected a usable user name."))?;
                query.user = Some(name.as_str().to_owned());
            }
            "--kind" => query.kind = Some(value.clone()),
            "--limit" => {
                query.limit = match value.parse::<usize>() {
                    Ok(limit) if limit > 0 && limit <= MAX_LIMIT => limit,
                    _ => {
                        return Err(invalid(
                            ctx,
                            flag,
                            value,
                            "Expected --limit to be 1 to 10000.",
                        ));
                    }
                };
            }
            _ => return Err(invalid(ctx, flag, value, USAGE)),
        }
        rest = tail;
    }
    if let [flag] = rest {
        return Err(invalid(ctx, flag, "", USAGE));
    }
    Ok(query)
}

const USAGE: &str =
    "Usage: timbre audit [--since <date>] [--user <name>] [--kind <kind>] [--limit <n>]";

fn invalid(
    ctx: &Ctx,
    flag: &str,
    value: &str,
    message: &'static str,
) -> Box<dyn timbre_kernel::Failure> {
    log!(ctx, warn, INVALID_ARGUMENT, flag = flag, value = value);
    Box::new(KernelFailure::Usage(message))
}
