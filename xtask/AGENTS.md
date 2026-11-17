# xtask

## Purpose

The repository's own tooling, run as `cargo xtask`: the `check` gate over the
workspace's shape, name prefixes, comment hygiene and dependency policy,
commit-message validation, module scaffolding (`module new|remove`) that keeps
the generated wiring consistent, and the removability build with each module
left out in turn. The `names` check ties every `LogEvent::new`,
`FailureCode::new`, `Permission::new` and `SchemeName::new` literal to the
owning package's module id and reports a workspace-wide duplicate, so a module
cannot claim a reserved prefix (`kernel`, `timbre`, `xtask`); the `migrations`
check ties every `.migration(...)` module literal to the same id.
It reads Rust source with the compiler's own lexer (`ra-ap-rustc_lexer`) and
`cargo metadata` output with `serde_json`, so the checks see code exactly as
rustc does instead of through a hand-written approximation.

## Ports

None.

## Signals

None.

## Failures

None of the module kind: commands report violations or errors as text and exit
with a non-zero status.
