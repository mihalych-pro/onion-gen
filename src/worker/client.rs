//! The worker's side of the conversation.
//!
//! One connection per request. A worker talks to its master about four times a
//! minute, so there is nothing for a pool to amortise, and the absence of a
//! pool removes the case that would otherwise need handling: after the master
//! restarts, a pooled connection is dead and has to be discovered and discarded.
//! Here there is never a connection old enough to be stale.

use std::io::{self, ErrorKind};
use std::net::SocketAddr;

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::proto::*;

/// Where the master is, already broken into what a request needs.
#[derive(Debug, Clone)]
pub struct Client {
    authority: String,
    addr: String,
}

impl Client {
    /// Takes `http://host:port` or plain `host:port`.
    ///
    /// Parsed here rather than with a URL crate: the whole grammar we accept is
    /// a host and a port, and the crate that would parse it brings Unicode
    /// normalisation for internationalised domain names along with it.
    pub fn new(master: &str) -> io::Result<Client> {
        let rest = master
            .trim()
            .strip_prefix("http://")
            .unwrap_or_else(|| master.trim());
        if rest.starts_with("https://") {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "https is not supported; terminate TLS in front of the master",
            ));
        }
        let authority = rest.split('/').next().unwrap_or(rest).to_string();
        if authority.is_empty() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "the master's address is empty",
            ));
        }
        // A bare host means the default port, the same way a browser would.
        let addr = if authority.contains(':') {
            authority.clone()
        } else {
            format!("{authority}:8080")
        };
        Ok(Client { authority, addr })
    }

    pub async fn register(&self, name: &str) -> io::Result<Registered> {
        self.post(
            "/v1/register",
            &Register {
                name: name.to_string(),
            },
        )
        .await
    }

    pub async fn work(
        &self,
        worker: u64,
        blocks_per_second: Option<f64>,
        finished: Option<u64>,
    ) -> io::Result<Lease> {
        self.post(
            "/v1/work",
            &AskWork {
                worker,
                blocks_per_second,
                finished,
            },
        )
        .await
    }

    pub async fn heartbeat(&self, beat: &Heartbeat) -> io::Result<Renewed> {
        self.post("/v1/heartbeat", beat).await
    }

    pub async fn found(&self, find: &Found) -> io::Result<Accepted> {
        self.post("/v1/found", find).await
    }

    async fn post<Q: Serialize, R: DeserializeOwned>(&self, path: &str, body: &Q) -> io::Result<R> {
        let stream = tokio::net::TcpStream::connect(self.resolve()?).await?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(other)?;
        // The connection needs driving while the request is in flight; it ends
        // when the response is done with.
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let payload = serde_json::to_vec(body)?;
        let request = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header("host", &self.authority)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(payload)))
            .map_err(other)?;

        let response = sender.send_request(request).await.map_err(other)?;
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .map_err(other)?
            .to_bytes();
        if !status.is_success() {
            return Err(io::Error::other(format!(
                "the master answered {status}: {}",
                String::from_utf8_lossy(&bytes).trim()
            )));
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(ErrorKind::InvalidData, format!("unreadable answer: {e}")))
    }

    /// Resolved every time rather than once.
    ///
    /// In Kubernetes the master is a Service name whose address changes when
    /// the pod behind it is replaced. Caching the first answer would leave a
    /// worker talking to an address that no longer exists, and it would look
    /// like the master being down.
    fn resolve(&self) -> io::Result<SocketAddr> {
        use std::net::ToSocketAddrs;
        self.addr.to_socket_addrs()?.next().ok_or_else(|| {
            io::Error::new(ErrorKind::NotFound, format!("no address for {}", self.addr))
        })
    }
}

fn other<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_may_be_written_several_ways() {
        for text in ["http://master:8080", "master:8080", "http://master:8080/"] {
            let c = Client::new(text).expect(text);
            assert_eq!(c.addr, "master:8080", "{text}");
            assert_eq!(c.authority, "master:8080", "{text}");
        }
    }

    #[test]
    fn a_bare_host_takes_the_default_port() {
        let c = Client::new("http://master").expect("a bare host");
        assert_eq!(c.addr, "master:8080");
        assert_eq!(
            c.authority, "master",
            "the Host header keeps what was written"
        );
    }

    #[test]
    fn https_is_refused_rather_than_quietly_downgraded() {
        let e = Client::new("https://master:8443").expect_err("https");
        assert!(
            e.to_string().contains("terminate TLS"),
            "the message must say what to do instead: {e}"
        );
    }

    #[test]
    fn an_empty_address_is_refused() {
        assert!(Client::new("http://").is_err());
        assert!(Client::new("   ").is_err());
    }
}
