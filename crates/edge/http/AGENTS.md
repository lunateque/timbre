# crates/edge/http

## Purpose

The HTTP server: `serve` binds every configured address, speaks HTTP/1.1
and HTTP/2 cleartext through hyper, turns each request into the kernel's
`Request` (percent-decoded path, merged form fields, the client address
resolved through `trusted_proxies`, a random request id), routes it, runs
the handler under `ctx.with_request`/`with_client`, streams the `Response`
back and logs every request. The `http.request` log line never carries a
secret: query and form values of `apiKey`, `p`, `t`, `s`, `token` and
`password` log as `***` (case-insensitive), and no header value is logged
at all - `SECRET_HEADER_NAMES` lists the ones that must stay out of any
future line too. Limits: 10 s header read timeout, 16 KiB of headers
(h1 byte cap and header count, h2 header list size), a configurable body
cap (413 above it), a 30 s configurable body read timeout (408 on
expiry), at most 1024 concurrent connections and at most 16 concurrent
HTTP/2 streams on one connection (HTTP/1.1 is one request at a time on
its connection anyway), and a 10 s drain of in-flight responses on
shutdown.

## Ports

None.

## Signals

None.

## Failures

- `http.bind_failed` (`HttpFailure::Bind`): an address could not be parsed
  or bound.
- `http.proxy_invalid` (`HttpFailure::ProxyInvalid`): a `trusted_proxies`
  entry is neither an address nor a CIDR prefix; `serve` refuses to start.
- `http.body_failed` (`HttpFailure::BodyUnavailable`): the request body
  stream errored; the wire answer is 400.
- `http.body_timeout` (`HttpFailure::BodyTimeout`): the request body
  arrived slower than `body_read_timeout`; the wire answer is 408.
