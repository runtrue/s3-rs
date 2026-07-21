use crate::operation::{
    Checksum, CreateMultipartUploadRequest, ListMultipartUploadsRequest, ObjectKey, PageSize,
    UploadId,
};
use crate::signing::{QueryParam, canonical_query};

use super::headers::{checksum_headers, create_headers};
use super::query::{list_query, upload_part_query, upload_query};

fn encoded(query: &[(String, String)]) -> String {
    canonical_query(
        &query
            .iter()
            .map(|(name, value)| QueryParam::new(name, value))
            .collect::<Vec<_>>(),
    )
}

#[test]
fn upload_queries_encode_opaque_upload_ids_exactly_once() {
    let upload_id = UploadId::new("id+/= value").unwrap();
    assert_eq!(
        encoded(&upload_part_query(7, &upload_id)),
        "partNumber=7&uploadId=id%2B%2F%3D%20value"
    );
    assert_eq!(
        encoded(&upload_query(&upload_id)),
        "uploadId=id%2B%2F%3D%20value"
    );
}

#[test]
fn list_query_contains_paired_resume_markers_and_bounds() {
    let request = ListMultipartUploadsRequest {
        prefix: Some("a b/".to_owned()),
        delimiter: Some("/".to_owned()),
        key_marker: Some("last+key".to_owned()),
        upload_id_marker: Some(UploadId::new("upload/id=").unwrap()),
        max_uploads: PageSize::new(17).unwrap(),
    };
    assert_eq!(
        encoded(&list_query(&request).unwrap()),
        "delimiter=%2F&key-marker=last%2Bkey&max-uploads=17&prefix=a%20b%2F&upload-id-marker=upload%2Fid%3D&uploads="
    );

    let invalid = ListMultipartUploadsRequest {
        upload_id_marker: Some(UploadId::new("upload-id").unwrap()),
        ..ListMultipartUploadsRequest::default()
    };
    assert!(list_query(&invalid).is_err());
}

#[test]
fn request_header_state_is_validated_before_transport() {
    let mut request = CreateMultipartUploadRequest::new(ObjectKey::new("key").unwrap());
    request
        .user_metadata
        .insert(String::new(), "value".to_owned());
    assert!(create_headers(&request).is_err());

    let checksum = Checksum {
        sha256: Some("not-base64".to_owned()),
        ..Checksum::default()
    };
    assert!(checksum_headers(&checksum).is_err());
    let checksum = Checksum {
        crc32: Some("AAAAAA==".to_owned()),
        ..Checksum::default()
    };
    assert_eq!(
        checksum_headers(&checksum)
            .unwrap()
            .get("x-amz-checksum-crc32")
            .unwrap(),
        "AAAAAA=="
    );
}
