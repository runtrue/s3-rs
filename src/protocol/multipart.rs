use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::operation::{
    Checksum, CompleteMultipartUploadOutput, CompleteMultipartUploadRequest, CopyObjectOutput,
    CreateMultipartUploadOutput, ListMultipartUploadsOutput, MultipartUploadEntry, ObjectKey,
    UploadId,
};

use super::{
    error::{ParsedS3Error, parse_s3_error},
    xml::{ProtocolError, expect_root, parse_bounded, serialize_bounded},
};

pub(crate) enum CompleteMultipartResponse {
    Complete(CompleteMultipartUploadOutput),
    EmbeddedError(ParsedS3Error),
}

pub(crate) enum CopyObjectResponse {
    Complete(CopyObjectOutput),
    EmbeddedError(ParsedS3Error),
}

#[derive(Serialize)]
#[serde(rename = "CompleteMultipartUpload")]
struct CompleteDocument<'a> {
    #[serde(rename = "Part")]
    parts: Vec<CompletedPartDocument<'a>>,
}

#[derive(Serialize)]
struct CompletedPartDocument<'a> {
    #[serde(rename = "PartNumber")]
    part_number: u16,
    #[serde(rename = "ETag")]
    e_tag: &'a str,
    #[serde(rename = "ChecksumCRC32", skip_serializing_if = "Option::is_none")]
    checksum_crc32: Option<&'a str>,
    #[serde(rename = "ChecksumCRC32C", skip_serializing_if = "Option::is_none")]
    checksum_crc32c: Option<&'a str>,
    #[serde(rename = "ChecksumCRC64NVME", skip_serializing_if = "Option::is_none")]
    checksum_crc64_nvme: Option<&'a str>,
    #[serde(rename = "ChecksumSHA1", skip_serializing_if = "Option::is_none")]
    checksum_sha1: Option<&'a str>,
    #[serde(rename = "ChecksumSHA256", skip_serializing_if = "Option::is_none")]
    checksum_sha256: Option<&'a str>,
}

#[derive(Deserialize)]
#[serde(rename = "InitiateMultipartUploadResult")]
struct CreateDocument {
    #[serde(rename = "Bucket")]
    bucket: Option<String>,
    #[serde(rename = "Key")]
    key: String,
    #[serde(rename = "UploadId")]
    upload_id: String,
    #[serde(rename = "ChecksumAlgorithm")]
    checksum_algorithm: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename = "CompleteMultipartUploadResult")]
struct CompleteResultDocument {
    #[serde(rename = "Location")]
    location: Option<String>,
    #[serde(rename = "Bucket")]
    bucket: Option<String>,
    #[serde(rename = "Key")]
    key: Option<String>,
    #[serde(rename = "ETag")]
    e_tag: Option<String>,
    #[serde(rename = "ChecksumCRC32")]
    checksum_crc32: Option<String>,
    #[serde(rename = "ChecksumCRC32C")]
    checksum_crc32c: Option<String>,
    #[serde(rename = "ChecksumCRC64NVME")]
    checksum_crc64_nvme: Option<String>,
    #[serde(rename = "ChecksumSHA1")]
    checksum_sha1: Option<String>,
    #[serde(rename = "ChecksumSHA256")]
    checksum_sha256: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename = "CopyObjectResult")]
struct CopyResultDocument {
    #[serde(rename = "ETag")]
    e_tag: Option<String>,
    #[serde(rename = "LastModified")]
    last_modified: Option<String>,
    #[serde(rename = "ChecksumCRC32")]
    checksum_crc32: Option<String>,
    #[serde(rename = "ChecksumCRC32C")]
    checksum_crc32c: Option<String>,
    #[serde(rename = "ChecksumCRC64NVME")]
    checksum_crc64_nvme: Option<String>,
    #[serde(rename = "ChecksumSHA1")]
    checksum_sha1: Option<String>,
    #[serde(rename = "ChecksumSHA256")]
    checksum_sha256: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename = "ListMultipartUploadsResult")]
struct ListUploadsDocument {
    #[serde(rename = "Upload", default)]
    uploads: Vec<UploadDocument>,
    #[serde(rename = "CommonPrefixes", default)]
    common_prefixes: Vec<CommonPrefixDocument>,
    #[serde(rename = "IsTruncated", default)]
    is_truncated: bool,
    #[serde(rename = "NextKeyMarker")]
    next_key_marker: Option<String>,
    #[serde(rename = "NextUploadIdMarker")]
    next_upload_id_marker: Option<String>,
}

