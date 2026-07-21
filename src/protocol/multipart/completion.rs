use serde::{Deserialize, Serialize};

use super::common::root_is_error;
use crate::operation::{
    Checksum, CompleteMultipartUploadOutput, CompleteMultipartUploadRequest, ObjectKey,
};
use crate::protocol::xml::{expect_root, parse_bounded, serialize_bounded};
use crate::protocol::{ParsedS3Error, ProtocolError, parse_s3_error};

pub(crate) enum CompleteMultipartResponse {
    Complete(CompleteMultipartUploadOutput),
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

pub(crate) fn parse_complete_multipart_upload(
    body: &[u8],
    maximum: usize,
) -> Result<CompleteMultipartResponse, ProtocolError> {
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
