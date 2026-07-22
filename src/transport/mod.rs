//! HTTP transport isolated from signing and the public API.

use std::error::Error;
use std::time::Duration;

use http::header::USER_AGENT;
use http::{HeaderValue, Request, Response};
use hyper::body::Incoming;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::{Client, Error as ClientError};
use hyper_util::rt::TokioExecutor;

use crate::error::{ErrorCategory, RetryClassification, S3Error, TimeoutPhase};
use crate::stream::TransportBody;

type HyperClient = Client<HttpsConnector<HttpConnector>, TransportBody>;

#[derive(Clone)]
pub(crate) struct Transport {
    client: HyperClient,
    user_agent: HeaderValue,
    allow_http: bool,
}

impl Transport {
    pub(crate) fn new(
        connect_timeout: Duration,
        user_agent: &str,
        allow_http: bool,
    ) -> Result<Self, S3Error> {
        // `aws-credentials` can bring AWS-LC into the same process while the
        // lightweight transport deliberately uses rustls' ring provider. When
        // no application-wide provider has been selected, make that choice
        // explicit so enabling the optional bridge cannot make TLS setup panic.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let mut http = HttpConnector::new();
        http.enforce_http(false);
        http.set_connect_timeout(Some(connect_timeout));
        http.set_nodelay(true);

        // WebPKI roots and rustls' normal hostname verification are always used
        // for HTTPS. Plain HTTP is available only through this explicit switch.
        #[cfg(feature = "http2")]
        let connector = if allow_http {
            HttpsConnectorBuilder::new()
                .with_webpki_roots()
                .https_or_http()
                .enable_http1()
                .enable_http2()
                .wrap_connector(http)
        } else {
            HttpsConnectorBuilder::new()
                .with_webpki_roots()
                .https_only()
                .enable_http1()
                .enable_http2()
                .wrap_connector(http)
        };
        #[cfg(not(feature = "http2"))]
        let connector = if allow_http {
            HttpsConnectorBuilder::new()
                .with_webpki_roots()
                .https_or_http()
                .enable_http1()
                .wrap_connector(http)
        } else {
            HttpsConnectorBuilder::new()
                .with_webpki_roots()
                .https_only()
                .enable_http1()
                .wrap_connector(http)
        };
        let client = Client::builder(TokioExecutor::new()).build(connector);
        let user_agent = HeaderValue::from_str(user_agent)
            .map_err(|_| S3Error::configuration("user agent is not a valid HTTP header value"))?;
        Ok(Self {
            client,
            user_agent,
            allow_http,
        })
    }

    pub(crate) async fn send(
        &self,
        mut request: Request<TransportBody>,
        attempt_timeout: Duration,
    ) -> Result<Response<Incoming>, S3Error> {
        let is_https = request.uri().scheme_str() == Some("https");
        if !is_https && (!self.allow_http || request.uri().scheme_str() != Some("http")) {
            return Err(S3Error::configuration(
                "transport refused a request without an explicitly permitted scheme",
            ));
        }
        request
            .headers_mut()
            .entry(USER_AGENT)
            .or_insert_with(|| self.user_agent.clone());

        tokio::time::timeout(attempt_timeout, self.client.request(request))
            .await
            .map_err(|_| {
                S3Error::timeout(
                    TimeoutPhase::Request,
                    "request attempt exceeded its configured timeout",
                )
            })?
            .map_err(|error| classify_client_error(error, is_https))
    }
}

fn classify_client_error(error: ClientError, is_https: bool) -> S3Error {
    if let Some(body_error) = error_chain_s3_error(&error) {
        let preserved = match body_error.timeout_phase() {
            Some(phase) => S3Error::timeout(phase, body_error.message()),
            None => S3Error::new(
                body_error.category(),
                body_error.message(),
                body_error.retry_classification(),
            ),
        };
        return preserved.with_source(error);
    }
    if error.is_connect() && error_chain_has_io_timeout(&error) {
        return S3Error::timeout(
            TimeoutPhase::Connect,
            "connection establishment exceeded its configured timeout",
        )
        .with_source(error);
    }
    if is_https && error.is_connect() && error_chain_has_tls_error(&error) {
        return S3Error::new(
            ErrorCategory::Tls,
            "TLS negotiation or certificate verification failed",
            RetryClassification::Never,
        )
        .with_source(error);
    }
    S3Error::transport(error)
}

pub(crate) fn classify_response_body_error(error: hyper::Error) -> S3Error {
    S3Error::integrity("response body could not be verified because the transport interrupted it")
        .with_source(error)
}

