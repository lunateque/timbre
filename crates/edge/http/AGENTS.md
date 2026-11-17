# crates/edge/http

## Purpose

The HTTP server: `serve` binds every configured address, speaks HTTP/1.1
and HTTP/2 cleartext through hyper, turns each request into the kernel's
`Request` (percent-decoded, RFC 3986 dot-normalized path, merged form
fields, the client address resolved through `trusted_proxies`, a request
id for log correlation only), routes it, runs the handler under
`ctx.with_request`/`with_client`, streams the `Response` back and logs
every request. The `http.request` log line never carries a secret: a
query or form value logs as `***` when its lowercased key contains
`token`, `key`, `secret`, `pass`, `auth`, `cookie`, `session` or `sid` or
is exactly `p`, `t`, `s`, `code` or `pwd`, every logged value is clipped
to 120 bytes and the whole logged target to 2 KiB, and no header value is
logged at all - `SECRET_HEADER_NAMES` lists the ones that must stay out
of any future line too. A `X-Forwarded-For` arriving as several header
lines is joined into one list, wire order, before the rightmost-untrusted
rule runs. The kernel `Response`'s framing headers (`content-length`,
`transfer-encoding`, `connection`, `keep-alive`, `upgrade`) are dropped
so hyper derives them from the body it serves. Limits: a configurable
header-read timeout (10 s by default) that also bounds protocol
detection - a connection silent through its first bytes is closed within
it and releases its slot - and detection parks between peeks of an
undecided prefix instead of spinning on the still-unread byte; a
configurable handler deadline (60 s default, 504 on expiry) that bounds
the time until the handler returns its response but never a streaming
body, so long downloads run on; 16 KiB of headers (h1 byte cap and
header count, h2 header list size), a configurable body cap (413 above
it), a configurable cap of query+form pairs (1024 by default; URL
overflow 414, key overflow 431, form overflow 413, each logged), a 30 s
configurable body read timeout (408 on expiry), at most 1024 concurrent
connections and at most 16 concurrent HTTP/2 streams on one connection
(HTTP/1.1 is one request at a time on its connection anyway), and a 10 s
drain of in-flight responses on shutdown. A streaming response body that
fails mid-flight cannot change the head any more; the failure logs as
`http.stream_failed` with its code.

The `[http]` config section is owned here: `HttpConfig::from_config`
reads it through `timbre-edge-config` (keys `listen`, `trusted_proxies`,
`max_body_bytes`, `header_read_timeout_ms`, `handler_timeout_ms`,
`body_read_timeout_ms`, `max_param_pairs`, each with the default of
`HttpConfig::default`; unknown keys fail with `config.invalid`), applies
`TIMBRE_HTTP__<KEY>` environment overrides, and converts the millisecond
values into the durations the server runs on.

## Ports

None.

## Signals

None.

## Log events

Every line but `http.listening` carries the standard `request` field: each
request runs on a `Ctx` bound to its request id, so its lines correlate
with the `http.request` summary.

- `http.listening` (`LISTENING`, info): a configured address is bound and
  accepting; fields `addr` (the bound address, so an ephemeral port is
  reported as the number it got).
- `http.request` (`REQUEST`, info): one request answered; fields `method`,
  `path` (the logged target: secret query/form values redacted, clipped),
  `status`, `bytes` (the body's size when known), `ms` (whole milliseconds
  since the request arrived), `client` (the address resolved through
  `trusted_proxies`).
- `http.handler_failed` (`HANDLER_FAILED`, warn): the route handler
  returned a failure; fields `code` (the failure's code), `status` (the
  status answered).
- `http.params_refused` (`PARAMS_REFUSED`, warn): the query or form broke
  a parameter limit and the request was refused before routing; fields
  `source` (`url` or `form`), `reason` (`key_too_long` or
  `too_many_pairs`), `pairs` (how many arrived).
- `http.body_failed` (`BODY_FAILED`, warn): the request body stream errored
  while being read; field `code` (`http.body_failed`).
- `http.body_timeout` (`BODY_TIMEOUT`, warn): the body arrived slower than
  `body_read_timeout`; field `code` (`http.body_timeout`).
- `http.handler_timeout` (`HANDLER_TIMEOUT`, warn): the handler was still
  running when `handler_timeout` expired; fields `code`
  (`http.handler_timeout`), `ms` (the configured timeout in whole
  milliseconds).
- `http.stream_failed` (`STREAM_FAILED`, warn): a streaming response body
  failed after its head was sent; field `code` (the producing failure's
  code).

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

## Kernel contract notes

- Query precedence belongs to this edge, not the kernel: the handler
  receives the URL pairs first, then the form pairs of a form POST, each
  group in wire order; repeated keys are kept, never merged.
- Request headers reach the kernel with lowercased names; the values of
  one name keep their receipt order, the order across different names is
  not guaranteed (hyper groups a header map by name), so nothing may read
  meaning into it.
