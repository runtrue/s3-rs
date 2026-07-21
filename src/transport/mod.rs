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
        let mut http = HttpConnector::new();
        http.enforce_http(false);
        http.set_connect_timeout(Some(connect_timeout));
        http.set_nodelay(true);

        // WebPKI roots and rustls' normal hostname verification are always used
        // for HTTPS. Plain HTTP is available only through this explicit switch.
        let builder = HttpsConnectorBuilder::new().with_webpki_roots();
        let connector = if allow_http {
            builder
                .https_or_http()
                .enable_http1()
                .enable_http2()
                .wrap_connector(http)
        } else {
            builder
                .https_only()
                .enable_http1()
                .enable_http2()
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
    use std::sync::Arc;
    use std::time::Duration;

    use http::Request;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    use super::Transport;
    use crate::stream::TransportBody;

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
}
