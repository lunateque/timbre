//! One request: decode, enforce limits, route, run the handler, answer, log.

use crate::adapter;
use crate::body;
use crate::failures::HttpFailure;
use crate::form;
use crate::log_events::{GUARD_TIMEOUT, HANDLER_FAILED, HANDLER_TIMEOUT, PARAMS_REFUSED, REQUEST};
use crate::params::{self, ParamsSource};
use crate::proxy::{client_address, forwarded_for};
use crate::serve::{HyperReply, Limits};
use hyper::body::Incoming;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use timbre_kernel::http::{Body, Request};
use timbre_kernel::{Ctx, Failure, Guard, Level, LogValue, RequestId, RouteMatch, log};

/// Answer one request and log it.
pub(crate) async fn handle(
    ctx: Ctx,
    limits: Arc<Limits>,
    peer: SocketAddr,
    incoming: http::Request<Incoming>,
) -> HyperReply {
    let started = Instant::now();
    let id = RequestId::new(crate::serve::next_request_id());
    let request_ctx = ctx.with_request(id);
    let answered = process(&request_ctx, &limits, peer, incoming).await;
    let status = u64::from(answered.reply.status().as_u16());
    let bytes = body::body_bytes(&answered.reply);
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let client = answered.client.to_string();
    // The `log!` macro takes its keys as bare identifiers, but the field
    // list here is conditional: a request the guard authenticated gains
    // the canonical user name (a name, never a secret: `UserName` admits
    // only `[a-z0-9._-]`), and a public or denied request logs none.
    let mut fields: Vec<(&'static str, &dyn LogValue)> = vec![
        ("method", &answered.method),
        ("path", &answered.path),
        ("status", &status),
        ("bytes", &bytes),
        ("ms", &ms),
        ("client", &client),
    ];
    if let Some(user) = answered.user.as_ref() {
        fields.push(("user", user));
    }
    // `Ctx::log` filters like the macro would; `http.request` stays out
    // of the wire under `TIMBRE_LOG=warn` either way.
    answered.ctx.log(Level::Info, &REQUEST, &fields);
    answered.reply
}

/// What `process` produced for one request.
struct Answered {
    ctx: Ctx,
    reply: HyperReply,
    method: String,
    /// The path with its query as it may be logged: secret values redacted.
    path: String,
    client: IpAddr,
    /// The canonical user name a guard authenticated, when this request
    /// passed one; a public or denied request has no principal to name.
    user: Option<String>,
}

/// What the timed guard check produced for one request.
enum Guarded {
    /// The check passed; `authorized` is the only way to the handler, and
    /// its context carries the principal the check established.
    Passed(timbre_kernel::Authorized),
    /// The check denied; the reply is the protocol's own `deny` response.
    Denied(HyperReply),
    /// The check did not finish within `guard_timeout`; the client is
    /// answered 504 and the check's late outcome is dropped.
    TimedOut,
}

