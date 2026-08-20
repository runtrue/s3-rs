use std::collections::BTreeMap;
use std::future::pending;
use std::sync::Arc;

use http::StatusCode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::{JoinHandle, JoinSet};

const MAX_REQUEST_HEAD: usize = 64 * 1024;
const MAX_REQUEST_BODY: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct CapturedRequest {
    pub method: String,
    pub target: String,
    pub headers: BTreeMap<String, Vec<String>>,
    pub body: Vec<u8>,
}

impl CapturedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.header_values(name).first().map(String::as_str)
    }

    pub fn header_values(&self, name: &str) -> &[String] {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

pub enum Reply {
    Full {
        status: StatusCode,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    Truncated {
        status: StatusCode,
        declared_length: usize,
        body: Vec<u8>,
    },
    Idle {
        status: StatusCode,
        declared_length: usize,
    },
    Stall,
}

impl Reply {
    pub fn empty(status: StatusCode) -> Self {
        Self::Full {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn xml(status: StatusCode, body: impl Into<Vec<u8>>) -> Self {
        Self::Full {
            status,
            headers: vec![("content-type".to_owned(), "application/xml".to_owned())],
            body: body.into(),
        }
    }
}

pub struct MockServer {
    endpoint: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    task: JoinHandle<()>,
}

impl MockServer {
    pub async fn start<F>(handler: F) -> Self
    where
        F: Fn(usize, &CapturedRequest) -> Reply + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let address = listener.local_addr().expect("read mock server address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let task_requests = Arc::clone(&requests);
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((mut socket, _)) = accepted else {
                            break;
                        };
                        let requests = Arc::clone(&task_requests);
                        let handler = Arc::clone(&handler);
                        connections.spawn(async move {
                            let Some(request) = read_request(&mut socket).await else {
                                return;
                            };
                            let attempt = {
                                let mut captured = requests.lock().await;
                                captured.push(request.clone());
                                captured.len()
                            };
                            write_reply(&mut socket, handler(attempt, &request)).await;
                        });
                    }
                    completed = connections.join_next(), if !connections.is_empty() => {
                        let _ = completed;
                    }
                }
            }
        });
        Self {
            endpoint: format!("http://{address}"),
            requests,
            task,
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().await.clone()
    }

    pub async fn request_count(&self) -> usize {
        self.requests.lock().await.len()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_request(socket: &mut TcpStream) -> Option<CapturedRequest> {
    let mut bytes = Vec::with_capacity(1024);
    let header_end = loop {
        if bytes.len() > MAX_REQUEST_HEAD {
            return None;
        }
        let mut chunk = [0_u8; 4096];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = find_subslice(&bytes, b"\r\n\r\n") {
            break index + 4;
        }
    };

    let head = std::str::from_utf8(&bytes[..header_end]).ok()?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next()?.to_owned();
    let target = request_parts.next()?.to_owned();
    let _version = request_parts.next()?;
    let mut headers = BTreeMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':')?;
        headers
            .entry(name.trim().to_ascii_lowercase())
            .or_insert_with(Vec::new)
            .push(value.trim().to_owned());
    }
    let content_length = headers
        .get("content-length")
        .and_then(|values| values.first())
        .map_or(Some(0), |value| value.parse::<usize>().ok())?;
    if content_length > MAX_REQUEST_BODY {
        return None;
    }
    while bytes.len().saturating_sub(header_end) < content_length {
        let mut chunk = [0_u8; 4096];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let body = bytes[header_end..header_end + content_length].to_vec();
    Some(CapturedRequest {
        method,
        target,
        headers,
        body,
    })
}

async fn write_reply(socket: &mut TcpStream, reply: Reply) {
    match reply {
        Reply::Full {
            status,
            mut headers,
            body,
        } => {
            if !headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            {
                headers.push(("content-length".to_owned(), body.len().to_string()));
            }
            write_head(socket, status, &headers).await;
            let _ = socket.write_all(&body).await;
            let _ = socket.shutdown().await;
        }
        Reply::Truncated {
            status,
            declared_length,
            body,
        } => {
            write_head(
                socket,
                status,
                &[("content-length".to_owned(), declared_length.to_string())],
            )
            .await;
            let _ = socket.write_all(&body).await;
        }
        Reply::Idle {
            status,
            declared_length,
        } => {
            write_head(
                socket,
                status,
                &[("content-length".to_owned(), declared_length.to_string())],
            )
            .await;
            let _ = socket.flush().await;
            pending::<()>().await;
        }
        Reply::Stall => pending::<()>().await,
    }
}

async fn write_head(socket: &mut TcpStream, status: StatusCode, headers: &[(String, String)]) {
    let reason = status.canonical_reason().unwrap_or("Unknown");
    let mut head = format!("HTTP/1.1 {} {reason}\r\n", status.as_u16());
    for (name, value) in headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("connection: close\r\n\r\n");
    let _ = socket.write_all(head.as_bytes()).await;
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
