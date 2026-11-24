# fuzz

## Purpose

The fuzzing harness: `cargo-fuzz` (libFuzzer, ADR 0003) targets over the
real outside-facing parsers - request-target normalization, query and form
decoding, header pair conversion, path parameter extraction, `X-Forwarded-For`
resolution, `UserName::normalize`, the `[http]` config section, audio tags,
bounded image thumbnails and enhanced LRC lyrics. Each target drives
the shipped parser (no copies: a parser change cannot silently leave the fuzzing
behind) and keeps the benchmark's invariant style: round trips and
differential checks that re-derive the answer from the documented contract
and compare, so a wrong result crashes exactly like a panic does.

This directory is its own cargo workspace (own `[workspace]`, own lockfile,
own `rust-toolchain.toml` pinning `nightly`) and is `exclude`d from the root
workspace: libFuzzer needs a nightly and a clang C++ build, and the stable
`just ci` must not pay for either. The stable part of the story is the
smoke run: `tests/corpus.rs` replays every checked-in seed under
`corpus/<target>/` through the same target functions with
`--no-default-features` (the `#![no_main]` fuzz bins are gated behind the
default `fuzz-targets` feature and are not linked on stable), so a crash on
a known input fails the normal gate, not only the nightly job.

Layout: `src/` holds the target functions (one module per parser family),
`fuzz_targets/` the thin `fuzz_target!` shells cargo-fuzz builds,
`corpus/<target>/seed-*` the tracked seeds, `dict/timbre.dict` the shared
dictionary. `corpus/` and `artifacts/` beyond the `seed-*` files are git
ignored - they are local fuzzer working state. A `.internal` tree, an
`xtask` and the crate-tier rules of the main workspace do not apply here;
this is test code and asserts freely.

## Ports

None: the harness directly drives the kernel, edge crates and the library's
LRC parser. Only fuzzing may know concrete modules.

## Signals

None.

## Failures

None of its own: a target reports by panicking, and libFuzzer turns the
panic (or a sanitizer trap, or a `-timeout=10` overrun) into a crash file
under `artifacts/<target>/`. Every crash becomes a regression test: shrink
with `cargo fuzz tmin <target> <artifact>`, check the bytes in as
`corpus/<target>/seed-regression-<short-name>`, and fix the parser (with a
unit test next to it) in the same change.

## Adding a target

1. Write the exercise function in `src/<family>.rs` - parse, then assert the
   contract (round trip, differential, cap), never `unwrap` a parser result
   the input is allowed to break; `# Panics` section mandatory.
2. Add the `fuzz_target!` shell in `fuzz_targets/<name>.rs` and a matching
   `[[bin]]` entry (with `required-features = ["fuzz-targets"]`) in
   `fuzz/Cargo.toml`, plus a `replay` test in `tests/corpus.rs`.
3. Seed `corpus/<name>/` with at least one `seed-*` file, run
   `cargo +nightly fuzz run <name>` (or `just fuzz <name> 60`) for a few
   minutes, and fold the discoveries back by committing the interesting
   `corpus/<name>/` files cargo-fuzz produced as `seed-*`.
4. Add the target name to `strategy.matrix.target` in
   `.github/workflows/fuzz.yml`, or the nightly job never runs it.
5. If the seam is missing a parser, add a delegating wrapper to the edge
   crate's `fuzzing.rs` (behind its `fuzzing` feature) - never a copy of
   the logic.
