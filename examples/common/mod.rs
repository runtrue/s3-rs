use std::env;
use std::error::Error;

use s3_wire::{AddressingStyle, Endpoint, S3Client, S3Config};

pub fn required_env(name: &str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("required environment variable {name} is not set").into())
}

pub fn client_from_env() -> Result<S3Client, Box<dyn Error>> {
    let region = env::var("S3_REGION").unwrap_or_else(|_| "us-east-1".to_owned());
    let endpoint = env::var("S3_ENDPOINT")
        .map_or_else(|_| Endpoint::for_aws_region(&region), Endpoint::new)?;

    let mut builder = S3Config::builder()
        .endpoint(endpoint.clone())
        .region(region)
        .bucket(required_env("S3_BUCKET")?)
        .addressing_style(AddressingStyle::Path);
    if !endpoint.is_https() {
        builder = builder.allow_http_for_local_testing();
    }
    Ok(S3Client::new(builder.build()?)?)
}
