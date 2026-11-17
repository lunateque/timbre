//! Credentials: what a protocol extracts from a request, and who checks it.

use crate::ctx::Ctx;
use crate::failure::Fallible;
use crate::future::BoxFuture;
use crate::identity::{Identity, UserName};
use crate::names;
use crate::secret::Secret;
use std::borrow::Cow;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

/// The name of a credential scheme, shaped `<module>.<name>` and validated
/// exactly like a [`Permission`](crate::Permission): the module that defines
/// the scheme prefixes it, so two protocols never collide on one owner slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SchemeName(&'static str);

impl SchemeName {
    /// Build a scheme name, returning `None` when the name is invalid.
    pub const fn try_new(name: &'static str) -> Option<Self> {
        if names::is_valid(name) {
            Some(Self(name))
        } else {
            None
        }
    }

    /// Build a scheme name, failing the build where the constant is declared
    /// when the name is invalid.
    ///
    /// # Panics
    ///
    /// When `name` is not shaped `<module>.<name>`; in a `const` context this
    /// is a compile-time error at the declaration.
    #[expect(
        clippy::panic,
        reason = "an invalid scheme name must fail the build where the constant is declared"
    )]
    pub const fn new(name: &'static str) -> Self {
        match Self::try_new(name) {
            Some(name) => name,
            None => panic!("scheme name must be <module>.<name>"),
        }
    }

    /// The name as written at its declaration site.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// The generic shape of a credential; each kind has exactly one owner module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CredentialKind {
    /// An opaque key presented in a header or query parameter.
    ApiKey,
    /// A named user with the secret itself in the request.
    Password,
    /// A secret presented in an authorization line.
    Bearer,
    /// A user asserted by a trusted front end, with no secret.
    Forwarded,
    /// A named protocol scheme the kernel does not interpret.
    Scheme(SchemeName),
}

impl CredentialKind {
    /// The lowercase name used in messages.
    #[must_use]
    pub fn as_str(self) -> Cow<'static, str> {
        match self {
            Self::ApiKey => Cow::Borrowed("api key"),
            Self::Password => Cow::Borrowed("password"),
            Self::Bearer => Cow::Borrowed("bearer"),
            Self::Forwarded => Cow::Borrowed("forwarded"),
            Self::Scheme(name) => Cow::Owned(format!("scheme `{}`", name.as_str())),
        }
    }
}

/// A credential as the protocol edge read it from the request. The user
/// fields carry the raw wire text; `Ctx::authenticate` canonicalizes it with
/// [`crate::UserName::normalize`] before the throttle or any provider sees
/// it, and rejects a name that cannot canonicalize as
/// [`RejectReason::Invalid`].
#[derive(Debug)]
pub enum Credential {
    /// An opaque per-device or per-account key.
    ApiKey(Secret<String>),
    /// A user name with the secret in clear or in a module's own encoding.
    Password {
        /// The named user.
        user: String,
        /// The secret.
        secret: Secret<String>,
    },
    /// A bearer-style opaque secret presented in an authorization line.
    Bearer(Secret<String>),
    /// A user asserted by a trusted front end.
    Forwarded {
        /// The asserted user.
        user: String,
    },
    /// A protocol-specific scheme the kernel does not interpret: it routes by
    /// `name` only, and the owning module reads `fields`.
    Scheme {
        /// The scheme, named by the module that defined it.
        name: SchemeName,
        /// The user the scheme names, when it names one.
        user: String,
        /// The scheme's own fields, in the order the protocol read them;
        /// values stay secret so a `Debug` dump cannot leak one.
        fields: Vec<(String, Secret<String>)>,
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
            Self::Scheme { name, .. } => CredentialKind::Scheme(*name),
        }
    }

    /// The user named in the credential, if it names one. A `Scheme` whose
    /// user is empty names none: a scheme may derive the user from its own
    /// fields, and the kernel must not canonicalize (and so cannot reject) a
    /// placeholder the owner never intends to use.
    #[must_use]
    pub fn user(&self) -> Option<&str> {
        match self {
            Self::ApiKey(_) | Self::Bearer(_) => None,
            Self::Password { user, .. } | Self::Forwarded { user } => Some(user),
            Self::Scheme { user, .. } => (!user.is_empty()).then_some(user.as_str()),
        }
    }

    /// Replace the named user with its canonical spelling; only the kernel
    /// calls this, after [`UserName::normalize`] accepted the original text,
    /// so provider, throttle and log all see one string.
    pub(crate) fn set_user(&mut self, canonical: &str) {
        match self {
            Self::Password { user, .. } | Self::Forwarded { user } | Self::Scheme { user, .. } => {
                canonical.clone_into(user);
            }
            Self::ApiKey(_) | Self::Bearer(_) => {}
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
    /// The credential belongs to this identity.
    Recognized(Identity),
    /// The credential does not pass.
    Rejected(RejectReason),
}

/// The outcome of `Ctx::authenticate`, wider than a [`Verdict`].
#[derive(Debug, PartialEq, Eq)]
pub enum AuthOutcome {
    /// The credential belongs to this identity.
    Recognized(Identity),
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

/// Brute-force limiting over credential checks; registered with
/// [`Registry::throttle`](crate::Registry::throttle) (not through a port:
/// there is exactly one, the kernel itself is its only consumer) and its
/// absence means no in-process limiting.
pub trait Throttle: Send + Sync {
    /// Whether an attempt from `client` (and `user`, when the credential
    /// named one, in its canonical spelling) may proceed.
    fn allow(&self, client: IpAddr, user: Option<&UserName>) -> bool;
}

/// A kind's owner: the provider and the module id whose registration
/// installed it, kept so a second owner can be refused by naming both.
struct Owner {
    provider: Arc<dyn CredentialProvider>,
    module: Option<&'static str>,
}

/// The credential providers registered so far; owned by the kernel.
#[derive(Default)]
pub(crate) struct CredentialTable {
    owners: HashMap<CredentialKind, Owner>,
}

impl CredentialTable {
    pub(crate) fn add(
        &mut self,
        kind: CredentialKind,
        provider: Arc<dyn CredentialProvider>,
        module: Option<&'static str>,
    ) -> Fallible {
        if let Some(existing) = self.owners.get(&kind) {
            return Err(Box::new(
                crate::failures::KernelFailure::CredentialKindTaken {
                    kind: kind.as_str().into_owned(),
                    first: owner_label(existing.module),
                    second: owner_label(module),
                },
            ));
        }
        self.owners.insert(kind, Owner { provider, module });
        Ok(())
    }

    pub(crate) fn get(&self, kind: CredentialKind) -> Option<&Arc<dyn CredentialProvider>> {
        self.owners.get(&kind).map(|owner| &owner.provider)
    }
}

/// The failure text for a registration made outside any module (the binary
/// itself, or a test harness).
pub(crate) fn owner_label(module: Option<&'static str>) -> String {
    module.unwrap_or("<binary>").to_owned()
}

#[cfg(test)]
mod tests;
