use std::future::Future;
use std::hint::black_box;
use std::pin::Pin;

use aws_sdk_s3::config::Region;
use aws_sdk_s3::{Client, Config};

fn main() {
    let config = Config::builder()
        .behavior_version_latest()
        .region(Region::new("us-east-1"))
        .build();
    let client = Client::from_conf(config);
    retain_poll_implementation(
        client
            .head_object()
            .bucket("comparison-bucket")
            .key("comparison-key")
            .send(),
    );
}

fn retain_poll_implementation<'a, T>(future: impl Future<Output = T> + 'a) {
    let future: Pin<Box<dyn Future<Output = T> + 'a>> = Box::pin(future);
    let _ = black_box(future);
}
