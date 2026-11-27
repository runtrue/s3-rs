//! Create short-lived presigned GET and PUT URLs.

use std::error::Error;
use std::time::Duration;

use http::{HeaderMap, HeaderValue};
use s3_wire::{ObjectKey, PresignPutRequest};

mod common;

fn deliver_to_authorized_caller(_method: &str, _url: &str) {
    // Send through an authenticated application channel. Do not log the URL.
}

fn deliver_upload_to_authorized_caller(_method: &str, _url: &str, _headers: &HeaderMap) {
    // The URL and its signed headers must reach the uploader together and be
    // sent unchanged. Do not log the URL or the header values.
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = common::client_from_env()?;
    let key = ObjectKey::new("examples/shared.txt")?;
    let expires = Duration::from_secs(5 * 60);

    let get = client.presigned_get(&key, expires).await?;
    deliver_to_authorized_caller("GET", get.expose());

    let put = client.presigned_put(&key, expires).await?;
    deliver_to_authorized_caller("PUT", put.expose());

    // Bind the exact upload headers into the signature: a known media type and
    // byte count. The uploader must send these values with the URL.
    let mut upload_headers = HeaderMap::new();
    upload_headers.insert("content-type", HeaderValue::from_static("image/png"));
    upload_headers.insert("content-length", HeaderValue::from_static("4096"));
    let request = client
        .presigned_put_with_headers(
            PresignPutRequest::new(key, expires).with_headers(upload_headers),
        )
        .await?;
    deliver_upload_to_authorized_caller(
        request.method().as_str(),
        request.url().expose(),
        request.headers(),
    );
    Ok(())
}
