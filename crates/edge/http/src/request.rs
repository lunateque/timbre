//! One request: decode, enforce limits, route, run the handler, answer, log.

use crate::adapter;
use crate::body;
use crate::failures::HttpFailure;
use crate::log_events::{HANDLER_FAILED, HANDLER_TIMEOUT, PARAMS_REFUSED, REQUEST};
use crate::params::{self, ParamsSource};
use crate::proxy::{client_address, forwarded_for};
use crate::serve::{HyperReply, Limits};
use hyper::body::Incoming;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use timbre_kernel::http::{Body, Request};
use timbre_kernel::{Ctx, Failure, RequestId, RouteMatch, log};

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
    log!(
        answered.ctx,
        info,
        REQUEST,
        method = answered.method.as_str(),
        path = answered.path.as_str(),
        status = u64::from(answered.reply.status().as_u16()),
        bytes = body::body_bytes(&answered.reply),
        ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        client = answered.client.to_string()
    );
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
}

/// The request path: decode, enforce limits, route, run the handler.
async fn process(
    request_ctx: &Ctx,
    limits: &Limits,
    peer: SocketAddr,
    incoming: http::Request<Incoming>,
) -> Answered {
    let (parts, incoming_body) = incoming.into_parts();
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
    };
    let fail_body = |refused: adapter::Refused, client: IpAddr, path: String| Answered {
        ctx: request_ctx.clone(),
        reply: body::plain(refused.status(), refused.message().map(str::to_owned)),
        method: method.clone(),
        path,
        client,
    };
    let Some(kernel_method) = adapter::to_method(&method) else {
        return fail(405, peer.ip(), early_target);
    };
    let Ok((path, mut query)) = adapter::path_and_query(&target) else {
        return fail(400, peer.ip(), early_target);
    };
    // Once parsed, the path and its (form-merged) query are logged through
    // the redaction and the caps, so a secret POST field stays out of the
    // line as well.
    let logged = |query: &[(String, String)]| params::logged_target(&path, query);
    // The URL is checked before the body is read: an attacker who sends a
    // ten-thousand-pair query should not also make the server read a body.
    if let Some((status, reason)) =
        params::refusal(&query, limits.config.max_param_pairs, ParamsSource::Url)
    {
        refuse(request_ctx, ParamsSource::Url, reason, query.len());
        return fail(status, peer.ip(), logged(&query));
    }
    let headers = header_pairs(&parts.headers);
    let forwarded = forwarded_for(&headers);
    let client = client_address(peer.ip(), forwarded.as_deref(), &limits.trusted);
    let content_type = adapter::header_bytes(&headers, "content-type");
    let body = match adapter::read_body(
        request_ctx,
        incoming_body,
        limits.config.max_body_bytes,
        limits.config.body_read_timeout,
    )
    .await
    {
        Ok(body) => body,
        Err(refused) => return fail_body(refused, client, logged(&query)),
    };
    if adapter::is_form_post(&method, content_type.as_deref()) {
        query.extend(adapter::form_pairs(&body));
        if let Some((status, reason)) =
            params::refusal(&query, limits.config.max_param_pairs, ParamsSource::Form)
        {
            refuse(request_ctx, ParamsSource::Form, reason, query.len());
            return fail(status, client, logged(&query));
        }
    }
    let logged = logged(&query);
    let request_ctx = request_ctx.with_client(client);
    let (handler, param) = match request_ctx.kernel().find_route(kernel_method, &path) {
        RouteMatch::Found { handler, param } => (handler, param),
        RouteMatch::MethodNotAllowed => return fail(405, client, logged.clone()),
        RouteMatch::NotFound => return fail(404, client, logged.clone()),
    };
    // A `{name}` capture is never a dot segment: normalization consumed
    // those, and the local check keeps that guarantee from depending on it.
    if param
        .as_deref()
        .is_some_and(|value| !adapter::param_usable(value))
    {
        return fail(400, client, logged.clone());
    }
    let request = Request {
        method: kernel_method,
        path: path.clone(),
        param,
        query,
        headers,
        peer,
        client,
        body: Body::Bytes(body),
    };
    let answered_ctx = Ctx::clone(&request_ctx);
    let reply = dispatch(
        handler,
        request_ctx,
        request,
        limits.config.handler_timeout,
        &answered_ctx,
    )
    .await;
    Answered {
        ctx: answered_ctx,
        reply,
        method,
        path: logged,
        client,
    }
}

/// Run the matched handler under the deadline and turn its outcome into the
/// reply. The deadline bounds the time until the handler produces its
/// response; a streaming body written after that is not cut short by it, so
/// a long audio download survives however long it takes.
async fn dispatch(
    handler: timbre_kernel::RouteHandler,
    request_ctx: Ctx,
    request: Request,
    timeout: Duration,
    answered_ctx: &Ctx,
) -> HyperReply {
    match tokio::time::timeout(timeout, handler(request_ctx, request)).await {
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

/// The request headers as the kernel shape sees them: names lowercased,
/// values raw bytes; the values of one name keep their receipt order, but
/// the order across different names is not guaranteed, because hyper groups
/// a header map by name. No logger prints these values; secret ones
/// (`params::SECRET_HEADER_NAMES`) would never reach the log anyway.
fn header_pairs(map: &http::HeaderMap) -> Vec<(String, Vec<u8>)> {
    map.iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                value.as_bytes().to_vec(),
            )
        })
        .collect()
}
