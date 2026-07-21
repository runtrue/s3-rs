//! Minimal CRUD, range, listing, and conditional-write example.

use std::error::Error;

use s3_wire::{
    ByteRange, ByteStream, DeleteObjectRequest, GetObjectRequest, HeadObjectRequest,
    ListObjectsV2Request, ObjectKey, PutObjectRequest,
};

mod common;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = common::client_from_env()?;
    let key = ObjectKey::new("examples/hello.txt")?;

    let mut put = PutObjectRequest::new(
        key.clone(),
        ByteStream::from_bytes(b"hello from s3-wire".as_slice()),
    );
    put.content_type = Some("text/plain".to_owned());
    put.conditions.if_none_match = Some("*".to_owned());
    client.put_object(put).await?;

    let metadata = client
        .head_object(HeadObjectRequest::new(key.clone()))
        .await?;
    println!("stored {} bytes", metadata.content_length);

    let mut get = GetObjectRequest::new(key.clone());
    get.range = Some(ByteRange::inclusive(0, 4)?);
    let download = client.get_object(get).await?;
    let mut bytes = Vec::new();
    download.body.write_to(&mut bytes).await?;
    println!("range: {}", String::from_utf8(bytes)?);

    let listing = client
        .list_objects_v2(ListObjectsV2Request {
            prefix: Some("examples/".to_owned()),
            ..ListObjectsV2Request::default()
        })
        .await?;
    println!("objects under examples/: {}", listing.objects.len());

    client.delete_object(DeleteObjectRequest::new(key)).await?;
    Ok(())
}
