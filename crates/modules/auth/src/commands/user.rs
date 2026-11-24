//! `timbre user ...`.

use crate::commands::{argument, lookup};
use crate::config::shared_db;
use crate::log_events::{PERMISSION_GRANTED, PERMISSION_REVOKED, USER_ADDED, USER_REMOVED};
use crate::{devices, store};
use timbre_kernel::clock::now_unix_millis;
use timbre_kernel::page::Window;
use timbre_kernel::signals::{DeviceRevoked, PermissionsChanged, UserAdded, UserRemoved};
use timbre_kernel::{BoxFuture, Ctx, Fallible, log};

pub(crate) fn add(ctx: Ctx, args: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let worker = ctx.clone();
        ctx.blocking(move || added(&worker, &args)).await?
    })
}

fn added(ctx: &Ctx, args: &[String]) -> Fallible {
    let (name, admin) = match args {
        [name] => (name, false),
        [name, flag] if flag == "--admin" => (name, true),
        _ => return argument("user add <name> [--admin]"),
    };
    let name = crate::user_name(ctx, name)?;
    let shared = shared_db(ctx)?;
    shared.get(ctx)?.write(|sql| {
        lookup(
            ctx,
            name.as_str(),
            store::insert_user(sql, &name, admin, now_unix_millis()),
        )?;
        Ok(())
    })?;
    ctx.publish(&UserAdded { user: name.clone() });
    if admin {
        ctx.publish(&PermissionsChanged {
            user: name.clone(),
            granted: Vec::new(),
            revoked: Vec::new(),
            admin: true,
        });
    }
    log!(ctx, info, USER_ADDED, name = name.as_str(), admin = admin);
    Ok(())
}

pub(crate) fn list(ctx: Ctx, args: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let worker = ctx.clone();
        ctx.blocking(move || listed(&worker, &args)).await?
    })
}

fn listed(ctx: &Ctx, args: &[String]) -> Fallible {
    if !args.is_empty() {
        return argument("user list");
    }
    let shared = shared_db(ctx)?;
    // One snapshot across every page: a list must not mix a user added
    // between two pages with one it already showed.
    shared.get(ctx)?.read_transaction(|sql| {
        let mut page = Window::first();
        loop {
            let rows = store::users_page(sql, page)?;
            let more = page.advance(rows.len());
            for row in rows.into_iter().take(page.count()) {
                let stored = store::permissions_of(sql, row.id)?;
                let held = crate::effective(ctx, row.admin, &stored);
                let names: Vec<_> = held.iter().map(|held| held.as_str()).collect();
                let devices = devices::count_for_user(sql, row.id)?;
                ctx.say(format_args!(
                    "{:?}  admin={}  devices={devices}  permissions={}",
                    row.name,
                    row.admin,
                    names.join(",")
                ))?;
            }
            if !more {
                break;
            }
        }
        Ok(())
    })
}

pub(crate) fn remove(ctx: Ctx, args: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let worker = ctx.clone();
        ctx.blocking(move || removed(&worker, &args)).await?
    })
}

fn removed(ctx: &Ctx, args: &[String]) -> Fallible {
    let [name] = args else {
        return argument("user remove <name>");
    };
    let name = crate::user_name(ctx, name)?;
    let shared = shared_db(ctx)?;
    let gone = shared.get(ctx)?.write(|sql| {
        let user = lookup(ctx, name.as_str(), store::require_user(sql, &name))?;
        let gone = devices::ids_of_user(sql, user.id)?;
        store::delete_user(sql, user.id)?;
        Ok(gone)
    })?;
    // The devices are revoked by the same delete, so whoever keeps copies of
    // their secrets hears about each one before the user itself disappears.
    for device in &gone {
        ctx.publish(&DeviceRevoked {
            device: device.clone(),
            user: name.clone(),
        });
    }
    ctx.publish(&UserRemoved { user: name.clone() });
    log!(
        ctx,
        info,
        USER_REMOVED,
        name = name.as_str(),
        devices = gone.len()
    );
    Ok(())
}

pub(crate) fn grant(ctx: Ctx, args: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let worker = ctx.clone();
        ctx.blocking(move || change(&worker, &args, true)).await?
    })
}

pub(crate) fn revoke(ctx: Ctx, args: Vec<String>) -> BoxFuture<Fallible> {
    Box::pin(async move {
        let worker = ctx.clone();
        ctx.blocking(move || change(&worker, &args, false)).await?
    })
}

fn change(ctx: &Ctx, args: &[String], granting: bool) -> Fallible {
    let [name, permission] = args else {
        return argument(if granting {
            "user grant <name> <permission>"
        } else {
            "user revoke <name> <permission>"
        });
    };
    let name = crate::user_name(ctx, name)?;
    let grant = granting
        .then(|| lookup(ctx, permission, crate::declared(ctx, permission)))
        .transpose()?;
    let shared = shared_db(ctx)?;
    let (admin, changed) = shared.get(ctx)?.write(|sql| {
        let user = lookup(ctx, name.as_str(), store::require_user(sql, &name))?;
        let changed = if let Some(grant) = grant {
            store::grant(sql, user.id, grant)?
        } else {
            store::revoke(sql, user.id, permission)?
        };
        Ok((user.admin, changed))
    })?;
    if changed {
        let (granted, revoked) = match grant {
            Some(grant) => (vec![grant], Vec::new()),
            None => (Vec::new(), vec![permission.clone()]),
        };
        ctx.publish(&PermissionsChanged {
            user: name.clone(),
            granted,
            revoked,
            admin,
        });
    }
    let event = if granting {
        PERMISSION_GRANTED
    } else {
        PERMISSION_REVOKED
    };
    log!(
        ctx,
        info,
        event,
        name = name.as_str(),
        permission = permission.as_str(),
        changed = changed
    );
    Ok(())
}
