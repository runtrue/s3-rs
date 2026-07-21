//! Stream an object directly to a file without buffering the complete body.

use std::env;
use std::error::Error;

use s3_wire::{GetObjectRequest, ObjectKey};

mod common;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = common::client_from_env()?;
    let key = ObjectKey::new(env::var("S3_KEY").unwrap_or_else(|_| "archive.tar".to_owned()))?;
    let destination =
        env::var("S3_DESTINATION").unwrap_or_else(|_| "downloaded-archive.tar".to_owned());

    let response = client.get_object(GetObjectRequest::new(key)).await?;
    let mut file = tokio::fs::File::create(&destination).await?;
    let written = response.body.write_to(&mut file).await?;
    println!("wrote {written} bytes to {destination}");
    Ok(())
}
