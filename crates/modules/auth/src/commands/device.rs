//! `timbre device ...`.
//!
//! A new secret leaves the process once, on stdout. Inside the process it
//! travels in `DeviceIssued`, never in the log or journal. Only its digest
//! is stored.

use crate::commands::{argument, lookup};
use crate::config::{MAX_EXPIRY_DAYS, shared_db};
use crate::log_events::{DEVICE_ISSUED, DEVICE_REISSUED, DEVICE_REVOKED, INVALID_ARGUMENT};
use crate::{devices, keys, store};
use timbre_kernel::clock::{DAY_MILLIS, now_unix_millis};
use timbre_kernel::page::Window;
use timbre_kernel::signals::{DeviceIssued, DeviceRevoked};
use timbre_kernel::{BoxFuture, Ctx, Fallible, clock, log};

pub(crate) fn issue(ctx: Ctx, args: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let worker = ctx.clone();
        ctx.blocking(move || issued(&worker, &args)).await?
    })
}

fn issued(ctx: &Ctx, args: &[String]) -> Fallible {
    let (user, label, days) = match args {
        [user, label] => (user, label, None),
        [user, label, flag, days] if flag == "--expires" => (user, label, Some(days.as_str())),
        _ => return argument("device issue <user> <label> [--expires <days>]"),
    };
    let user = crate::user_name(ctx, user)?;
    if !valid_label(label) {
        log!(ctx, warn, INVALID_ARGUMENT, label = label.as_str());
        return argument(
            "Expected a device label with 1 to 64 characters and no control or format characters.",
        );
    }
    let days = match days {
        Some(text) => Some(expiry_days(ctx, text)?),
        None => crate::default_expiry(ctx)?,
    };
    let created = now_unix_millis();
    let expires = days.map(|days| created.saturating_add(i64::from(days) * DAY_MILLIS));
    let id = keys::device_id()?;
    let key = keys::issue()?;
    let shared = shared_db(ctx)?;
    shared.get(ctx)?.write(|sql| {
        let owner = lookup(ctx, user.as_str(), store::require_user(sql, &user))?;
        devices::insert(
            sql,
            devices::NewDevice {
                id: &id,
                user: owner.id,
                label,
                prefix: &key.prefix,
                digest: &key.digest,
                created,
                expires,
            },
        )
    })?;
    let signal = DeviceIssued {
        device: id,
        label: label.clone(),
        user,
        secret: key.secret,
    };
    ctx.publish(&signal);
    log!(
        ctx,
        info,
        DEVICE_ISSUED,
        id = signal.device.as_str(),
        user = signal.user.as_str(),
        label = signal.label.as_str(),
        expires = expires
            .map_or_else(String::new, |at| clock::iso(clock::from_unix_millis(at)))
            .as_str(),
    );
    ctx.say(format_args!("{}", signal.secret.expose()))
}

pub(crate) fn reissue(ctx: Ctx, args: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let worker = ctx.clone();
        ctx.blocking(move || reissued(&worker, &args)).await?
    })
}

fn reissued(ctx: &Ctx, args: &[String]) -> Fallible {
    let [id] = args else {
        return argument("device reissue <id>");
    };
    let key = keys::issue()?;
    let shared = shared_db(ctx)?;
    let (device, user) = shared.get(ctx)?.write(|sql| {
        let device = lookup(ctx, id, devices::require(sql, id))?;
        let user = crate::stored_user(&device.user)?;
        devices::rotate(sql, id, &key.prefix, &key.digest)?;
        Ok((device, user))
    })?;
    // The device keeps its id, so a consumer that stored the old secret
    // learns about the new one from the same signal as a first issue.
    let signal = DeviceIssued {
        device: device.id,
        label: device.label,
        user,
        secret: key.secret,
    };
    ctx.publish(&signal);
    log!(
        ctx,
        info,
        DEVICE_REISSUED,
        id = signal.device.as_str(),
        user = signal.user.as_str(),
        label = signal.label.as_str(),
    );
    ctx.say(format_args!("{}", signal.secret.expose()))
}

pub(crate) fn list(ctx: Ctx, args: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let worker = ctx.clone();
        ctx.blocking(move || listed(&worker, &args)).await?
    })
}

fn listed(ctx: &Ctx, args: &[String]) -> Fallible {
    let user = match args {
        [] => None,
        [user] => Some(crate::user_name(ctx, user)?),
        _ => return argument("device list [<user>]"),
    };
    let shared = shared_db(ctx)?;
    // One snapshot across every page, so a device issued mid-list cannot
    // shift the rows a later page returns.
    shared.get(ctx)?.read_transaction(|sql| {
        let owner = user.as_ref().map(|name| lookup(ctx, name.as_str(), store::require_user(sql, name))).transpose()?;
        let mut page = Window::first();
        loop {
            let rows = match &owner {
                Some(owner) => devices::of_user(sql, owner.id, page)?,
                None => devices::every(sql, page)?,
            };
            let more = page.advance(rows.len());
            for row in rows.into_iter().take(page.count()) {
                ctx.say(format_args!(
                    "{:?}  user={:?}  label={:?}  created={}  last_used={}  last_client={:?}  expires={}",
                    row.id, row.user, row.label, clock::iso(clock::from_unix_millis(row.created)),
                    stamp(row.last_used), row.last_client.as_deref().unwrap_or_default(), stamp(row.expires)
                ))?;
            }
            if !more {
                break;
            }
        }
        Ok(())
    })
}

pub(crate) fn revoke(ctx: Ctx, args: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let worker = ctx.clone();
        ctx.blocking(move || revoked(&worker, &args)).await?
    })
}

fn revoked(ctx: &Ctx, args: &[String]) -> Fallible {
    let [id] = args else {
        return argument("device revoke <id>");
    };
    let shared = shared_db(ctx)?;
    let (device, user) = shared.get(ctx)?.write(|sql| {
        let device = lookup(ctx, id, devices::require(sql, id))?;
        let user = crate::stored_user(&device.user)?;
        devices::delete(sql, id)?;
        Ok((device, user))
    })?;
    ctx.publish(&DeviceRevoked {
        device: device.id.clone(),
        user,
    });
    log!(
        ctx,
        info,
        DEVICE_REVOKED,
        id = device.id.as_str(),
        user = device.user.as_str(),
        label = device.label.as_str(),
    );
    Ok(())
}

fn stamp(at: Option<i64>) -> String {
    at.map_or_else(String::new, |at| clock::iso(clock::from_unix_millis(at)))
}

fn expiry_days(ctx: &Ctx, text: &str) -> Fallible<u32> {
    if text.bytes().all(|byte| byte.is_ascii_digit())
        && let Ok(days) = text.parse::<u32>()
        && (1..=MAX_EXPIRY_DAYS).contains(&days)
    {
        return Ok(days);
    }
    log!(ctx, warn, INVALID_ARGUMENT, expires = text);
    argument("Expected --expires to be a whole number of days (1 to 36500).")
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.chars().count() <= 64
        && !label
            .chars()
            .any(|ch| ch.is_control() || timbre_kernel::text::is_format_char(ch))
}
