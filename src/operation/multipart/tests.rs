use super::*;
use crate::operation::ObjectKey;
use crate::stream::ByteStream;

#[test]
fn completion_rejects_missing_duplicate_and_unordered_parts() {
    let key = ObjectKey::new("key").unwrap();
    let upload_id = UploadId::new("upload").unwrap();
    assert!(matches!(
        CompleteMultipartUploadRequest::new(key.clone(), upload_id.clone(), vec![]),
        Err(MultipartError::NoCompletedParts)
    ));

    let two = CompletedPart::new(2, "etag-2").unwrap();
    let duplicate = CompletedPart::new(2, "etag-2-again").unwrap();
    assert!(matches!(
        CompleteMultipartUploadRequest::new(key.clone(), upload_id.clone(), vec![two, duplicate]),
        Err(MultipartError::OutOfOrder { .. })
    ));

    let two = CompletedPart::new(2, "etag-2").unwrap();
    let one = CompletedPart::new(1, "etag-1").unwrap();
    assert!(matches!(
        CompleteMultipartUploadRequest::new(key, upload_id, vec![two, one]),
        Err(MultipartError::OutOfOrder { .. })
    ));
}

#[test]
fn tracked_upload_sorts_concurrent_completion_order() {
    let mut upload = MultipartUpload::new(
        ObjectKey::new("key").unwrap(),
        UploadId::new("upload").unwrap(),
    );
    upload
        .record_part(CompletedPart::new(2, "etag-2").unwrap())
        .unwrap();
    upload
        .record_part(CompletedPart::new(1, "etag-1").unwrap())
        .unwrap();
    assert_eq!(upload.completed_parts()[0].part_number().get(), 1);
    assert_eq!(upload.completed_parts()[1].part_number().get(), 2);
    assert!(matches!(
        upload.record_part(CompletedPart::new(1, "duplicate").unwrap()),
        Err(MultipartError::DuplicatePart(_))
    ));
}

#[test]
fn upload_id_rejects_empty_oversized_and_control_values() {
    assert_eq!(UploadId::new(""), Err(UploadIdError::Empty));
    assert_eq!(
        UploadId::new("x".repeat(UploadId::MAX_LENGTH + 1)),
        Err(UploadIdError::TooLong {
            length: UploadId::MAX_LENGTH + 1,
            maximum: UploadId::MAX_LENGTH,
        })
    );
    assert_eq!(
        UploadId::new("unsafe\nvalue"),
        Err(UploadIdError::AsciiControl { index: 6 })
    );
    assert_eq!(
        UploadId::new("unsafe\u{7f}value"),
        Err(UploadIdError::AsciiControl { index: 6 })
    );
    assert_eq!(
        UploadId::new("valid-opaque-üpload").unwrap().as_str(),
        "valid-opaque-üpload"
    );
}

#[test]
fn upload_id_and_requests_redact_formatting() {
    let upload_id = UploadId::new("do-not-log-this-value").unwrap();
    assert_eq!(upload_id.to_string(), "[REDACTED]");
    assert_eq!(format!("{upload_id:?}"), "UploadId([REDACTED])");
    assert!(!format!("{upload_id:?}").contains(upload_id.as_str()));

    let mut request = UploadPartRequest::new(
        ObjectKey::new("key").unwrap(),
        upload_id,
        PartNumber::new(1).unwrap(),
        ByteStream::from_bytes(b"secret body".as_slice()),
    );
    request.headers.insert(
        "x-amz-server-side-encryption-customer-key",
        "sentinel-sse-c-key".parse().unwrap(),
    );
    let debug = format!("{request:?}");
    assert!(debug.contains("UploadId([REDACTED])"));
    assert!(debug.contains(r#"body: "<stream>""#));
    assert!(debug.contains(r#"headers: "<redacted>""#));
    assert!(!debug.contains("do-not-log-this-value"));
    assert!(!debug.contains("secret body"));
    assert!(!debug.contains("sentinel-sse-c-key"));
}

#[test]
fn request_constructors_retain_validated_upload_ids() {
    let key = ObjectKey::new("key").unwrap();
    let upload_id = UploadId::new("upload").unwrap();
    let part = UploadPartRequest::new(
        key.clone(),
        upload_id.clone(),
        PartNumber::new(1).unwrap(),
        ByteStream::from_bytes(Vec::new()),
    );
    let abort = AbortMultipartUploadRequest::new(key, upload_id);

    assert_eq!(part.upload_id().as_str(), "upload");
    assert_eq!(abort.upload_id().as_str(), "upload");
}

#[test]
fn managed_request_debug_redacts_source_and_metadata_values() {
    let mut request = ManagedMultipartUploadRequest::from_path(
        ObjectKey::new("key").unwrap(),
        "/sensitive/source/path",
    )
    .with_metadata("name", "sensitive-value");
    for headers in [
        &mut request.headers.create,
        &mut request.headers.upload_part,
        &mut request.headers.complete,
        &mut request.headers.abort,
    ] {
        headers.insert("x-secret", "sentinel-header-value".parse().unwrap());
    }
    let rendered = format!("{request:?}");
    assert!(rendered.contains(r#"source: "file""#));
    assert!(rendered.contains("name"));
    assert_eq!(rendered.matches(r#""<redacted>""#).count(), 4);
    assert!(!rendered.contains("/sensitive/source/path"));
    assert!(!rendered.contains("sensitive-value"));
    assert!(!rendered.contains("sentinel-header-value"));
}

#[test]
fn multipart_options_validate_and_derive_the_buffer_bound() {
    assert!(MultipartOptions::new(5 * 1024 * 1024 - 1, 1).is_err());
    assert!(MultipartOptions::new(5 * 1024 * 1024, 0).is_err());
    assert!(MultipartOptions::new(5 * 1024 * 1024, 65).is_err());

    let options = MultipartOptions::new(8 * 1024 * 1024, 4).unwrap();
    assert_eq!(options.maximum_buffered_bytes(), 32 * 1024 * 1024);
    assert!(
        options
            .with_transfer_timeout(std::time::Duration::ZERO)
            .is_err()
    );
    assert!(
        options
            .with_cleanup_timeout(std::time::Duration::ZERO)
            .is_err()
    );
    assert!(
        options
            .with_transfer_timeout(std::time::Duration::MAX)
            .is_err()
    );
    assert!(
        options
            .with_cleanup_timeout(std::time::Duration::MAX)
            .is_err()
    );
}
