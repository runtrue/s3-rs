use std::future::Future;
use std::hint::black_box;
use std::pin::Pin;

use s3_wire::{HeadObjectRequest, ObjectKey, S3Client, S3Config};

fn main() {
    let config = S3Config::builder()
        .region("us-east-1")
        .bucket("comparison-bucket")
        .build()
        .expect("static comparison configuration must be valid");
    let client = S3Client::new(config).expect("client construction must succeed");
    retain_poll_implementation(client.head_object(HeadObjectRequest::new(
        ObjectKey::new("comparison-key").expect("static comparison key must be valid"),
    )));
}

fn retain_poll_implementation<'a, T>(future: impl Future<Output = T> + 'a) {
    let future: Pin<Box<dyn Future<Output = T> + 'a>> = Box::pin(future);
    let _ = black_box(future);
}
