# crates/edge/text

## Purpose

Use the same Unicode folding at search index and query time. Browse order
uses the same fold after dropping a leading English article. `contains_cjk`
identifies Han, Kana and Hangul for the library's trigram rule.

## Ports

None. The library calls this edge directly.

## Signals

None.

## Failures

None. The API takes valid Rust strings and has no I/O.
