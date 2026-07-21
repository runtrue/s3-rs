//! Minimal CRUD, range, listing, and conditional-write example.

use std::env;
use std::error::Error;
use std::sync::Arc;

use s3_wire::{
    AddressingStyle, ByteRange, ByteStream, Credentials, DeleteObjectRequest, Endpoint,
    GetObjectRequest, HeadObjectRequest, ListObjectsV2Request, ObjectKey, PutObjectRequest,
    S3Client, S3Config, StaticCredentialsProvider,
};

fn required_env(name: &str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("required environment variable {name} is not set").into())
}

fn client_from_env() -> Result<S3Client, Box<dyn Error>> {
    let region = env::var("S3_REGION").unwrap_or_else(|_| "us-east-1".to_owned());
    let endpoint = env::var("S3_ENDPOINT")
        .map_or_else(|_| Endpoint::for_aws_region(&region), Endpoint::new)?;
    let credentials = Credentials::new(
        required_env("S3_ACCESS_KEY_ID")?,
        required_env("S3_SECRET_ACCESS_KEY")?,
        env::var("S3_SESSION_TOKEN").ok(),
    )?;

    let mut builder = S3Config::builder()
        .endpoint(endpoint.clone())
        .region(region)
        .bucket(required_env("S3_BUCKET")?)
        .addressing_style(AddressingStyle::Path)
        .credentials_provider(Arc::new(StaticCredentialsProvider::new(credentials)));
    if !endpoint.is_https() {
        builder = builder.allow_http_for_local_testing();
    }
    Ok(S3Client::new(builder.build()?)?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = client_from_env()?;
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