#[derive(Deserialize)]
struct UploadDocument {
    #[serde(rename = "Key")]
    key: String,
    #[serde(rename = "UploadId")]
    upload_id: String,
    #[serde(rename = "Initiated")]
    initiated: Option<String>,
    #[serde(rename = "StorageClass")]
    storage_class: Option<String>,
}

#[derive(Deserialize)]
struct CommonPrefixDocument {
    #[serde(rename = "Prefix")]
    prefix: String,
}

pub(crate) fn serialize_complete_multipart_upload(
    request: &CompleteMultipartUploadRequest,
    maximum: usize,
) -> Result<Vec<u8>, ProtocolError> {
    let document = CompleteDocument {
        parts: request
            .completed_parts()
            .iter()
            .map(|part| CompletedPartDocument {
                part_number: part.part_number().get(),
                e_tag: part.e_tag(),
                checksum_crc32: part.checksum().crc32.as_deref(),
                checksum_crc32c: part.checksum().crc32c.as_deref(),
                checksum_crc64_nvme: part.checksum().crc64_nvme.as_deref(),
                checksum_sha1: part.checksum().sha1.as_deref(),
                checksum_sha256: part.checksum().sha256.as_deref(),
            })
            .collect(),
    };
    serialize_bounded(&document, maximum)
}

pub(crate) fn parse_create_multipart_upload(
    body: &[u8],
    maximum: usize,
) -> Result<CreateMultipartUploadOutput, ProtocolError> {
    expect_root(body, maximum, &[b"InitiateMultipartUploadResult"])?;
    let document: CreateDocument = parse_bounded(body, maximum)?;
    let upload_id = parse_upload_id(document.upload_id, "UploadId")?;
    let key = ObjectKey::new(document.key).map_err(|error| ProtocolError::InvalidField {
        field: "Key",
        reason: error.to_string(),
    })?;
    Ok(CreateMultipartUploadOutput::new(
        document.bucket,
        key,
        upload_id,
        document.checksum_algorithm,
    ))
}

pub(crate) fn parse_complete_multipart_upload(
    body: &[u8],
    maximum: usize,
) -> Result<CompleteMultipartResponse, ProtocolError> {
    // CompleteMultipartUpload may return an XML Error inside an HTTP 200. Root
    // inspection is intentionally bounded and does not parse a second body.
    if root_is_error(body, maximum)? {
        return parse_s3_error(body, maximum).map(CompleteMultipartResponse::EmbeddedError);
    }
    expect_root(body, maximum, &[b"CompleteMultipartUploadResult"])?;
    let document: CompleteResultDocument = parse_bounded(body, maximum)?;
    let key = document
        .key
        .map(ObjectKey::new)
        .transpose()
        .map_err(|error| ProtocolError::InvalidField {
            field: "Key",
            reason: error.to_string(),
        })?;
    Ok(CompleteMultipartResponse::Complete(
        CompleteMultipartUploadOutput {
            location: document.location,
            bucket: document.bucket,
            key,
            e_tag: document.e_tag,
            version_id: None,
            checksum: Checksum {
                crc32: document.checksum_crc32,
                crc32c: document.checksum_crc32c,
                crc64_nvme: document.checksum_crc64_nvme,
                sha1: document.checksum_sha1,
                sha256: document.checksum_sha256,
            },
            request_ids: Default::default(),
        },
    ))
}

pub(crate) fn parse_copy_object(
    body: &[u8],
    maximum: usize,
) -> Result<CopyObjectResponse, ProtocolError> {
    if root_is_error(body, maximum)? {
        return parse_s3_error(body, maximum).map(CopyObjectResponse::EmbeddedError);
    }
    expect_root(body, maximum, &[b"CopyObjectResult"])?;
    let document: CopyResultDocument = parse_bounded(body, maximum)?;
    let last_modified = document
        .last_modified
        .map(|value| {
            OffsetDateTime::parse(&value, &Rfc3339).map_err(|error| ProtocolError::InvalidField {
                field: "LastModified",
                reason: error.to_string(),
            })
        })
        .transpose()?;
    Ok(CopyObjectResponse::Complete(CopyObjectOutput {
        e_tag: document.e_tag,
        last_modified,
        version_id: None,
        checksum: Checksum {
            crc32: document.checksum_crc32,
            crc32c: document.checksum_crc32c,
            crc64_nvme: document.checksum_crc64_nvme,
            sha1: document.checksum_sha1,
            sha256: document.checksum_sha256,
        },
        request_ids: Default::default(),
    }))
}

