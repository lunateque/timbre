//! Identity: who a request belongs to and what they may do.

use crate::names;

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

/// The longest accepted user name, in bytes.
const MAX_USER_NAME_BYTES: usize = 64;

/// The accepted user name bytes: lowercase is enforced by normalization.
const fn is_name_byte(byte: u8) -> bool {
    matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-')
}

const fn is_valid_user_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let len = bytes.len();
    let [b'a'..=b'z' | b'0'..=b'9', tail @ ..] = bytes else {
        return false;
    };
    if len > MAX_USER_NAME_BYTES {
        return false;
    }
    let mut rest = tail;
    while let [byte, tail @ ..] = rest {
        // Names are lowercased before this runs, so an uppercase byte fails.
        if !is_name_byte(*byte) {
            return false;
        }
        rest = tail;
    }
    true
}

/// A user name in the kernel's one canonical spelling: 1 to 64 ASCII bytes.
/// It starts with a lowercase letter or digit; subsequent bytes may also
/// be `.`, `_` or `-`.
/// [`UserName::normalize`] is the only way in from arbitrary text,
/// so throttling, audit and logs always see the same string for one account.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UserName(String);

impl UserName {
    /// The longest accepted name, in bytes.
    pub const MAX_LENGTH: usize = MAX_USER_NAME_BYTES;

    /// Canonicalize `input`: trimmed of surrounding ASCII whitespace, then
    /// ASCII-lowercased. The result must begin with a lowercase letter or
    /// digit, contain only `[a-z0-9._-]`, and fit [`UserName::MAX_LENGTH`].
    /// The trimmed-and-lowercased candidate is returned as `Err` so the
    /// caller can report what it was given.
    ///
    /// # Errors
    ///
    /// The trimmed-and-lowercased candidate when it is not a valid name.
    pub fn normalize(input: &str) -> Result<Self, String> {
        let trimmed = input.trim_matches(|byte: char| byte.is_ascii_whitespace());
        let lowered = trimmed.to_ascii_lowercase();
        if is_valid_user_name(&lowered) {
            Ok(Self(lowered))
        } else {
            Err(lowered)
        }
    }

    /// Parse a persisted name that must already be in canonical form.
    #[must_use]
    pub fn parse_canonical(input: &str) -> Option<Self> {
        is_valid_user_name(input).then(|| Self(input.to_owned()))
    }

    /// The canonical name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for UserName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The longest byte offset at or below `bound` that ends on a `char`
/// boundary of `text`: where a clip to fit a length limit must end.
pub(crate) fn clip_boundary(text: &str, bound: usize) -> usize {
    let mut end = 0;
    for (offset, ch) in text.char_indices() {
        let next = offset + ch.len_utf8();
        if next > bound {
            break;
        }
        end = next;
    }
    end
}

/// What a credential provider recognized: a user and the permissions granted
/// to them. Providers answer with this, never with a [`Principal`]: only the
/// kernel can mint the principal a guarded handler runs as. The permissions
/// a provider grants are not checked against the permissions declared at
/// registration: a provider is trusted with its own verdict, and a guard is
/// the only consumer, so a permission that was never declared simply can
/// never satisfy a guard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// The user this identity stands for.
    pub user: UserName,
    /// The granted permissions; [`Principal::has`] is the check modules make.
    pub permissions: Vec<Permission>,
}

/// Who is acting: a sealed record the kernel builds from an [`Identity`]
/// during a guard check, so no handler can invent a principal for itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Principal {
    user: UserName,
    permissions: Vec<Permission>,
}

impl Principal {
    pub(crate) fn from_identity(identity: &Identity) -> Self {
        Self {
            user: identity.user.clone(),
            permissions: identity.permissions.clone(),
        }
    }

    /// The user this principal stands for.
    #[must_use]
    pub fn user(&self) -> &UserName {
        &self.user
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn normalize(input: &str) -> String {
        UserName::normalize(input)
            .unwrap_or_else(|rejected| panic!("`{input}` must normalize, got `{rejected}`"))
            .to_string()
    }

    #[test]
    fn normalization_trims_and_lowercases() {
        assert_eq!(normalize("  Ann-Marie_2.x  "), "ann-marie_2.x");
        assert_eq!(normalize("9"), "9");
    }

    #[test]
    fn other_forms_are_rejected() {
        for bad in [
            "",
            "   ",
            "--admin",
            ".",
            "..",
            "_alice",
            "a b",
            "ана",
            "user@host",
            "user+tag",
            &"x".repeat(UserName::MAX_LENGTH + 1),
        ] {
            assert!(
                UserName::normalize(bad).is_err(),
                "`{bad}` must be rejected"
            );
        }
        assert!(
            UserName::normalize(&"x".repeat(UserName::MAX_LENGTH)).is_ok(),
            "the bound is inclusive"
        );
    }

    #[test]
    fn a_rejected_name_comes_back_lowercased_and_trimmed() {
        let rejected = UserName::normalize("  A B  ").expect_err("a space is not accepted");
        assert_eq!(rejected, "a b", "the report is the canonical candidate");
    }
}
