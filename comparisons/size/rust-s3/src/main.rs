use std::future::Future;
use std::hint::black_box;
use std::pin::Pin;

use s3::creds::Credentials;
use s3::{Bucket, Region};

fn main() {
    let credentials = Credentials::new(
        Some("comparison-access-key"),
        Some("comparison-secret-key"),
        None,
        None,
        None,
    )
    .expect("static comparison credentials must be valid");
    let region = Region::Custom {
        region: "us-east-1".to_owned(),
        endpoint: "https://s3.us-east-1.amazonaws.com".to_owned(),
    };
    let bucket = Bucket::new("comparison-bucket", region, credentials)
        .expect("static comparison bucket must be valid");
    retain_poll_implementation(bucket.head_object("comparison-key"));
}

fn retain_poll_implementation<'a, T>(future: impl Future<Output = T> + 'a) {
    let future: Pin<Box<dyn Future<Output = T> + 'a>> = Box::pin(future);
    let _ = black_box(future);
}
