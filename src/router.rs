use crate::config::{RouterConfig, RouterKey};
use crate::http::{FetchResult, auth_get, client as http_client};
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
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

type BoxBody = http_body_util::combinators::BoxBody<Bytes, std::io::Error>;

const ANTHROPIC_OAUTH_BETA: &str = "oauth-2025-04-20";

fn key_eq_bytes(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Clone)]
struct CachedToken {
    access_token: String,
    expires_at_ms: u64,
}

struct RouterState {
    upstream: reqwest::Url,
    broker_url: String,
    consume_key: String,
    keys: Vec<RouterKey>,
    expiry_buffer_ms: u64,
    cache: Mutex<Option<CachedToken>>,
    inbound: reqwest::Client,
    upstream_client: reqwest::Client,
}

impl RouterState {
    fn match_key(&self, provided: &str) -> Option<&str> {
        if provided.is_empty() {
            return None;
        }
        for k in &self.keys {
            if k.disabled {
                continue;
            }
            if key_eq_bytes(provided.as_bytes(), k.key.as_bytes()) {
                return Some(&k.name);
            }
        }
        None
    }
}

impl RouterState {
    async fn token(&self) -> Result<String> {
        let now = now_ms();
        {
            let guard = self.cache.lock().await;
            if let Some(c) = guard.as_ref() {
                if c.expires_at_ms > now + self.expiry_buffer_ms {
                    return Ok(c.access_token.clone());
                }
            }
        }
        let fresh = self.fetch_from_broker().await?;
        let token = fresh.access_token.clone();
        let mut guard = self.cache.lock().await;
        *guard = Some(fresh);
        Ok(token)
    }

    async fn fetch_from_broker(&self) -> Result<CachedToken> {
        let url = format!("{}/consume", self.broker_url.trim_end_matches('/'));
        let body = match auth_get(&self.inbound, &url, &self.consume_key).await? {
            FetchResult::Ok(b) => b,
            FetchResult::Gone(_) => bail!("broker has no token published yet"),
        };
        let envelope: serde_json::Value =
            serde_json::from_str(&body).context("broker response was not valid JSON")?;
        let oauth = envelope
            .get("credentials")
            .and_then(|v| v.get("claudeAiOauth"))
            .context("envelope missing credentials.claudeAiOauth")?;
        let access_token = oauth
            .get("accessToken")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .context("missing claudeAiOauth.accessToken")?
            .to_string();
        let expires_at_ms = oauth
            .get("expiresAt")
            .and_then(|v| v.as_u64())
            .unwrap_or_else(|| now_ms() + 60 * 60 * 1000);
        log(&format!(
            "router: fetched fresh token (expires in {}s)",
            expires_at_ms.saturating_sub(now_ms()) / 1000
        ));
        Ok(CachedToken {
            access_token,
            expires_at_ms,
        })
    }
}

pub async fn run(cfg: RouterConfig) -> Result<()> {
    let upstream = reqwest::Url::parse(&cfg.upstream)
        .with_context(|| format!("invalid upstream: {}", cfg.upstream))?;
    if upstream.cannot_be_a_base() {
        bail!("upstream must be an absolute URL with host: {}", upstream);
    }

    let enabled = cfg.keys.iter().filter(|k| !k.disabled).count();
    if enabled == 0 {
        bail!(
            "no enabled router keys configured — run `claude-courier router-key add <name>` to provision one"
        );
    }

    let state = Arc::new(RouterState {
        upstream: upstream.clone(),
        broker_url: cfg.broker_url.clone(),
        consume_key: cfg.consume_key.clone(),
        keys: cfg.keys.clone(),
        expiry_buffer_ms: cfg.expiry_buffer_secs.saturating_mul(1000),
        cache: Mutex::new(None),
        inbound: http_client()?,
        upstream_client: reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .context("build reqwest client")?,
    });

    let listener = TcpListener::bind(&cfg.listen)
        .await
        .with_context(|| format!("bind {}", cfg.listen))?;
    log(&format!(
        "router listening on {} -> {} ({} key(s) enabled, token from {})",
        cfg.listen, upstream, enabled, cfg.broker_url
    ));

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
    state: Arc<RouterState>,
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

    let provided_xkey = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let provided_courier = req
        .headers()
        .get("x-courier-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let matched = state
        .match_key(provided_xkey)
        .or_else(|| state.match_key(provided_courier));
    let key_name = match matched {
        Some(name) => name.to_string(),
        None => {
            log(&format!(
                "{} key=- {} {} -> 401 unauthorized",
                peer, method, path_and_query
            ));
            return error_response(StatusCode::UNAUTHORIZED, "unauthorized");
        }
    };

    match forward(state, req, &path_and_query).await {
        Ok(resp) => {
            log(&format!(
                "{} key={} {} {} -> {} ({} ms)",
                peer,
                key_name,
                method,
                path_and_query,
                resp.status().as_u16(),
                started.elapsed().as_millis()
            ));
            resp
        }
        Err(e) => {
            log(&format!(
                "{} key={} {} {} -> 502 {} ({} ms)",
                peer,
                key_name,
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
    state: Arc<RouterState>,
    req: Request<Incoming>,
    path_and_query: &str,
) -> Result<Response<BoxBody>> {
    let token = state.token().await?;

    let mut target = state.upstream.clone();
    {
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
        .upstream_client
        .request(parts.method.clone(), target.clone());

    let host_str = target.host_str().unwrap_or("").to_string();
    let mut had_beta = false;
    for (name, value) in parts.headers.iter() {
        let n = name.as_str().to_ascii_lowercase();
        if matches!(
            n.as_str(),
            "host"
                | "connection"
                | "proxy-connection"
                | "content-length"
                | "transfer-encoding"
                | "x-api-key"
                | "x-courier-key"
                | "authorization"
        ) {
            continue;
        }
        if n == "anthropic-beta" {
            had_beta = true;
        }
        upstream_req = upstream_req.header(name.as_str(), value);
    }
    upstream_req = upstream_req.header("host", &host_str);
    upstream_req = upstream_req.header("authorization", format!("Bearer {token}"));
    if !had_beta {
        upstream_req = upstream_req.header("anthropic-beta", ANTHROPIC_OAUTH_BETA);
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
