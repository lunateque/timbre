//! The directory of users and their devices: what another module may read
//! about accounts without knowing how they are stored or verified.
//!
//! Secrets and digests are not part of this contract and never will be: the
//! module that owns users keeps them, and everything else only ever learns
//! who exists, what they may do, and which devices they issued.

use crate::identity::{Permission, UserName};
use crate::{Ctx, Fallible, Page, PageRequest, Port};
use std::time::SystemTime;

/// A user as the directory shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryUser {
    /// The canonical account name.
    pub name: UserName,
    /// Whether the account holds every permission declared at check time.
    pub admin: bool,
    /// The permissions in effect, already including an admin's full set, in
    /// declaration order for an admin and alphabetical otherwise.
    pub permissions: Vec<Permission>,
    /// When the account was created.
    pub created: SystemTime,
}

/// One device of a user; never its secret, and never the digest of one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryDevice {
    /// The opaque device id, as `DeviceIssued` and `DeviceRevoked` carry it.
    pub id: String,
    /// The label its owner recognizes it by.
    pub label: String,
    /// When the device was issued.
    pub created: SystemTime,
    /// When a request last presented its secret, as far as the owner has
    /// recorded it (recording is throttled, so this lags by up to a minute).
    pub last_used: Option<SystemTime>,
    /// The client address of that last use.
    pub last_client: Option<String>,
    /// When the device stops being accepted, when it expires at all.
    pub expires: Option<SystemTime>,
}

/// Read access to accounts and their devices. Implementations block on
/// storage, so asynchronous callers go through
/// [`Ctx::blocking`](crate::Ctx::blocking). Every paging method serves at
/// most [`MAX_PAGE_LIMIT`](crate::MAX_PAGE_LIMIT) items and refuses a larger
/// `limit` with `kernel.page_too_large`. Follow the next cursor even if a
/// page is short: invalid stored account names are skipped. Concurrent
/// inserts or deletes can shift later offset pages.
pub trait Directory: Port {
    /// The users, alphabetically by name.
    ///
    /// # Errors
    ///
    /// The provider's failure when the page cannot be read or `limit` is
    /// above the cap.
    fn users(&self, ctx: &Ctx, page: PageRequest<'_>) -> Fallible<Page<DirectoryUser>>;

    /// One user by canonical name, `None` when there is no such account.
    /// An account whose stored name cannot identify it is treated as absent.
    ///
    /// # Errors
    ///
    /// The provider's failure when the account cannot be read.
    fn user(&self, ctx: &Ctx, name: &UserName) -> Fallible<Option<DirectoryUser>>;

    /// The devices of one user, oldest first.
    ///
    /// # Errors
    ///
    /// The provider's failure when the user is unknown, the page cannot be
    /// read, or `limit` is above the cap.
    fn devices(
        &self,
        ctx: &Ctx,
        user: &UserName,
        page: PageRequest<'_>,
    ) -> Fallible<Page<DirectoryDevice>>;

    /// Who a device belongs to, `None` when it is absent or its owner's
    /// stored name cannot identify a user.
    ///
    /// # Errors
    ///
    /// The provider's failure when the lookup cannot be made.
    fn owner_of_device(&self, ctx: &Ctx, device: &str) -> Fallible<Option<UserName>>;
}
