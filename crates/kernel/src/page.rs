//! Ordered result pages: the shape every port between modules returns.

/// An opaque continuation token; only the port that issued it reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor(String);

impl Cursor {
    /// Wrap an opaque token.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The token as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One page of a larger result: `items` in the method's documented stable
/// order, and `next` when more items follow. Ports return pages instead of
/// iterators so a caller can stream page after page without holding the whole
/// result set in memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    /// The page's items, in the method's documented order.
    pub items: Vec<T>,
    /// The cursor asking for the following page, `None` at the end.
    pub next: Option<Cursor>,
}
