//! The worker's own endpoint: its metrics and its two probes.
//!
//! Read-only by construction. There is no handler here that changes anything —
//! no way to hand out a lease, cancel one, or make the worker write something —
//! which is what keeps the property that coordination only ever travels from
//! the worker to its master. An orchestrator asking whether a pod is alive is
//! not the master giving it orders.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use prometheus_client::encoding::{text::encode, EncodeLabelSet};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;

use crate::proto::safe_label;
use crate::worker::Health;

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct WorkerLabel {
    worker: String,
}

/// Serves the worker's endpoint until the process ends.
pub async fn serve(addr: SocketAddr, name: String, health: Arc<Health>) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("onion-gen: worker endpoint on {}", listener.local_addr()?);
    loop {
        let (stream, _) = listener.accept().await?;
        let health = Arc::clone(&health);
        let name = name.clone();
        tokio::spawn(async move {
            let service = service_fn(move |req| route(req, name.clone(), Arc::clone(&health)));
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

async fn route(
    req: Request<Incoming>,
    name: String,
    health: Arc<Health>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    // Only GET. Not a nicety: a worker that answered a POST would be taking an
    // instruction, and the whole arrangement rests on it never doing that.
    if req.method() != Method::GET {
        return Ok(reply(
            StatusCode::METHOD_NOT_ALLOWED,
            "this endpoint only reads\n",
        ));
    }
    Ok(match req.uri().path() {
        "/metrics" => Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
            .body(Full::new(Bytes::from(render(&name, &health))))
            .expect("a valid response"),
        // Alive as long as the search is moving. Deliberately blind to the
        // master: see the note in `health`.
        "/health" => {
            if health.alive() {
                reply(StatusCode::OK, "searching\n")
            } else {
                reply(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "the search has not moved\n",
                )
            }
        }
        "/ready" => {
            if health.ready() {
                reply(StatusCode::OK, "holding work\n")
            } else {
                reply(StatusCode::SERVICE_UNAVAILABLE, "no work from the master\n")
            }
        }
        _ => reply(StatusCode::NOT_FOUND, "no such endpoint\n"),
    })
}

/// What only the worker knows about itself.
pub fn render(name: &str, health: &Health) -> String {
    let mut registry = Registry::default();
    let label = WorkerLabel {
        worker: safe_label(name),
    };

    let rate = Family::<WorkerLabel, Gauge<f64, std::sync::atomic::AtomicU64>>::default();
    registry.register(
        "onion_gen_worker_blocks_per_second",
        "Blocks per second this worker last measured on itself",
        rate.clone(),
    );
    rate.get_or_create(&label).set(health.rate_now());

    let blocks = Counter::<u64>::default();
    registry.register(
        "onion_gen_worker_blocks",
        "Blocks this worker has finished since it started",
        blocks.clone(),
    );
    blocks.inc_by(health.blocks());

    // The number that says the master has been away: finds waiting to be handed
    // over. It is known only here, and it is the one worth alerting on.
    let buffered = Family::<WorkerLabel, Gauge>::default();
    registry.register(
        "onion_gen_worker_unsent_finds",
        "Finds this worker is holding because the master could not take them",
        buffered.clone(),
    );
    buffered
        .get_or_create(&label)
        .set(health.buffered_count() as i64);

    let up = Family::<WorkerLabel, Gauge>::default();
    registry.register(
        "onion_gen_worker_ready",
        "Whether this worker currently holds work from its master",
        up.clone(),
    );
    up.get_or_create(&label).set(i64::from(health.ready()));

    let mut out = String::new();
    encode(&mut out, &registry).expect("writing to a String cannot fail");
    out
}

fn reply(status: StatusCode, message: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(message.to_string())))
        .expect("a valid response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_reports_what_only_it_knows() {
        let health = Health::default();
        health.rate(439.7);
        health.buffered(3);
        health.advanced(12);
        let text = render("node-1", &health);
        assert!(
            text.contains("onion_gen_worker_unsent_finds{worker=\"node-1\"} 3"),
            "{text}"
        );
        assert!(text.contains("onion_gen_worker_blocks_total 12"), "{text}");
        assert!(
            text.contains("onion_gen_worker_blocks_per_second{worker=\"node-1\"} 439.7"),
            "{text}"
        );
    }

    #[test]
    fn a_hostile_name_cannot_break_the_workers_exposition_either() {
        let health = Health::default();
        let text = render("ev\"il\\node", &health);
        assert!(text.contains("worker=\"ev_il_node\""), "{text}");
        for line in text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            assert_eq!(line.matches('"').count() % 2, 0, "{line}");
        }
    }

    #[test]
    fn readiness_shows_in_the_metrics_too() {
        let health = Health::default();
        assert!(render("n", &health).contains("onion_gen_worker_ready{worker=\"n\"} 0"));
        health.registered(true);
        health.holding(true);
        assert!(render("n", &health).contains("onion_gen_worker_ready{worker=\"n\"} 1"));
    }
}
