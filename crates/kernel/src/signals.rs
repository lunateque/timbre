//! The signals the kernel itself publishes.

use crate::bus::Signal;
use crate::credential::RejectReason;
use crate::identity::UserId;
use crate::secret::Secret;
use std::net::IpAddr;

/// Published once after every module is registered: the server is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Started;

impl Signal for Started {}

/// A user is gone; modules holding their rows delete them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserRemoved {
    /// The user whose data modules must forget.
    pub user: UserId,
}

impl Signal for UserRemoved {}

/// A device secret was issued; the plaintext travels in-process only and is
/// never logged.
#[derive(Clone, Debug)]
pub struct DeviceIssued {
    /// The device label the secret belongs to.
    pub device: String,
    /// The user the device acts for.
    pub user: UserId,
    /// The new secret, readable only by whoever stores it.
    pub secret: Secret<String>,
}

impl Signal for DeviceIssued {}

/// A device was revoked; modules holding copies of its secret delete them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRevoked {
    /// The device whose secret is gone.
    pub device: String,
}

impl Signal for DeviceRevoked {}

/// A credential check ended in a rejection; published by `Ctx::authenticate`
/// after every one, so limiting modules can count failures per client and user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthFailed {
    /// The client the attempt came from.
    pub client: IpAddr,
    /// The named user, when the credential named one.
    pub user: Option<String>,
    /// Why the attempt was rejected.
    pub reason: RejectReason,
}

impl Signal for AuthFailed {}
