//! Upload a file through the bounded managed multipart API.

use std::env;
use std::error::Error;

use s3_wire::{DeleteObjectRequest, ManagedMultipartUploadRequest, ObjectKey};

mod common;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = common::client_from_env()?;
    let source = common::required_env("S3_FILE")?;
    let key = ObjectKey::new(
        env::var("S3_KEY").unwrap_or_else(|_| "examples/multipart-upload.bin".to_owned()),
    )?;

    client
        .multipart_upload(
            ManagedMultipartUploadRequest::from_path(key.clone(), source)
                .with_content_type("application/octet-stream"),
        )
        .await?;

    client.delete_object(DeleteObjectRequest::new(key)).await?;
    Ok(())
}
