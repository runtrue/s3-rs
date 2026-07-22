//! Create short-lived presigned GET and PUT URLs.

use std::error::Error;
use std::time::Duration;

use s3_wire::{ObjectKey, PresignedUrl};

mod common;

fn deliver_to_authorized_caller(_method: &str, _url: &str) {
    // Send through an authenticated application channel. Do not log the URL.
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = common::client_from_env()?;
    let key = ObjectKey::new("examples/shared.txt")?;
    let expires = Duration::from_secs(5 * 60);

    let get: PresignedUrl = client.presigned_get(&key, expires).await?;
    let put: PresignedUrl = client.presigned_put(&key, expires).await?;
    deliver_to_authorized_caller("GET", get.expose());
    deliver_to_authorized_caller("PUT", put.expose());
    Ok(())
}