fn error_chain_s3_error<'a>(error: &'a (dyn Error + 'static)) -> Option<&'a S3Error> {
    let mut source = Some(error);
    while let Some(error) = source {
        if let Some(error) = error.downcast_ref::<S3Error>() {
            return Some(error);
        }
        source = error.source();
    }
    None
}

fn error_chain_has_io_timeout(error: &(dyn Error + 'static)) -> bool {
    let mut source = Some(error);
    while let Some(error) = source {
        if error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == std::io::ErrorKind::TimedOut)
        {
            return true;
        }
        source = error.source();
    }
    false
}

fn error_chain_has_tls_error(error: &(dyn Error + 'static)) -> bool {
    let mut source = Some(error);
    while let Some(error) = source {
        if error.downcast_ref::<rustls::Error>().is_some() {
            return true;
        }
        source = error.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Arc;
    use std::time::Duration;

    use bytes::Bytes;
    use http::Request;
    use http_body_util::BodyExt as _;
    use sha2::{Digest as _, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    use super::Transport;
    use crate::stream::{ByteStream, TransportBody};

    #[test]
    fn transport_builds_with_verified_tls_defaults() {
        Transport::new(Duration::from_secs(1), "s3-wire/test", false).expect("transport builds");
    }

    #[tokio::test]
    async fn plain_http_requires_explicit_transport_permission() {
        let transport = Transport::new(Duration::from_secs(1), "s3-wire/test", false).unwrap();
        let request = Request::get("http://127.0.0.1:9/key")
            .body(TransportBody::empty())
            .unwrap();
        let error = transport
            .send(request, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(error.category(), crate::error::ErrorCategory::Configuration);
    }

    #[tokio::test]
    async fn sends_exact_origin_form_without_following_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let request_head = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&request_head);
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut byte = [0_u8; 1];
            while !bytes.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).await.unwrap();
                bytes.push(byte[0]);
            }
            *captured.lock().await = bytes;
            socket
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
        });

        let target = format!("http://{address}/bucket/a/../b//.?partNumber=7&uploadId=a%2Fb");
        let request = Request::get(target).body(TransportBody::empty()).unwrap();
        let transport = Transport::new(Duration::from_secs(1), "s3-wire/test", true).unwrap();
        let response = transport
            .send(request, Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(response.status(), http::StatusCode::FOUND);
        server.await.unwrap();

        let request_head = String::from_utf8(request_head.lock().await.clone()).unwrap();
        let mut lines = request_head.lines();
        assert_eq!(
            lines.next(),
            Some("GET /bucket/a/../b//.?partNumber=7&uploadId=a%2Fb HTTP/1.1")
        );
        assert!(request_head.contains(&format!("host: {address}\r\n")));
    }

    #[tokio::test]
    async fn preserves_upload_integrity_errors_wrapped_by_hyper() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0_u8; 4096];
            while socket.read(&mut bytes).await.unwrap_or(0) != 0 {}
        });

        let source = futures_util::stream::iter([Ok::<_, io::Error>(Bytes::from_static(b"x"))]);
        let prepared = ByteStream::from_stream(source, 1, Sha256::digest(b"wrong").into())
            .prepare()
            .await
            .unwrap();
        let request = Request::put(format!("http://{address}/bucket/key"))
            .body(prepared.request_body().await.unwrap())
            .unwrap();
        let transport = Transport::new(Duration::from_secs(1), "s3-wire/test", true).unwrap();
        let error = transport
            .send(request, Duration::from_secs(2))
            .await
            .unwrap_err();

        assert_eq!(error.category(), crate::error::ErrorCategory::Integrity);
        assert_eq!(
            error.retry_classification(),
            crate::error::RetryClassification::Never
        );
        server.abort();
    }

    #[tokio::test]
    async fn classifies_incomplete_response_body_as_integrity_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut byte = [0_u8; 1];
            while !bytes.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).await.unwrap();
                bytes.push(byte[0]);
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nabc")
                .await
                .unwrap();
        });

        let request = Request::get(format!("http://{address}/bucket/key"))
            .body(TransportBody::empty())
            .unwrap();
        let transport = Transport::new(Duration::from_secs(1), "s3-wire/test", true).unwrap();
        let mut body = transport
            .send(request, Duration::from_secs(2))
            .await
            .unwrap()
            .into_body();
        let mut classified = None;
        while let Some(frame) = body.frame().await {
            if let Err(error) = frame {
                classified = Some(super::classify_response_body_error(error));
                break;
            }
        }

        let error = classified.expect("truncated response produces a body error");
        assert_eq!(error.category(), crate::error::ErrorCategory::Integrity);
        assert_eq!(
            error.retry_classification(),
            crate::error::RetryClassification::Never
        );
        server.await.unwrap();
    }
}
