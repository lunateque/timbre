//! Identity: who a request belongs to and what they may do.

use crate::names;

/// The stable identifier of a user.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UserId(String);

impl UserId {
    /// Wrap an identifier chosen by the module that owns users.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The identifier as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A declared capability, shaped `<module>.<snake_case_name>` and validated
/// exactly like a [`LogEvent`](crate::LogEvent), e.g. `auth.admin`, `library.scan`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Permission(&'static str);

impl Permission {
    /// Build a permission from a name, returning `None` when the name is invalid.
    pub const fn try_new(permission: &'static str) -> Option<Self> {
        if names::is_valid(permission) {
            Some(Self(permission))
        } else {
            None
        }
    }

    /// Build a permission from a name, failing the build where the constant is
    /// declared when the name is invalid.
    ///
    /// # Panics
    ///
    /// When `permission` is not shaped `<module>.<snake_case_name>`; in a
    /// `const` context this is a compile-time error at the declaration.
    #[expect(
        clippy::panic,
        reason = "an invalid permission must fail the build where the constant is declared"
    )]
    pub const fn new(permission: &'static str) -> Self {
        match Self::try_new(permission) {
            Some(permission) => permission,
            None => panic!("permission must be <module>.<snake_case_name>"),
        }
    }

    /// The permission as written at its declaration site.
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The module prefix of the permission.
    pub fn module(self) -> &'static str {
        names::module_of(self.0)
    }
}

/// Who is acting: a user and the permissions granted to them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Principal {
    /// The user this principal stands for.
    pub user: UserId,
    /// The granted permissions; `has` is the check modules make.
    pub permissions: Vec<Permission>,
}

impl Principal {
    /// Whether this principal holds `permission`.
    #[must_use]
    pub fn has(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }
}

/// A permission a module declared at registration: what it checks and why.
#[derive(Clone, Copy, Debug)]
pub struct PermissionEntry {
    /// The permission itself.
    pub permission: Permission,
    /// The description shown to whoever grants and revokes.
    pub description: &'static str,
}
