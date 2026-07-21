use super::*;
use crate::operation::{CompleteMultipartUploadRequest, CompletedPart, ObjectKey};
use crate::protocol::ProtocolError;

#[test]
fn completion_serializes_validated_parts_in_order() {
    let request = CompleteMultipartUploadRequest::new(
        ObjectKey::new("key").unwrap(),
        "upload".parse().unwrap(),
        vec![
            CompletedPart::new(1, "\"one&more\"").unwrap(),
            CompletedPart::new(2, "\"two\"").unwrap(),
        ],
    )
    .unwrap();
    let xml =
        String::from_utf8(serialize_complete_multipart_upload(&request, 4_096).unwrap()).unwrap();
    assert!(xml.contains("<PartNumber>1</PartNumber><ETag>\"one&amp;more\"</ETag>"));
    assert!(xml.find("<PartNumber>1").unwrap() < xml.find("<PartNumber>2").unwrap());
}

#[test]
fn detects_embedded_completion_error() {
    let body = b"<Error><Code>InternalError</Code><Message>failed late</Message></Error>";
    let response = parse_complete_multipart_upload(body, 4_096).unwrap();
    match response {
        CompleteMultipartResponse::EmbeddedError(error) => {
            assert_eq!(error.code.as_deref(), Some("InternalError"));
        }
        CompleteMultipartResponse::Complete(_) => panic!("error parsed as success"),
    }
}

#[test]
fn detects_embedded_copy_error() {
    let body = b"<Error><Code>SlowDown</Code><Message>retry later</Message></Error>";
    match parse_copy_object(body, 4_096).unwrap() {
        CopyObjectResponse::EmbeddedError(error) => {
            assert_eq!(error.code.as_deref(), Some("SlowDown"));
        }
        CopyObjectResponse::Complete(_) => panic!("error parsed as copy success"),
    }
}

#[test]
fn parses_multipart_listing() {
    let body = b"<ListMultipartUploadsResult><IsTruncated>false</IsTruncated><Upload><Key>key</Key><UploadId>upload</UploadId><Initiated>2024-03-12T10:15:30Z</Initiated></Upload></ListMultipartUploadsResult>";
    let result = parse_list_multipart_uploads(body, 4_096).unwrap();
    assert_eq!(result.uploads[0].key.as_str(), "key");
    assert_eq!(result.uploads[0].upload_id().as_str(), "upload");
}

#[test]
fn parses_validated_upload_id_markers() {
    let body = b"<ListMultipartUploadsResult><IsTruncated>true</IsTruncated><NextUploadIdMarker>next-upload</NextUploadIdMarker></ListMultipartUploadsResult>";
    let result = parse_list_multipart_uploads(body, 4_096).unwrap();
    assert_eq!(
        result
            .next_upload_id_marker
            .as_ref()
            .map(|upload_id| upload_id.as_str()),
        Some("next-upload")
    );
}

#[test]
fn treats_empty_minio_next_markers_as_absent() {
    let body = b"<ListMultipartUploadsResult><IsTruncated>false</IsTruncated><NextKeyMarker></NextKeyMarker><NextUploadIdMarker></NextUploadIdMarker></ListMultipartUploadsResult>";
    let result = parse_list_multipart_uploads(body, 4_096).unwrap();
    assert_eq!(result.next_key_marker, None);
    assert_eq!(result.next_upload_id_marker, None);
}

#[test]
fn rejects_invalid_upload_ids_from_service_documents() {
    let create = b"<InitiateMultipartUploadResult><Key>key</Key><UploadId>bad&#10;id</UploadId></InitiateMultipartUploadResult>";
    assert!(matches!(
        parse_create_multipart_upload(create, 4_096),
        Err(ProtocolError::InvalidField {
            field: "UploadId",
            ..
        })
    ));

    let listing = b"<ListMultipartUploadsResult><NextUploadIdMarker>bad&#10;id</NextUploadIdMarker></ListMultipartUploadsResult>";
    assert!(matches!(
        parse_list_multipart_uploads(listing, 4_096),
        Err(ProtocolError::InvalidField {
            field: "NextUploadIdMarker",
            ..
        })
    ));
}
