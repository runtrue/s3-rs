//! Criterion baselines for validation, signing, parsing, and bounded streaming.

use std::hint::black_box;
use std::sync::Arc;

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use s3_wire::fuzzing::{
    canonical_header_pairs, canonical_query_pairs, canonical_uri_path, consume_byte_stream,
    parse_error_xml, parse_listing_xml, parse_multipart_xml, signing_output_size,
};
use s3_wire::{Credentials, MultipartOptions, S3Client, S3Config, StaticCredentialsProvider};

fn canonicalization(criterion: &mut Criterion) {
    let path = "/bucket/photos/2024/space + unicode-é.jpg";
    criterion.bench_function("canonical_uri", |bencher| {
        bencher.iter(|| canonical_uri_path(black_box(path)));
    });

    let query = (0..64)
        .map(|index| (format!("field-{index:02}"), format!("value {index}+")))
        .collect::<Vec<_>>();
    criterion.bench_function("canonical_query_64", |bencher| {
        bencher.iter(|| canonical_query_pairs(black_box(&query)));
    });

    let headers = (0..32)
        .map(|index| {
            (
                format!("x-amz-meta-field-{index:02}"),
                " a\t b  c ".to_owned(),
            )
        })
        .chain(std::iter::once((
            "host".to_owned(),
            "bucket.example.test".to_owned(),
        )))
        .collect::<Vec<_>>();
    criterion.bench_function("canonical_headers_33", |bencher| {
        bencher.iter(|| canonical_header_pairs(black_box(&headers)).unwrap());
    });
}

fn signing(criterion: &mut Criterion) {
    criterion.bench_function("sigv4_headers", |bencher| {
        bencher.iter(|| signing_output_size(black_box("/bucket/key with spaces")).unwrap());
    });
}

fn xml_parsing(criterion: &mut Criterion) {
    let error = br#"<Error><Code>SlowDown</Code><Message>retry</Message><RequestId>request</RequestId><HostId>host</HostId><Region>us-east-1</Region></Error>"#;
    let listing = br#"<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>true</IsTruncated><Contents><Key>a%2Fb</Key><LastModified>2024-03-12T10:15:30Z</LastModified><ETag>&quot;etag&quot;</ETag><Size>1048576</Size></Contents><CommonPrefixes><Prefix>a%2F</Prefix></CommonPrefixes><NextContinuationToken>opaque+/=</NextContinuationToken></ListBucketResult>"#;
    let multipart = br#"<ListMultipartUploadsResult><IsTruncated>true</IsTruncated><Upload><Key>a/b</Key><UploadId>opaque+/=</UploadId><Initiated>2024-03-12T10:15:30Z</Initiated></Upload><NextKeyMarker>a/b</NextKeyMarker><NextUploadIdMarker>next+/=</NextUploadIdMarker></ListMultipartUploadsResult>"#;

    criterion.bench_function("parse_s3_error_xml", |bencher| {
        bencher.iter(|| parse_error_xml(black_box(error), 64 * 1024).unwrap());
    });
    criterion.bench_function("parse_list_objects_v2_xml", |bencher| {
        bencher.iter(|| parse_listing_xml(black_box(listing), 64 * 1024).unwrap());
    });
    criterion.bench_function("parse_multipart_xml", |bencher| {
        bencher.iter(|| parse_multipart_xml(black_box(multipart), 64 * 1024));
    });
}

fn client_construction(criterion: &mut Criterion) {
    let credentials = Credentials::new("access", "secret", None).unwrap();
    let config = S3Config::builder()
        .bucket("benchmark-bucket")
        .credentials_provider(Arc::new(StaticCredentialsProvider::new(credentials)))
        .build()
        .unwrap();
    criterion.bench_function("client_construction", |bencher| {
        bencher.iter(|| S3Client::new(black_box(config.clone())).unwrap());
    });
}

fn streaming(criterion: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut group = criterion.benchmark_group("in_memory_stream");
    for size in [4 * 1024, 64 * 1024, 1024 * 1024] {
        let bytes = Bytes::from(vec![0x5a; size]);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &bytes,
            |bencher, bytes| {
                bencher.iter(|| {
                    runtime
                        .block_on(consume_byte_stream(black_box(bytes.clone())))
                        .unwrap()
                });
            },
        );
    }
    group.finish();
}

fn multipart_options(criterion: &mut Criterion) {
    criterion.bench_function("multipart_options", |bencher| {
        bencher.iter(|| {
            MultipartOptions::new(black_box(8 * 1024 * 1024), black_box(4))
                .unwrap()
                .maximum_buffered_bytes()
        });
    });
}

criterion_group!(
    benches,
    canonicalization,
    signing,
    xml_parsing,
    client_construction,
    streaming,
    multipart_options
);
criterion_main!(benches);
