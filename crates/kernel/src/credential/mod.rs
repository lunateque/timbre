//! Credentials: what a protocol extracts from a request, and who checks it.

use crate::ctx::Ctx;
use crate::failure::Fallible;
use crate::future::BoxFuture;
use crate::identity::Principal;
use crate::port::Port;
use crate::secret::Secret;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

/// The generic shape of a credential; each kind has exactly one owner module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CredentialKind {
    /// A bearer token from a header or query key.
    ApiKey,
    /// A named user with the secret itself in the request.
    Password,
    /// An `Authorization: Bearer` token.
    Bearer,
    /// A user asserted by a trusted front end, with no secret.
    Forwarded,
}

impl CredentialKind {
    /// The lowercase name used in messages.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api key",
            Self::Password => "password",
            Self::Bearer => "bearer",
            Self::Forwarded => "forwarded",
        }
    }
}

/// A credential as the protocol edge read it from the request.
#[derive(Debug)]
pub enum Credential {
    /// An API key (Subsonic `apiKey`, device tokens).
    ApiKey(Secret<String>),
    /// A user name with the secret in clear or in a module's own encoding.
    Password {
        /// The named user.
        user: String,
        /// The secret.
        secret: Secret<String>,
    },
    /// A bearer token.
    Bearer(Secret<String>),
    /// A user asserted by a trusted front end.
    Forwarded {
        /// The asserted user.
        user: String,
    },
}

impl Credential {
    /// The kind, which decides the owner module.
    #[must_use]
    pub fn kind(&self) -> CredentialKind {
        match self {
            Self::ApiKey(_) => CredentialKind::ApiKey,
            Self::Password { .. } => CredentialKind::Password,
            Self::Bearer(_) => CredentialKind::Bearer,
            Self::Forwarded { .. } => CredentialKind::Forwarded,
        }
    }

    /// The user named in the credential, if it names one.
    #[must_use]
    pub fn user(&self) -> Option<&str> {
        match self {
            Self::ApiKey(_) | Self::Bearer(_) => None,
            Self::Password { user, .. } | Self::Forwarded { user } => Some(user),
        }
    }
}

/// Why a credential was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RejectReason {
    /// The secret or user did not verify.
    Invalid,
    /// A `Throttle` refused to ask the provider at all.
    Throttled,
    /// The account or device exists but is disabled.
    Disabled,
}

impl RejectReason {
    /// The lowercase name used in logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Throttled => "throttled",
            Self::Disabled => "disabled",
        }
    }
}

/// The provider's answer about one credential.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The credential belongs to this principal.
    Recognized(Principal),
    /// The credential does not pass.
    Rejected(RejectReason),
}

/// The outcome of `Ctx::authenticate`, wider than a [`Verdict`].
#[derive(Debug, PartialEq, Eq)]
pub enum AuthOutcome {
    /// The credential belongs to this principal.
    Recognized(Principal),
    /// The credential does not pass.
    Rejected(RejectReason),
    /// No module owns this kind; the server cannot answer it at all.
    Unsupported,
}

/// The module that owns one [`CredentialKind`] and decides its verdicts.
pub trait CredentialProvider: Send + Sync {
    /// Check `credential`; the protocol maps the verdict to its wire codes.
    fn check(&self, ctx: Ctx, credential: Credential) -> BoxFuture<Verdict>;
}

/// Brute-force limiting over credential checks; `auth_guard` provides it and
/// its absence means no in-process limiting.
pub trait Throttle: Port {
    /// Whether an attempt from `client` (and `user`, when named) may proceed.
    fn allow(&self, client: IpAddr, user: Option<&str>) -> bool;
}

/// The credential providers registered so far; owned by the kernel.
#[derive(Default)]
pub(crate) struct CredentialTable {
    owners: HashMap<CredentialKind, Arc<dyn CredentialProvider>>,
}

impl CredentialTable {
    pub(crate) fn add(
        &mut self,
        kind: CredentialKind,
        provider: Arc<dyn CredentialProvider>,
    ) -> Fallible {
        if self.owners.contains_key(&kind) {
            return Err(Box::new(
                crate::failures::KernelFailure::CredentialKindTaken {
                    kind: kind.as_str(),
                },
            ));
        }
        self.owners.insert(kind, provider);
        Ok(())
    }

    pub(crate) fn get(&self, kind: CredentialKind) -> Option<&Arc<dyn CredentialProvider>> {
        self.owners.get(&kind)
    }
}

#[cfg(test)]
mod tests;
