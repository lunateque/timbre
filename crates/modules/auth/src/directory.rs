//! What other modules may read about accounts: the [`Directory`] port.

use crate::config::shared_db;
use crate::{Auth, devices, log_events::INVALID_STORED_NAME, store};
use timbre_edge_sqlite::rusqlite::Connection;
use timbre_kernel::clock::from_unix_millis;
use timbre_kernel::directory::{Directory, DirectoryDevice, DirectoryUser};
use timbre_kernel::identity::UserName;
use timbre_kernel::{Ctx, Fallible, Page, PageRequest, log, page::Window};

impl Directory for Auth {
    fn users(&self, ctx: &Ctx, page: PageRequest<'_>) -> Fallible<Page<DirectoryUser>> {
        let window = Window::new(page)?;
        read(ctx, |sql| {
            let Page { items, next } = window.page(store::users_page(sql, window)?);
            let mut users = Vec::with_capacity(items.len());
            for row in items {
                if let Some(user) = self.shown(ctx, sql, &row)? {
                    users.push(user);
                }
            }
            Ok(Page { items: users, next })
        })
    }

    fn user(&self, ctx: &Ctx, name: &UserName) -> Fallible<Option<DirectoryUser>> {
        read(ctx, |sql| match store::user_by_name(sql, name)? {
            Some(row) => self.shown(ctx, sql, &row),
            None => Ok(None),
        })
    }

    fn devices(
        &self,
        ctx: &Ctx,
        user: &UserName,
        page: PageRequest<'_>,
    ) -> Fallible<Page<DirectoryDevice>> {
        let window = Window::new(page)?;
        read(ctx, |sql| {
            let owner = store::require_user(sql, user)?;
            let Page { items, next } = window.page(devices::of_user(sql, owner.id, window)?);
            Ok(Page {
                items: items.into_iter().map(listed).collect(),
                next,
            })
        })
    }

    fn owner_of_device(&self, ctx: &Ctx, device: &str) -> Fallible<Option<UserName>> {
        read(ctx, |sql| {
            let Some(row) = devices::by_id(sql, device)? else {
                return Ok(None);
            };
            Ok(self.stored_name(ctx, &row.user))
        })
    }
}

fn read<T>(ctx: &Ctx, work: impl FnOnce(&Connection) -> Fallible<T>) -> Fallible<T> {
    let shared = shared_db(ctx)?;
    shared.get(ctx)?.read(work)
}

impl Auth {
    fn stored_name(&self, ctx: &Ctx, text: &str) -> Option<UserName> {
        if let Ok(name) = UserName::normalize(text)
            && name.as_str() == text
        {
            return Some(name);
        }
        let first = {
            let mut seen = self
                .bad_users
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            !seen.contains(text) && seen.insert(text.to_owned())
        };
        if first {
            log!(ctx, warn, INVALID_STORED_NAME, user = text);
        }
        None
    }

    fn shown(
        &self,
        ctx: &Ctx,
        sql: &Connection,
        row: &store::UserRow,
    ) -> Fallible<Option<DirectoryUser>> {
        let Some(name) = self.stored_name(ctx, &row.name) else {
            return Ok(None);
        };
        let stored = store::permissions_of(sql, row.id)?;
        Ok(Some(DirectoryUser {
            name,
            admin: row.admin,
            permissions: crate::effective(ctx, row.admin, &stored),
            created: from_unix_millis(row.created),
        }))
    }
}

/// A stored device as the port shows it: never the digest, never a secret.
fn listed(row: devices::DeviceRow) -> DirectoryDevice {
    DirectoryDevice {
        id: row.id,
        label: row.label,
        created: from_unix_millis(row.created),
        last_used: row.last_used.map(from_unix_millis),
        last_client: row.last_client,
        expires: row.expires.map(from_unix_millis),
    }
}
