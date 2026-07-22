//! Copy an object, then remove the source and destination in one batch.

use std::error::Error;

use s3_wire::{
    ByteStream, CopyObjectRequest, CopySource, DeleteObjectRequest, DeleteObjectsRequest,
    ObjectKey, PutObjectRequest,
};

mod common;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = common::client_from_env()?;
    let bucket = common::required_env("S3_BUCKET")?;
    let source = ObjectKey::new("examples/copy-source.txt")?;
    let destination = ObjectKey::new("examples/copy-destination.txt")?;

    client
        .put_object(PutObjectRequest::new(
            source.clone(),
            ByteStream::from_bytes(b"copy me".as_slice()),
        ))
        .await?;

    client
        .copy_object(CopyObjectRequest::new(
            CopySource {
                bucket,
                key: source.clone(),
                version_id: None,
            },
            destination.clone(),
        ))
        .await?;

    let deleted = client
        .delete_objects(DeleteObjectsRequest::new(vec![
            DeleteObjectRequest::new(source),
            DeleteObjectRequest::new(destination),
        ])?)
        .await?;
    if !deleted.errors.is_empty() {
        return Err(format!("{} objects could not be deleted", deleted.errors.len()).into());
    }
    Ok(())
}
