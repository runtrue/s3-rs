use serde::Deserialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::common::root_is_error;
use crate::operation::{Checksum, CopyObjectOutput};
use crate::protocol::xml::{expect_root, parse_bounded};
use crate::protocol::{ParsedS3Error, ProtocolError, parse_s3_error};

pub(crate) enum CopyObjectResponse {
    Complete(CopyObjectOutput),
    EmbeddedError(ParsedS3Error),
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