/// The request path: decode, enforce limits, route, run the handler.
async fn process(
    request_ctx: &Ctx,
    limits: &Limits,
    peer: SocketAddr,
    incoming: http::Request<Incoming>,
) -> Answered {
    let (parts, incoming_body) = incoming.into_parts();
    let mut incoming_body = Some(incoming_body);
    let method = parts.method.as_str().to_owned();
    let target = parts
        .uri
        .path_and_query()
        .map_or("/", |each| each.as_str())
        .to_owned();
    // What the log may carry: the target minus every secret query value.
    let early_target = params::redacted_target(&target);
    let fail = |status: u16, client: IpAddr, path: String| Answered {
        ctx: request_ctx.clone(),
        reply: body::plain(status, None),
        method: method.clone(),
        path,
        client,
        user: None,
    };
    let Some(kernel_method) = adapter::to_method(&method) else {
        return fail(405, peer.ip(), early_target);
    };
    let Ok((path, query)) = adapter::path_and_query(&target) else {
        return fail(400, peer.ip(), early_target);
    };
    // Once parsed, the path and its (form-merged) query are logged through
    // the redaction and the caps, so a secret POST field stays out of the
    // line as well.
    let log_target = |path: &str, query: &[(String, String)]| params::logged_target(path, query);
    // The URL is checked before the body is read: an attacker who sends a
    // ten-thousand-pair query should not also make the server read a body.
    if let Some((status, reason)) =
        params::refusal(&query, limits.config.max_param_pairs, ParamsSource::Url)
    {
        refuse(request_ctx, ParamsSource::Url, reason, query.len());
        return fail(status, peer.ip(), log_target(&path, &query));
    }
    let headers = adapter::header_pairs(&parts.headers);
    let forwarded = forwarded_for(&headers);
    let client = client_address(peer.ip(), forwarded.as_deref(), &limits.trusted);
    // Routing comes before a single body byte is buffered: a path nobody
    // owns, or a method that path does not take, is refused without the
    // body ever being read.
    let matched = match request_ctx.kernel().find_route(kernel_method, &path) {
        RouteMatch::Found(matched) => matched,
        RouteMatch::MethodNotAllowed => return fail(405, client, log_target(&path, &query)),
        RouteMatch::NotFound => return fail(404, client, log_target(&path, &query)),
    };
    // A `{name}` capture is never a dot segment: normalization consumed
    // those, and the local check keeps that guarantee from depending on it.
    if matched
        .param()
        .is_some_and(|value| !adapter::param_usable(value))
    {
        return fail(400, client, log_target(&path, &query));
    }
    let content_type = adapter::header_bytes(&headers, "content-type");
    let is_form = adapter::is_form_post(&method, content_type.as_deref());
    // A guarded non-form route runs its check before the body is buffered:
    // the credential rides the headers, and an outsider probing a bogus
    // credential must not pin server memory by pointing a large body at an
    // endpoint that will refuse it anyway. A form POST is the exception -
    // its credential may travel in the body, so the body is read and
    // merged first - and a public route has no check worth running ahead
    // of the body at all.
    let guard_before_body = matches!(matched.guard(), Guard::Requires { .. }) && !is_form;
    let mut request = Request {
        method: kernel_method,
        path,
        param: matched.param().map(str::to_owned),
        query,
        headers,
        peer,
        client,
        body: Body::Empty,
    };
    if !guard_before_body
        && let Some(answered) = read_and_merge(
            request_ctx,
            limits,
            &mut incoming_body,
            is_form,
            &mut request,
        )
        .await
    {
        return answered;
    }
    let logged = log_target(&request.path, &request.query);
    // Deny by default: the kernel runs the route's guard (extract ->
    // authenticate -> permission check) under `guard_timeout` before the
    // handler may run, and the only way to invoke the handler is the
    // `Authorized` the guard hands back.
    let guarded = run_guard(request_ctx, &matched, &request, limits.config.guard_timeout).await;
    finish_guarded(
        guarded,
        request_ctx,
        limits,
        &mut incoming_body,
        request,
        guard_before_body,
        method,
        logged,
        client,
    )
    .await
}

/// Turn the guard check's outcome into the answer: a denial or a timeout is
/// answered as it stands; a passed check buffers a still-unread body under
/// the usual limits and dispatches the authorized handler on the context
/// carrying the principal the guard established.
#[expect(
    clippy::too_many_arguments,
    reason = "the whole remainder of one request, moved forward from `process`"
)]
async fn finish_guarded(
    guarded: Guarded,
    request_ctx: &Ctx,
    limits: &Limits,
    incoming_body: &mut Option<Incoming>,
    mut request: Request,
    guard_before_body: bool,
    method: String,
    logged: String,
    client: IpAddr,
) -> Answered {
    let answered = |reply: HyperReply, user| Answered {
        ctx: request_ctx.clone(),
        reply,
        method: method.clone(),
        path: logged.clone(),
        client,
        user,
    };
    match guarded {
        Guarded::Denied(reply) => answered(reply, None),
        Guarded::TimedOut => {
            let failure = HttpFailure::GuardTimeout;
            answered(
                body::plain(failure.status(), Some(failure.public_message().to_owned())),
                None,
            )
        }
        Guarded::Passed(authorized) => {
            // The check passed a guarded non-form route: only now is the
            // body buffered, under the same cap and read deadline; the
            // query cannot change anymore, so `logged` stays exact.
            if guard_before_body
                && let Some(refused) =
                    read_and_merge(request_ctx, limits, incoming_body, false, &mut request).await
            {
                return refused;
            }
            // The name only ever comes from a passed guard, so it is the
            // kernel-canonical spelling; it is a user name, never a secret.
            let user = authorized.user().map(ToString::to_string);
            // Logging runs on the request context: the guard only ever adds
            // a principal to it, and the reply lines never read one;
            // `authorized.user()` above is how the edge sees authentication.
            let reply = dispatch(
                authorized,
                request,
                limits.config.handler_timeout,
                request_ctx,
            )
            .await;
            answered(reply, user)
        }
    }
}

