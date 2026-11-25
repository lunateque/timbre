//! The signals the kernel itself publishes.

use crate::bus::Signal;
use crate::credential::{RejectDetail, RejectReason};
use crate::identity::{Permission, UserName};
use crate::secret::Secret;
use std::net::IpAddr;
use std::time::SystemTime;

/// Published once after every module is registered: the server is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Started;

impl Signal for Started {}

/// Published after HTTP stops accepting, its request handlers have ended
/// and jobs have had their shutdown grace, whether serving ended cleanly
/// or with a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerDrained;

impl Signal for ServerDrained {}

/// A user was created; modules that keep per-user state learn the name
/// here, so nothing has to ask the module that owns users.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserAdded {
    /// The new user.
    pub user: UserName,
}

impl Signal for UserAdded {}

/// A user is gone; modules holding their rows delete them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserRemoved {
    /// The user whose data modules must forget.
    pub user: UserName,
}

impl Signal for UserRemoved {}

/// What a user may do changed: either permissions were granted or revoked,
/// or the admin flag (which stands for every declared permission) moved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionsChanged {
    /// The user whose access changed.
    pub user: UserName,
    /// Permissions the change added.
    pub granted: Vec<Permission>,
    /// Stored permission names the change took away, including names no
    /// registered module still declares.
    pub revoked: Vec<String>,
    /// Whether the user is an admin after the change.
    pub admin: bool,
}

impl Signal for PermissionsChanged {}

/// A device secret was issued; the plaintext travels in-process only and is
/// never logged.
#[derive(Clone, Debug)]
pub struct DeviceIssued {
    /// The opaque id of the device the secret belongs to.
    pub device: String,
    /// The label its owner recognizes the device by.
    pub label: String,
    /// The user the device acts for.
    pub user: UserName,
    /// The new secret, readable only by whoever stores it.
    pub secret: Secret<String>,
}

impl Signal for DeviceIssued {}

/// A device was revoked; modules holding copies of its secret delete them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRevoked {
    /// The opaque id of the device whose secret is gone.
    pub device: String,
    /// The user the device acted for.
    pub user: UserName,
}

impl Signal for DeviceRevoked {}

/// A credential was rejected; published by `Ctx::authenticate`. Refusals
/// by the throttle bypass the journal and are counted by the kernel instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthFailed {
    /// The client the attempt came from.
    pub client: IpAddr,
    /// The canonical user name, when the credential named a valid one;
    /// `None` when the text given was not a user name the kernel can
    /// canonicalize (the audit line still carries the clipped candidate).
    pub user: Option<UserName>,
    /// Why the attempt was rejected.
    pub reason: RejectReason,
    /// Provider-specific detail for the operator; never part of a wire denial.
    pub detail: Option<RejectDetail>,
}

impl Signal for AuthFailed {}

/// A recognized identity was refused a guarded route because it does not
/// hold the route's permission; published by the guard check after every
/// such refusal, so limiting and auditing modules see what the protocol's
/// own denial hides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessDenied {
    /// The client the request came from.
    pub client: IpAddr,
    /// The recognized user who lacked the permission.
    pub user: UserName,
    /// The permission the route required.
    pub permission: Permission,
}

impl Signal for AccessDenied {}

/// A user completed a play submission for a track; external scrobblers and
/// other listening-history consumers subscribe here without touching the
/// module that stores plays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Played {
    /// The canonical user who played the track.
    pub user: UserName,
    /// Opaque catalog track id.
    pub track: String,
    /// When the play occurred.
    pub at: SystemTime,
}

impl Signal for Played {}
