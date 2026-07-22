//! Upload a one-shot stream with an exact length and SHA-256 digest.

use std::error::Error;
use std::io;

use bytes::Bytes;
use futures_util::stream;
use s3_wire::{ByteStream, DeleteObjectRequest, ObjectKey, PutObjectRequest};
use sha2::{Digest as _, Sha256};

mod common;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = common::client_from_env()?;
    let key = ObjectKey::new("examples/one-shot.txt")?;
    let payload: &'static [u8] = b"one-shot streaming upload";
    let digest: [u8; 32] = Sha256::digest(payload).into();
    let body = stream::once(async move { Ok::<_, io::Error>(Bytes::from_static(payload)) });

    client
        .put_object(PutObjectRequest::new(
            key.clone(),
            ByteStream::from_stream(body, payload.len() as u64, digest),
        ))
        .await?;

    client.delete_object(DeleteObjectRequest::new(key)).await?;
    Ok(())
}
