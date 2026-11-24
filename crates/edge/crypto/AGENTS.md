# crates/edge/crypto

## Purpose

The primitives every secret in the tree is built from: operating-system
random bytes, SHA-256 digests, a constant-time digest comparison and the
lowercase base32 text form secrets travel in. The crate knows nothing about
users, devices or requests; `auth` composes a device secret out of these
pieces and stores only the digest.

Base32 (RFC 4648 lowercase, no padding) over base62 or hex: one case, no
padding characters, no `0`/`O` confusion, and a third shorter than hex. The
decoder is strict - foreign bytes, impossible lengths and non-zero padding
bits are refused - so one secret has exactly one spelling and a lookup
prefix can be compared as text.

## Ports

None. `auth` calls this edge directly.

## Signals

None.

## Failures

- `crypto.random_failed`: the operating system refused random bytes. A
  secret is never built from a fallback.
- `crypto.encoding_invalid`: the text is not canonical base32.
