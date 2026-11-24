# crates/edge/image

## Purpose

Decode JPEG, PNG and WebP art, resize with Lanczos3 to a requested long
edge and encode a JPEG at quality 80. The encoded length and header
dimensions are checked before decoding pixels. AVIF is not enabled.

## Ports

None. The library calls this edge directly.

## Signals

None.

## Failures

`image.unsupported` for other formats, `image.limit` for oversized
images, `image.decode_failed` for malformed supported inputs and
`image.thumbnail_failed` for resize or encode failures.
