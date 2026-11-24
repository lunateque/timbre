//! Fuzz targets over Timbre's outside-facing parsers.
//!
//! Every exercise function drives the shipped parser through the crate's
//! `fuzzing` seam - no copies, so a parser change cannot silently leave the
//! fuzzing behind - and keeps every invariant assertion the benchmark of
//! ADR 0003 (`.internal/bench/fuzz`) used: round trips that must return the
//! input's pairs unchanged, and differential checks that re-derive the
//! answer from the documented contract and compare.
//!
//! The `nightly` binaries in `fuzz_targets/` are thin `fuzz_target!` shells
//! around these functions; `tests/corpus.rs` replays the same functions over
//! the checked-in corpus on stable, so `just ci` keeps a smoke run without a
//! nightly toolchain. Harness code is test code: an assertion *is* the
//! oracle, so `assert!`/`expect` live here exactly where the shipped crates
//! would forbid them.

pub mod config;
pub mod http;
pub mod identity;
pub mod image;
pub mod log_filter;
pub mod lyrics;
pub mod proxy;
pub mod tags;

pub use config::config;
pub use http::{form_roundtrip, http_head};
pub use identity::identity;
pub use image::image_thumbnail;
pub use log_filter::log_filter;
pub use lyrics::lrc_parse;
pub use proxy::proxy;
pub use tags::tags_read;
