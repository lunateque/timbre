# timbre-edge-tags

## Purpose

Read stream properties and native audio tags without interpreting legacy text
separators. Typed fields retain distinct values where the format stores a
list; ID3v2.3 single values remain raw. Pictures contain encoded bytes, and
lyrics retain the stored language, timestamp unit, and entry order.

The fingerprint combines duration, logical payload size, and XXH3-128 of
its first and last 64 KiB in that order, hashing overlapping bytes once.
For contiguous formats the payload is the audio range after stripping tags
and container metadata. MP4 concatenates the bodies of all top-level mdat
boxes in file order and uses the sum of their lengths. Ogg concatenates
only audio packet bytes, excluding identification, comment and setup packets
and all page framing; its size component is the final granule in sample
frames (minus Opus pre-skip). To avoid walking the audio stream, Ogg reads
from both ends, validating the CRC of pages located by reverse search. Ogg
physical bounds can still span page framing. Never use those bounds as the
logical payload for fingerprinting.

## Ports

None. In addition to reading tags and fingerprinting, `picture_range` locates
contiguous encoded picture bytes; `read_picture` handles transformed or
page-spanning pictures without measuring audio. The fixtures feature exposes
native tag writers for tests; fuzzing feeds the same parsers in memory.

## Signals

None.

## Failures

See src/failures.rs. Read and fingerprint return typed failures for I/O,
unsupported formats, or invalid audio data.
