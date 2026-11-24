//! Ordered result pages: the shape every port between modules returns.

use crate::{Fallible, failures::KernelFailure};

/// The largest page a port method serves. Larger requests are rejected
/// rather than clamped, so a caller never mistakes a short page for the end.
pub const MAX_PAGE_LIMIT: usize = 500;

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

/// Request the first page or continue from a cursor returned by the same list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRequest<'a> {
    /// Maximum number of items to return, at most [`MAX_PAGE_LIMIT`].
    pub limit: usize,
    /// `None` on the first request.
    pub cursor: Option<&'a Cursor>,
}

/// An offset window for a paged list. Offset cursors do not pin a snapshot
/// across calls: concurrent insertions or deletions can shift later pages.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    count: usize,
    offset: usize,
}

impl Window {
    /// Decode an offset cursor and check the count against [`MAX_PAGE_LIMIT`].
    ///
    /// # Errors
    /// `kernel.page_too_large` above the cap, or `kernel.cursor_invalid`
    /// when the cursor is not an offset.
    pub fn new(request: PageRequest<'_>) -> Fallible<Self> {
        Self::check_limit(request.limit)?;
        let offset = request
            .cursor
            .map_or(Ok(0), |cursor| cursor.as_str().parse())
            .map_err(|_| -> Box<dyn crate::Failure> { Box::new(KernelFailure::CursorInvalid) })?;
        Ok(Self {
            count: request.limit,
            offset,
        })
    }

    /// Validate a count without reading a cursor, for conditional requests.
    ///
    /// # Errors
    /// `kernel.page_too_large` above [`MAX_PAGE_LIMIT`].
    pub fn check_limit(count: usize) -> Fallible {
        if count > MAX_PAGE_LIMIT {
            return Err(Box::new(KernelFailure::PageTooLarge));
        }
        Ok(())
    }

    /// Build a window from an explicit count and offset, without a cursor.
    ///
    /// # Errors
    /// `kernel.page_too_large` above [`MAX_PAGE_LIMIT`].
    pub fn from_offset(count: usize, offset: usize) -> Fallible<Self> {
        Self::check_limit(count)?;
        Ok(Self { count, offset })
    }

    /// The initial window for a full traversal.
    pub const fn first() -> Self {
        Self {
            count: MAX_PAGE_LIMIT,
            offset: 0,
        }
    }

    /// The requested page size, without a lookahead row.
    pub const fn count(self) -> usize {
        self.count
    }

    /// The number of rows to skip.
    pub const fn offset(self) -> usize {
        self.offset
    }

    /// One extra row to decide whether a next page exists.
    pub const fn lookahead(self) -> usize {
        self.count + 1
    }

    /// The limit with lookahead, for queries using signed integers.
    pub fn limit(self) -> i64 {
        i64::try_from(self.lookahead()).unwrap_or(i64::MAX)
    }

    /// The offset for queries using signed integers.
    pub fn skip(self) -> i64 {
        i64::try_from(self.offset).unwrap_or(i64::MAX)
    }

    /// Move to the next window if the result contained a lookahead row.
    pub const fn advance(&mut self, rows: usize) -> bool {
        if self.count == 0 || rows <= self.count {
            return false;
        }
        self.offset = self.offset.saturating_add(self.count);
        true
    }

    /// Drop any lookahead rows and return the matching continuation token.
    pub fn page<T>(mut self, mut items: Vec<T>) -> Page<T> {
        let next = self
            .advance(items.len())
            .then(|| Cursor::new(self.offset.to_string()));
        items.truncate(self.count);
        Page { items, next }
    }
}
