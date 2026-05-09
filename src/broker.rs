use crate::config::BrokerConfig;
use crate::log::log;
use anyhow::{Context, Result};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

type BoxBody = http_body_util::combinators::BoxBody<Bytes, Infallible>;

struct State {
    queue: Mutex<VecDeque<Bytes>>,
    publish_key: String,
    consume_key: String,
}

pub async fn run(cfg: BrokerConfig) -> Result<()> {
    let state = Arc::new(State {
        queue: Mutex::new(VecDeque::new()),
        publish_key: cfg.publish_key.clone(),
        consume_key: cfg.consume_key.clone(),
    });

    let listener = TcpListener::bind(&cfg.bind)
        .await
        .with_context(|| format!("bind {}", cfg.bind))?;
    log(&format!("broker listening on {}", cfg.bind));

    loop {
        let (stream, peer) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let state = state.clone();
        tokio::spawn(async move {
            let svc = service_fn(move |req| {
                let state = state.clone();
                async move { Ok::<_, Infallible>(handle(state, req, peer.to_string()).await) }
            });
            if let Err(e) = http1::Builder::new().serve_connection(io, svc).await {
                log(&format!("broker connection error: {e}"));
            }
        });
    }
}

async fn handle(state: Arc<State>, req: Request<Incoming>, peer: String) -> Response<BoxBody> {
    let started = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let provided = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let resp = match (&method, path.as_str()) {
        (&Method::POST, "/publish") => {
            if provided != state.publish_key {
                reply(StatusCode::UNAUTHORIZED, "unauthorized")
            } else {
                match req.into_body().collect().await {
                    Ok(collected) => {
                        let bytes = collected.to_bytes();
                        if bytes.is_empty() {
                            reply(StatusCode::BAD_REQUEST, "empty body")
                        } else {
                            state.queue.lock().await.push_back(bytes);
                            reply(StatusCode::ACCEPTED, "ok")
                        }
                    }
                    Err(e) => reply(
                        StatusCode::BAD_REQUEST,
                        &format!("read body failed: {e}"),
                    ),
                }
            }
        }
        (&Method::GET, "/consume") => {
            if provided != state.consume_key {
                reply(StatusCode::UNAUTHORIZED, "unauthorized")
            } else {
                match state.queue.lock().await.pop_front() {
                    Some(bytes) => Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "application/octet-stream")
                        .body(Full::new(bytes).boxed())
                        .unwrap(),
                    None => reply(StatusCode::GONE, "no messages pending"),
                }
            }
        }
        _ => reply(StatusCode::NOT_FOUND, "not found"),
    };

    log(&format!(
        "{} {} {} -> {} ({} ms)",
        peer,
        method,
        path,
        resp.status().as_u16(),
        started.elapsed().as_millis()
    ));
    resp
}

fn reply(status: StatusCode, msg: &str) -> Response<BoxBody> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(msg.to_string())).boxed())
        .unwrap()
}
