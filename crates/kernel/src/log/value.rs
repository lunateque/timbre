//! Typed field values accepted by the logger.

use crate::secret::Secret;
use std::time::Duration;

/// A log field value after conversion from the Rust type written at the call site.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
    /// A boolean.
    Bool(bool),
    /// Any integer, widened so the logger needs no per-width formatting.
    Int(i128),
    /// A float; non-finite values render as `null` in JSON, which has no infinity.
    Float(f64),
    /// A borrowed string.
    Str(&'a str),
    /// A hidden value, such as a [`Secret`].
    Redacted,
}

/// Convert a value into something the logger can write. Implement for types
/// modules pass as `log!` fields; everything here is free of allocation.
pub trait LogValue {
    /// The loggable view of this value.
    fn log_value(&self) -> Value<'_>;
}

impl LogValue for bool {
    fn log_value(&self) -> Value<'_> {
        Value::Bool(*self)
    }
}

macro_rules! impl_int {
    ($($ty:ty),*) => {
        $(
            impl LogValue for $ty {
                fn log_value(&self) -> Value<'_> {
                    Value::Int(i128::from(*self))
                }
            }
        )*
    };
}

impl_int!(i8, i16, i32, i64, u8, u16, u32, u64);

macro_rules! impl_size_int {
    ($($ty:ty),*) => {
        $(
            impl LogValue for $ty {
                fn log_value(&self) -> Value<'_> {
                    // Only on platforms wider than i128 could this saturate, and never in practice.
                    Value::Int(i128::try_from(*self).unwrap_or(i128::MAX))
                }
            }
        )*
    };
}

impl_size_int!(isize, usize);

impl LogValue for f32 {
    fn log_value(&self) -> Value<'_> {
        Value::Float(f64::from(*self))
    }
}

impl LogValue for f64 {
    fn log_value(&self) -> Value<'_> {
        Value::Float(*self)
    }
}

impl LogValue for str {
    fn log_value(&self) -> Value<'_> {
        Value::Str(self)
    }
}

impl LogValue for String {
    fn log_value(&self) -> Value<'_> {
        Value::Str(self.as_str())
    }
}

impl LogValue for Duration {
    fn log_value(&self) -> Value<'_> {
        Value::Int(i128::try_from(self.as_millis()).unwrap_or(i128::MAX))
    }
}

impl<T> LogValue for Secret<T> {
    fn log_value(&self) -> Value<'_> {
        Value::Redacted
    }
}

impl<T: LogValue + ?Sized> LogValue for &T {
    fn log_value(&self) -> Value<'_> {
        T::log_value(self)
    }
}
