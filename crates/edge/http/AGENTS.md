# crates/edge/http

## Purpose

The HTTP server: `serve` binds every configured address, speaks HTTP/1.1
and HTTP/2 cleartext through hyper, turns each request into the kernel's
`Request` (percent-decoded, RFC 3986 dot-normalized path, merged form
fields, the client address resolved through `trusted_proxies`, a request
id for log correlation only), routes it, runs the route's `Guard` through
`Kernel::guarded` under `ctx.with_request` before the handler may run,
streams the `Response` back and logs every request. The route is looked up
before a single body byte is buffered: 404/405 answers never read the
body, and a `Guard::Requires` route whose credential rides the headers
runs its check first too - a denied request is refused without the body
being held in memory, so a bogus credential pointed at a large body costs
the server nothing beyond the socket drain hyper does after the answer.
The exceptions: a `application/x-www-form-urlencoded` POST (its credential
may travel in the body, so the body is read and merged first, under the
same `max_body_bytes`/`body_read_timeout` limits) and public routes (no
check worth running ahead of the body). The guard check itself is bounded
by `guard_timeout`: an unanswered check answers 504, logs
`http.guard_timeout` with the code, and drops the check's late outcome;
the handler never runs. A
`Guard::Requires` route whose check fails is answered with the protocol's
own `deny` response exactly like any handler reply - framing headers
stripped, the `http.request` line logs its status - and the handler never
runs; a passed check hands the handler the context carrying the sealed
principal, and the `http.request` line gains `user` (the canonical,
never-secret user name) when the request carries one. A denial names no
user: `Kernel::guarded` hands the edge only the rendered `deny` response,
never the `Denial` or the identity behind it, so even a 403 line stays
nameless (the kernel's own `kernel.access_denied` audit line carries the
user). The
`http.request` log line never carries a secret: a
query or form value logs as `***` when its lowercased key contains
`token`, `key`, `secret`, `pass`, `auth`, `cookie`, `session` or `sid` or
is exactly `p`, `t`, `s`, `code` or `pwd`, every logged value is clipped
to 120 bytes and the whole logged target to 2 KiB, and no header value is
logged at all - `SECRET_HEADER_NAMES` lists the ones that must stay out
of any future line too. A `X-Forwarded-For` arriving as several header
lines is joined into one list, wire order, before the rightmost-untrusted
rule runs; scanning right to left, a hop that does not parse as an address
(a proxy appending `unknown`) stops the scan and the peer is used, so a
client-forged left hop is never blamed for what the peer did. IPv4-mapped
IPv6 peers and forwarded hops are treated as IPv4 for proxy trust,
connection limits and request identity; a trusted entry written as an IPv6
network is still matched against the address as it arrived, so `::/0` and
prefixes wider than the mapped range cover a mapped peer too. The kernel
`Response`'s framing headers (`content-length`,
`transfer-encoding`, `connection`, `keep-alive`, `upgrade`) are dropped
so hyper derives them from the body it serves, with one exception: a
`Body::Stream` is the body hyper cannot size itself, so its
`content-length` reaches the wire when exactly one header carries one
well-formed value (ASCII digits a `u64` holds) - the edge also reports it
as the body's exact size, which is what frames the response by length
(chunked otherwise, and what makes a `HEAD` of the stream carry the header
with no body) and puts the declared number in the `http.request` line's
`bytes`. The stream must then produce exactly that many bytes: shorter, and
hyper aborts the connection at end-of-body without reusing it (the client
loses that answer, never the next one); longer, and the bytes past the
promise are cut and the connection stays usable. A malformed or duplicated
value is dropped like the rest of the framing and logged as
`http.content_length_invalid`, so a bad declaration costs the size hint,
never the response. Limits: a configurable
header-read timeout (10 s by default) that also bounds protocol
detection - a connection silent through its first bytes is closed within
it and releases its slot - and detection parks between peeks of an
undecided prefix instead of spinning on the still-unread byte; a
configurable handler deadline (60 s default, 504 on expiry) that bounds
the time until the handler returns its response but never a streaming
body, so long downloads run on; a configurable guard deadline (10 s
default, 504 on expiry) that bounds the route's guard check; 16 KiB of
headers (h1 byte cap and header count, h2 header list size), a
configurable body cap (413 above
it), a configurable cap of query+form pairs (1024 by default; URL
overflow 414, key overflow 431, form overflow 413, each logged; a form is
parsed bounded, so an over-cap body costs the cap plus one pair and not
the whole list), a 30 s
configurable body read timeout (408 on expiry), at most 1024 concurrent
connections and at most 16 concurrent HTTP/2 streams on one connection
(HTTP/1.1 is one request at a time on its connection anyway), a
configurable cap of live connections per client address
(`max_connections_per_client`, 64 by default, a configured `0` clamps to 1 -
zero would refuse every peer forever; a peer over the cap is
closed at once and logged as `http.connections_full`, and peers listed in
`trusted_proxies` are exempt because behind a proxy every client shares
the proxy's one address), and a 10 s
drain of in-flight responses on shutdown. The service waits for tracked
request futures (including separately polled HTTP/2 requests) to finish or be
cancelled after the graceful shutdown grace. It does not wait for work
detached from those futures, including a blocking closure whose
`Ctx::blocking` awaiter was cancelled. The edge does not publish
`ServerDrained`; the binary does that after the service returns. A streaming
response body that fails mid-flight cannot change the head any more; the failure logs as
`http.stream_failed` with its code. Every accepted socket gets TCP
keepalive (idle 30 s, probes every 15 s, four misses kill it), and every
response write runs behind a configurable send-stall guard
(`send_stall_timeout_ms`, 300 s by default, deliberately generous because
a paused player legitimately stops reading): a write or flush that lands
no byte for that long fails the connection and logs `http.send_stall`.
The guard wraps the connection's IO under both protocols, below hyper's
state machines; the bound counts time since the last byte moved, so a
slow-but-moving client is never punished.

The `[http]` config section is owned here: `HttpConfig::from_config`
reads it through `timbre-edge-config` (keys `listen`, `trusted_proxies`,
`max_body_bytes`, `header_read_timeout_ms`, `handler_timeout_ms`,
`guard_timeout_ms`,
`body_read_timeout_ms`, `max_param_pairs`, `max_connections_per_client`,
`send_stall_timeout_ms`, each with the default of
`HttpConfig::default`; unknown keys fail with `config.invalid`), applies
`TIMBRE_HTTP__<KEY>` environment overrides, and converts the millisecond
values into the durations the server runs on.

## Ports

None.

## Signals

None.

## Log events

Every line but `http.listening`, `http.connections_full` and
`http.send_stall` carries the standard `request` field: each
request runs on a `Ctx` bound to its request id, so its lines correlate
with the `http.request` summary; the two connection-level lines belong to
no request, only to a peer address.

- `http.listening` (`LISTENING`, info): a configured address is bound and
  accepting; fields `addr` (the bound address, so an ephemeral port is
  reported as the number it got).
- `http.request` (`REQUEST`, info): one request answered; fields `method`,
  `path` (the logged target: secret query/form values redacted, clipped),
  `status` (a guard's deny response included: the route's refusal status,
  not a kernel error), `bytes` (the body's size when known), `ms` (whole
  milliseconds since the request arrived), `client` (the address resolved
  through `trusted_proxies`), and `user` - the authenticated canonical
  user name - only when the request passed a `Guard::Requires` check
  (a denial carries no name: `Kernel::guarded` returns only the rendered
  `deny` response, so even a 403 line stays nameless).
- `http.handler_failed` (`HANDLER_FAILED`, warn): the route handler
  returned a failure; fields `code` (the failure's code), `status` (the
  status answered).
- `http.params_refused` (`PARAMS_REFUSED`, warn): the query or form broke
  a parameter limit and the request was refused before the guard and the
  handler; fields `source` (`url` or `form`), `reason` (`key_too_long` or
  `too_many_pairs`), `pairs` (how many arrived).
- `http.body_failed` (`BODY_FAILED`, warn): the request body stream errored
  while being read; field `code` (`http.body_failed`).
- `http.body_timeout` (`BODY_TIMEOUT`, warn): the body arrived slower than
  `body_read_timeout`; field `code` (`http.body_timeout`).
- `http.handler_timeout` (`HANDLER_TIMEOUT`, warn): the handler was still
  running when `handler_timeout` expired; fields `code`
  (`http.handler_timeout`), `ms` (the configured timeout in whole
  milliseconds).
- `http.guard_timeout` (`GUARD_TIMEOUT`, warn): the route's guard check
  was still running when `guard_timeout` expired; the client is answered
  504; fields `code` (`http.guard_timeout`), `ms` (the configured timeout
  in whole milliseconds).
- `http.stream_failed` (`STREAM_FAILED`, warn): a streaming response body
  failed after its head was sent; field `code` (the producing failure's
  code).
- `http.content_length_invalid` (`CONTENT_LENGTH_INVALID`, warn): a
  streamed response carried a `content-length` the edge cannot trust - a
  malformed value, or more than one - so it was dropped and hyper framed the
  body chunked instead; field `value` (what arrived, clipped).
- `http.connections_full` (`CONNECTIONS_FULL`, warn): a peer already holds
  `max_connections_per_client` live connections and its new one was closed
  at once; fields `peer` (the address) and `limit` (the cap it crossed).
- `http.send_stall` (`SEND_STALL`, warn): a response write made no progress
  for `send_stall_timeout`; the connection is failed and its slot released;
  fields `peer` (the address) and `ms` (the bound that expired).

## Failures

- `http.bind_failed` (`HttpFailure::BindFailed`): an address could not be
  parsed or bound.
- `http.proxy_invalid` (`HttpFailure::ProxyInvalid`): a `trusted_proxies`
  entry is neither an address nor a CIDR prefix; `serve` refuses to start.
- `http.body_failed` (`HttpFailure::BodyFailed`): the request body stream
  errored; the wire answer is 400 with the failure's public message.
- `http.body_timeout` (`HttpFailure::BodyTimeout`): the request body
  arrived slower than `body_read_timeout`; the wire answer is 408.
- `http.handler_timeout` (`HttpFailure::HandlerTimeout`): the handler was
  still running when `handler_timeout` expired; the wire answer is 504.
- `http.guard_timeout` (`HttpFailure::GuardTimeout`): the guard check was
  still running when `guard_timeout` expired; the wire answer is 504 and
  the check's late outcome is dropped.

## Kernel contract notes

- Query precedence belongs to this edge, not the kernel: the handler
  receives the URL pairs first, then the form pairs of a form POST, each
  group in wire order; repeated keys are kept, never merged.
- Request headers reach the kernel with lowercased names; the values of
  one name keep their receipt order, the order across different names is
  not guaranteed (hyper groups a header map by name), so nothing may read
  meaning into it.
