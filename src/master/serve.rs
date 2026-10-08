//! The master over HTTP.
//!
//! A thin layer: every rule lives in [`super::Master`], which is tested without
//! a socket in sight. What is here is routing, JSON in and out, and the one
//! decision the transport does own — that an unreadable request is refused
//! rather than guessed at.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::Master;
use crate::proto::*;

/// The master, shared across connections.
///
/// A plain mutex rather than an async one: no handler awaits while holding it,
/// so it is never held across a suspension point. The one slow thing under the
/// lock is writing a find to disk, measured at some five milliseconds; finds
/// are rare enough that serialising other requests behind one is cheaper than
/// the machinery to avoid it.
pub type Shared = Arc<Mutex<Master>>;

/// Serves until the process is stopped.
pub async fn serve(addr: SocketAddr, master: Shared) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    eprintln!("onion-gen: master listening on {bound}");

    // Finds go down a batch at a time, and a batch that is not filled has to be
    // written anyway. A fleet that goes quiet would otherwise leave its last
    // finds in memory until the master stopped.
    {
        let master = Arc::clone(&master);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
            loop {
                tick.tick().await;
                let outcome = master
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .flush_if_due();
                if let Err(e) = outcome {
                    eprintln!("onion-gen: could not write the keys: {e}");
                }
            }
        });
    }

    loop {
        let (stream, _) = listener.accept().await?;
        let master = Arc::clone(&master);
        tokio::spawn(async move {
            let service = service_fn(move |req| route(req, Arc::clone(&master)));
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                // A worker that hangs up mid-request is ordinary, not news.
                let _ = e;
            }
        });
    }
}

async fn route(
    req: Request<Incoming>,
    master: Shared,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    Ok(match (&method, path.as_str()) {
        (&Method::POST, "/v1/register") => {
            handle(req, master, |m, ask: Register, now| {
                Ok(m.register(&ask, now))
            })
            .await
        }
        (&Method::POST, "/v1/work") => {
            handle(req, master, |m, ask: AskWork, now| Ok(m.work(&ask, now))).await
        }
        (&Method::POST, "/v1/heartbeat") => {
            handle(req, master, |m, beat: Heartbeat, now| {
                Ok(m.heartbeat(&beat, now))
            })
            .await
        }
        (&Method::POST, "/v1/found") => {
            handle(req, master, |m, report: Found, now| m.found(&report, now)).await
        }
        (&Method::GET, "/metrics") => {
            let view = master
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .view(Instant::now());
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
                .body(Full::new(Bytes::from(super::metrics::render(&view))))
                .expect("a valid response")
        }
        (&Method::GET, "/v1/status") => {
            let view = master
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .view(Instant::now());
            json(StatusCode::OK, &view)
        }
        _ => text(StatusCode::NOT_FOUND, "no such endpoint\n"),
    })
}

/// Reads a JSON body, hands it to the state machine, writes the answer back.
async fn handle<Q, R, F>(req: Request<Incoming>, master: Shared, f: F) -> Response<Full<Bytes>>
where
    Q: DeserializeOwned,
    R: Serialize,
    F: FnOnce(&mut Master, Q, Instant) -> std::io::Result<R>,
{
    let body = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return text(StatusCode::BAD_REQUEST, "could not read the request body\n"),
    };
    let ask: Q = match serde_json::from_slice(&body) {
        Ok(ask) => ask,
        // Refused rather than guessed at: a master that invents a plausible
        // request from a malformed one hands out work nobody asked for.
        Err(e) => return text(StatusCode::BAD_REQUEST, &format!("{e}\n")),
    };
    let answer = {
        let mut guard = master.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard, ask, Instant::now())
    };
    match answer {
        Ok(value) => json(StatusCode::OK, &value),
        // The one failure the master itself can have: it could not write a
        // find. Saying so lets the worker keep it buffered and try again,
        // which is exactly what it should do.
        Err(e) => text(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}\n")),
    }
}

fn json<T: Serialize>(status: StatusCode, value: &T) -> Response<Full<Bytes>> {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body)))
        .expect("a valid response")
}

fn text(status: StatusCode, message: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(message.to_string())))
        .expect("a valid response")
}
