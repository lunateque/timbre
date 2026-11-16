//! Stable failure codes and the `Failure` trait every cross-module error implements.

use crate::names;

/// A stable, machine-readable identifier for a failure, shaped `<module>.<snake_case_name>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FailureCode(&'static str);

impl FailureCode {
    /// Build a code from a name, returning `None` when the name is invalid.
    pub const fn try_new(code: &'static str) -> Option<Self> {
        if names::is_valid(code) {
            Some(Self(code))
        } else {
            None
        }
    }

    /// Build a code from a name, failing the build where the constant is declared
    /// when the name is invalid.
    ///
    /// # Panics
    ///
    /// When `code` is not shaped `<module>.<snake_case_name>`; in a `const`
    /// context this is a compile-time error at the declaration.
    #[expect(
        clippy::panic,
        reason = "an invalid code must fail the build where the constant is declared"
    )]
    pub const fn new(code: &'static str) -> Self {
        match Self::try_new(code) {
            Some(code) => code,
            None => panic!("failure code must be <module>.<snake_case_name>"),
        }
    }

    /// The code as written at its declaration site.
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The module prefix of the code.
    pub fn module(self) -> &'static str {
        names::module_of(self.0)
    }
}

/// An error that may cross a module boundary: a stable code, an HTTP status and
/// a user-safe message. Detail stays in the `Debug` output, which only logs see.
pub trait Failure: std::fmt::Debug + Send + Sync + 'static {
    /// The stable code identifying this failure.
    fn code(&self) -> FailureCode;
    /// The HTTP status a boundary should answer with.
    fn status(&self) -> u16;
    /// A message safe to show users.
    fn public_message(&self) -> &str;
}

/// The result type used at every module boundary. Errors are boxed explicitly
/// with `Box::new(..)`; there is no blanket `From` impl to keep conversions visible.
pub type Fallible<T = ()> = Result<T, Box<dyn Failure>>;
