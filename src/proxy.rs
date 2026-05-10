use crate::config::ProxyConfig;
use crate::log::log;
use anyhow::{Context, Result, bail};
use futures::TryStreamExt;
use http_body_util::{BodyExt, StreamBody};
use hyper::body::{Bytes, Frame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;

type BoxBody = http_body_util::combinators::BoxBody<Bytes, std::io::Error>;

fn key_eq_bytes(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

pub fn resolve_upstream(spec: &str) -> Result<reqwest::Url> {
    let url = match spec.to_ascii_lowercase().as_str() {
        "anthropic" | "claude" => "https://api.anthropic.com",
        "openai" => "https://api.openai.com",
        "gemini" | "google" => "https://generativelanguage.googleapis.com",
        _ => spec,
    };
    reqwest::Url::parse(url).with_context(|| format!("invalid upstream: {spec}"))
}

struct ProxyState {
    upstream: reqwest::Url,
    inject_api_key: Option<String>,
    listen_key: Option<String>,
    client: reqwest::Client,
}

pub async fn run(cfg: ProxyConfig) -> Result<()> {
    let upstream = resolve_upstream(&cfg.upstream)?;
    if upstream.cannot_be_a_base() {
        bail!("upstream must be an absolute URL with host: {}", upstream);
    }
    let state = Arc::new(ProxyState {
        upstream: upstream.clone(),
        inject_api_key: cfg.inject_api_key.clone(),
        listen_key: cfg.listen_key.clone(),
        client: reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .context("build reqwest client")?,
    });

    let listener = TcpListener::bind(&cfg.listen)
        .await
        .with_context(|| format!("bind {}", cfg.listen))?;
    log(&format!("proxy listening on {} -> {}", cfg.listen, upstream));

    loop {
        let (stream, peer) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let state = state.clone();
        tokio::spawn(async move {
            let svc = service_fn(move |req| {
                let state = state.clone();
                async move { Ok::<_, Infallible>(handle(state, req, peer.to_string()).await) }
            });
            if let Err(e) = http1::Builder::new()
                .preserve_header_case(true)
                .title_case_headers(false)
                .serve_connection(io, svc)
                .await
            {
                log(&format!("connection error: {e}"));
            }
        });
    }
}

async fn handle(
    state: Arc<ProxyState>,
    req: Request<Incoming>,
    peer: String,
) -> Response<BoxBody> {
    let started = Instant::now();
    let method = req.method().clone();
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());

    if let Some(expected) = &state.listen_key {
        let provided = req
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !key_eq_bytes(provided.as_bytes(), expected.as_bytes()) {
            log(&format!(
                "{} {} {} -> 401 unauthorized",
                peer, method, path_and_query
            ));
            return error_response(StatusCode::UNAUTHORIZED, "unauthorized");
        }
    }

    match forward(state, req, &path_and_query).await {
        Ok(resp) => {
            log(&format!(
                "{} {} {} -> {} ({} ms)",
                peer,
                method,
                path_and_query,
                resp.status().as_u16(),
                started.elapsed().as_millis()
            ));
            resp
        }
        Err(e) => {
            log(&format!(
                "{} {} {} -> 502 {} ({} ms)",
                peer,
                method,
                path_and_query,
                e,
                started.elapsed().as_millis()
            ));
            error_response(StatusCode::BAD_GATEWAY, &format!("upstream error: {e}"))
        }
    }
}

async fn forward(
    state: Arc<ProxyState>,
    req: Request<Incoming>,
    path_and_query: &str,
) -> Result<Response<BoxBody>> {
    let mut target = state.upstream.clone();
    {
        // Append the incoming path to the upstream's existing path (rare but supported).
        let base_path = target.path().trim_end_matches('/').to_string();
        let incoming = path_and_query.trim_start_matches('/');
        let split: Vec<&str> = incoming.splitn(2, '?').collect();
        let new_path = if base_path.is_empty() {
            format!("/{}", split[0])
        } else {
            format!("{}/{}", base_path, split[0])
        };
        target.set_path(&new_path);
        target.set_query(split.get(1).copied());
    }

    let (parts, body) = req.into_parts();
    let body_bytes = body.collect().await.context("read request body")?.to_bytes();

    let mut upstream_req = state
        .client
        .request(parts.method.clone(), target.clone());

    let host_str = target.host_str().unwrap_or("").to_string();
    let mut forwarded_auth = false;
    let mut forwarded_xkey = false;
    for (name, value) in parts.headers.iter() {
        let n = name.as_str().to_ascii_lowercase();
        if matches!(
            n.as_str(),
            "host" | "connection" | "proxy-connection" | "content-length" | "transfer-encoding"
        ) {
            continue;
        }
        // Don't leak the listen-side auth key to the upstream — it's a local
        // secret, not the upstream's API key.
        if n == "x-api-key"
            && state
                .listen_key
                .as_deref()
                .map(|k| key_eq_bytes(value.as_bytes(), k.as_bytes()))
                .unwrap_or(false)
        {
            continue;
        }
        if n == "authorization" {
            forwarded_auth = true;
        }
        if n == "x-api-key" {
            forwarded_xkey = true;
        }
        upstream_req = upstream_req.header(name.as_str(), value);
    }
    upstream_req = upstream_req.header("host", &host_str);

    if let Some(key) = &state.inject_api_key {
        // WARNING: never enable inject_api_key without listen_key. An open relay
        // that injects the operator's key lets anyone drain the operator's
        // upstream credits.
        if !forwarded_auth && !forwarded_xkey {
            // Anthropic uses x-api-key; OpenAI/Gemini use Authorization. Default
            // to x-api-key when upstream is anthropic, else Authorization Bearer.
            if host_str.contains("anthropic") {
                upstream_req = upstream_req.header("x-api-key", key);
            } else {
                upstream_req =
                    upstream_req.header("authorization", format!("Bearer {key}"));
            }
        }
    }

    if !body_bytes.is_empty() {
        upstream_req = upstream_req.body(body_bytes);
    }

    let resp = upstream_req.send().await.context("send to upstream")?;
    let status = resp.status();
    let mut builder = Response::builder().status(status);
    for (name, value) in resp.headers().iter() {
        let n = name.as_str().to_ascii_lowercase();
        if matches!(
            n.as_str(),
            "connection" | "transfer-encoding" | "content-length"
        ) {
            continue;
        }
        builder = builder.header(name.as_str(), value);
    }

    let stream = resp
        .bytes_stream()
        .map_ok(Frame::data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
    let body = StreamBody::new(stream).boxed();

    builder.body(body).context("build response")
}

fn error_response(status: StatusCode, msg: &str) -> Response<BoxBody> {
    let body = http_body_util::Full::new(Bytes::from(msg.to_string()))
        .map_err(|e| match e {})
        .boxed();
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(body)
        .unwrap()
}