/// Buffer the request body under the size cap and the read deadline and,
/// for a form POST, merge its pairs into the request's query and re-check
/// the parameter cap there. On refusal the returned `Some` answers the
/// request without the guard or the handler ever running; the refusal's own
/// event is already logged by [`adapter::read_body`] and [`refuse`].
async fn read_and_merge(
    request_ctx: &Ctx,
    limits: &Limits,
    incoming_body: &mut Option<Incoming>,
    is_form: bool,
    request: &mut Request,
) -> Option<Answered> {
    let refused = |reply: HyperReply, path: String, request: &Request| Answered {
        ctx: request_ctx.clone(),
        reply,
        method: request.method.as_str().to_owned(),
        path,
        client: request.client,
        user: None,
    };
    // One request body, read exactly once: the two call sites are
    // exclusive, so a second call would find the slot empty.
    let taken = incoming_body.take()?;
    let body = match adapter::read_body(
        request_ctx,
        taken,
        limits.config.max_body_bytes,
        limits.config.body_read_timeout,
    )
    .await
    {
        Ok(body) => body,
        Err(refused_body) => {
            let path = params::logged_target(&request.path, &request.query);
            let reply = body::plain(
                refused_body.status(),
                refused_body.message().map(str::to_owned),
            );
            return Some(refused(reply, path, request));
        }
    };
    if is_form {
        // The merge parses the form bounded: at most `max_param_pairs + 1`
        // pairs are materialized, so a `a&a&a...` body pays for the cap plus
        // one pair before the refusal below answers 413 (see `crate::form`).
        let url = std::mem::take(&mut request.query);
        request.query = form::merge_form_pairs(url, &body, limits.config.max_param_pairs);
        if let Some((status, reason)) = params::refusal(
            &request.query,
            limits.config.max_param_pairs,
            ParamsSource::Form,
        ) {
            refuse(request_ctx, ParamsSource::Form, reason, request.query.len());
            let path = params::logged_target(&request.path, &request.query);
            return Some(refused(body::plain(status, None), path, request));
        }
    }
    request.body = Body::Bytes(body);
    None
}

/// Run the matched route's guard under `timeout`. The kernel's answer is
/// kept whole: a denial is answered like any handler reply (framing
/// stripped, its status logged in `handle`) without the handler ever
/// seeing the request; a passed check hands back the `Authorized` whose
/// context carries the principal the handler runs as.
async fn run_guard(
    request_ctx: &Ctx,
    matched: &timbre_kernel::Matched,
    request: &Request,
    timeout: Duration,
) -> Guarded {
    match tokio::time::timeout(
        timeout,
        request_ctx
            .kernel()
            .guarded(request_ctx.clone(), matched, request),
    )
    .await
    {
        Err(_elapsed) => {
            let failure = HttpFailure::GuardTimeout;
            log!(
                request_ctx,
                warn,
                GUARD_TIMEOUT,
                code = failure.code().as_str(),
                ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)
            );
            Guarded::TimedOut
        }
        Ok(Err(denied)) => Guarded::Denied(body::to_hyper(denied, request_ctx)),
        Ok(Ok(authorized)) => Guarded::Passed(authorized),
    }
}

/// Run the authorized handler under the deadline and turn its outcome into
/// the reply. The deadline bounds the time until the handler produces its
/// response; a streaming body written after that is not cut short by it, so
/// a long audio download survives however long it takes.
async fn dispatch(
    authorized: timbre_kernel::Authorized,
    request: Request,
    timeout: Duration,
    answered_ctx: &Ctx,
) -> HyperReply {
    match tokio::time::timeout(timeout, authorized.run(request)).await {
        Err(_elapsed) => {
            let failure = HttpFailure::HandlerTimeout;
            log!(
                answered_ctx,
                warn,
                HANDLER_TIMEOUT,
                code = failure.code().as_str(),
                ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)
            );
            body::plain(failure.status(), Some(failure.public_message().to_owned()))
        }
        Ok(Ok(response)) => body::to_hyper(response, answered_ctx),
        Ok(Err(failure)) => {
            let status = failure.status();
            log!(
                answered_ctx,
                warn,
                HANDLER_FAILED,
                code = failure.code().as_str(),
                status = u64::from(status)
            );
            body::plain(status, Some(failure.public_message().to_owned()))
        }
    }
}

/// One line saying which parameter limit refused a request.
fn refuse(ctx: &Ctx, source: ParamsSource, reason: &'static str, pairs: usize) {
    log!(
        ctx,
        warn,
        PARAMS_REFUSED,
        source = source.label(),
        reason = reason,
        pairs = u64::try_from(pairs).unwrap_or(u64::MAX)
    );
}
