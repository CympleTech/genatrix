//! Talking to the local inference process over its Unix socket.
//!
//! A deliberately small HTTP/1.1 client: one request per connection, no
//! pooling, no TLS, no proxies, no redirects. It cannot be pointed at a
//! network address even by mistake, because it only knows how to open a
//! Unix socket.

use std::path::{Path, PathBuf};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;

/// A client for one local inference socket.
#[derive(Clone, Debug)]
pub struct LocalClient {
    socket: PathBuf,
}

/// What can go wrong reaching the local process.
#[derive(Debug, thiserror::Error)]
pub enum LocalError {
    /// The socket is not there or could not be connected to.
    #[error("local inference process is not reachable at {path}: {source}")]
    Unreachable {
        /// Socket path.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// The HTTP exchange failed.
    #[error("local inference request failed: {0}")]
    Http(#[from] hyper::Error),
    /// The response could not be read.
    #[error("local inference response was unreadable: {0}")]
    Body(String),
}

/// A response from the local process.
#[derive(Debug)]
pub struct LocalResponse {
    /// HTTP status.
    pub status: u16,
    /// Whole body.
    pub body: Bytes,
}

impl LocalClient {
    /// A client for the given socket.
    #[must_use]
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    /// The socket this client talks to.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// POST a JSON body, optionally with one extra header, and hand back the
    /// response without buffering it, so a streamed answer reaches the caller
    /// as it is produced.
    pub async fn post_streaming(
        &self,
        path: &str,
        body: Bytes,
        extra_header: Option<(&str, &str)>,
    ) -> Result<(u16, Incoming), LocalError> {
        let stream =
            UnixStream::connect(&self.socket)
                .await
                .map_err(|source| LocalError::Unreachable {
                    path: self.socket.clone(),
                    source,
                })?;
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::debug!(error = %e, "local connection closed");
            }
        });
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header("host", "localhost")
            .header("content-type", "application/json");
        if let Some((name, value)) = extra_header {
            builder = builder.header(name, value);
        }
        let req = builder
            .body(Full::new(body))
            .map_err(|e| LocalError::Body(e.to_string()))?;
        let resp = sender.send_request(req).await?;
        let status = resp.status().as_u16();
        Ok((status, resp.into_body()))
    }

    /// POST a JSON body, streaming, with no extra headers.
    pub async fn post_json_streaming(
        &self,
        path: &str,
        body: Bytes,
    ) -> Result<(u16, Incoming), LocalError> {
        self.post_streaming(path, body, None).await
    }

    /// POST a JSON body and read the whole response.
    pub async fn post_json(&self, path: &str, body: Bytes) -> Result<LocalResponse, LocalError> {
        let (status, body) = self.post_json_streaming(path, body).await?;
        let body = body
            .collect()
            .await
            .map_err(|e| LocalError::Body(e.to_string()))?
            .to_bytes();
        Ok(LocalResponse { status, body })
    }

    /// POST a JSON body with one extra header and read the whole response.
    pub async fn post_with_header(
        &self,
        path: &str,
        body: Bytes,
        extra_header: Option<(&str, &str)>,
    ) -> Result<(u16, Bytes), LocalError> {
        let (status, body) = self.post_streaming(path, body, extra_header).await?;
        let body = body
            .collect()
            .await
            .map_err(|e| LocalError::Body(e.to_string()))?
            .to_bytes();
        Ok((status, body))
    }

    /// Whether the local process answers.
    pub async fn healthy(&self) -> bool {
        let Ok(stream) = UnixStream::connect(&self.socket).await else {
            return false;
        };
        let Ok((mut sender, conn)) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await
        else {
            return false;
        };
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let Ok(req) = Request::builder()
            .method("GET")
            .uri("/health")
            .header("host", "localhost")
            .body(Full::new(Bytes::new()))
        else {
            return false;
        };
        sender
            .send_request(req)
            .await
            .is_ok_and(|r| r.status().is_success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_missing_socket_is_reported_as_unreachable() {
        let client = LocalClient::new("/tmp/genatrix-no-such-socket.sock");
        assert!(!client.healthy().await);
        let err = client
            .post_json("/v1/chat/completions", Bytes::from_static(b"{}"))
            .await
            .unwrap_err();
        assert!(matches!(err, LocalError::Unreachable { .. }), "{err:?}");
    }
}
