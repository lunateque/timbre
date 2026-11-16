//! The module list: one by-value registration line pair per module, kept in
//! step with the cargo features by `cargo xtask module new|remove`.

use timbre_kernel::{Fallible, Registry};

// With the hello feature off the body never returns an error, but the
// `Fallible<Registry>` shape is the fixed seam `xtask module` edits into.
#[cfg_attr(
    not(feature = "hello"),
    expect(clippy::unnecessary_wraps, reason = "stable signature for xtask edits")
)]
pub(crate) fn registry() -> Fallible<Registry> {
    let registry = Registry::new();
    #[cfg(feature = "hello")]
    let registry = registry.module(timbre_hello::register)?;
    Ok(registry)
}
