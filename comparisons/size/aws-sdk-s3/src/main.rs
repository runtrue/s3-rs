use std::hint::black_box;

use aws_sdk_s3::config::Region;
use aws_sdk_s3::{Client, Config};

fn main() {
    let config = Config::builder()
        .behavior_version_latest()
        .region(Region::new("us-east-1"))
        .build();
    let client = Client::from_conf(config);
    black_box(client);
}
