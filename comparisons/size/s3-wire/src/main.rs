use std::hint::black_box;

use s3_wire::{S3Client, S3Config};

fn main() {
    let config = S3Config::builder()
        .region("us-east-1")
        .bucket("comparison-bucket")
        .build()
        .expect("static comparison configuration must be valid");
    let client = S3Client::new(config).expect("client construction must succeed");
    black_box(client);
}