pub(crate) fn parse_list_multipart_uploads(
    body: &[u8],
    maximum: usize,
) -> Result<ListMultipartUploadsOutput, ProtocolError> {
    expect_root(body, maximum, &[b"ListMultipartUploadsResult"])?;
    let document: ListUploadsDocument = parse_bounded(body, maximum)?;
    let mut uploads = Vec::with_capacity(document.uploads.len());
    for upload in document.uploads {
        let upload_id = parse_upload_id(upload.upload_id, "Upload.UploadId")?;
        let initiated = upload
            .initiated
            .map(|value| {
                OffsetDateTime::parse(&value, &Rfc3339).map_err(|error| {
                    ProtocolError::InvalidField {
                        field: "Upload.Initiated",
                        reason: error.to_string(),
                    }
                })
            })
            .transpose()?;
        let key = ObjectKey::new(upload.key).map_err(|error| ProtocolError::InvalidField {
            field: "Upload.Key",
            reason: error.to_string(),
        })?;
        uploads.push(MultipartUploadEntry::new(
            key,
            upload_id,
            initiated,
            upload.storage_class,
        ));
    }
    let next_upload_id_marker = document
        .next_upload_id_marker
        .map(|value| parse_upload_id(value, "NextUploadIdMarker"))
        .transpose()?;
    Ok(ListMultipartUploadsOutput {
        uploads,
        common_prefixes: document
            .common_prefixes
            .into_iter()
            .map(|prefix| prefix.prefix)
            .collect(),
        is_truncated: document.is_truncated,
        next_key_marker: document.next_key_marker,
        next_upload_id_marker,
        request_ids: Default::default(),
    })
}

fn parse_upload_id(value: String, field: &'static str) -> Result<UploadId, ProtocolError> {
    UploadId::new(value).map_err(|error| ProtocolError::InvalidField {
        field,
        reason: error.to_string(),
    })
}

fn root_is_error(body: &[u8], maximum: usize) -> Result<bool, ProtocolError> {
    if body.len() > maximum {
        return Err(ProtocolError::Oversized {
            actual: body.len(),
            maximum,
        });
    }
    // XML declarations and whitespace are allowed. This scanner only chooses
    // the schema; the bounded parser still validates the complete document.
    let text =
        std::str::from_utf8(body).map_err(|error| ProtocolError::InvalidXml(error.to_string()))?;
    let mut reader = quick_xml::Reader::from_str(text);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(start) | quick_xml::events::Event::Empty(start)) => {
                return Ok(start.local_name().as_ref() == b"Error");
            }
            Ok(
                quick_xml::events::Event::Decl(_)
                | quick_xml::events::Event::Text(_)
                | quick_xml::events::Event::Comment(_)
                | quick_xml::events::Event::PI(_),
            ) => {}
            Ok(quick_xml::events::Event::DocType(_)) => {
                return Err(ProtocolError::InvalidXml(
                    "DTD declarations are not accepted".to_owned(),
                ));
            }
            Ok(quick_xml::events::Event::Eof) => {
                return Err(ProtocolError::InvalidXml("missing root element".to_owned()));
            }
            Ok(_) => {}
            Err(error) => return Err(ProtocolError::InvalidXml(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::operation::{CompleteMultipartUploadRequest, CompletedPart};

    use super::*;

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
        let xml = String::from_utf8(serialize_complete_multipart_upload(&request, 4_096).unwrap())
            .unwrap();
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
    fn rejects_invalid_upload_ids_from_service_documents() {
        let create = b"<InitiateMultipartUploadResult><Key>key</Key><UploadId>bad&#10;id</UploadId></InitiateMultipartUploadResult>";
        assert!(matches!(
            parse_create_multipart_upload(create, 4_096),
            Err(ProtocolError::InvalidField {
                field: "UploadId",
                ..
            })
        ));

        let listing = b"<ListMultipartUploadsResult><NextUploadIdMarker></NextUploadIdMarker></ListMultipartUploadsResult>";
        assert!(matches!(
            parse_list_multipart_uploads(listing, 4_096),
            Err(ProtocolError::InvalidField {
                field: "NextUploadIdMarker",
                ..
            })
        ));
    }
}
