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

pub(crate) enum UploadPartCopyResponse {
    Complete(CopyPartResult),
    EmbeddedError(ParsedS3Error),
}

pub(crate) struct CopyPartResult {
    pub e_tag: String,
    pub last_modified: Option<OffsetDateTime>,
    pub checksum: Checksum,
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

pub(crate) fn parse_upload_part_copy(
    body: &[u8],
    maximum: usize,
) -> Result<UploadPartCopyResponse, ProtocolError> {
    if root_is_error(body, maximum)? {
        return parse_s3_error(body, maximum).map(UploadPartCopyResponse::EmbeddedError);
    }
    expect_root(body, maximum, &[b"CopyPartResult"])?;
    let document: CopyResultDocument = parse_bounded(body, maximum)?;
    let checksum = document.checksum();
    let e_tag = document
        .e_tag
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ProtocolError::InvalidField {
            field: "ETag",
            reason: "copied part has no usable ETag".to_owned(),
        })?;
    let last_modified = parse_last_modified(document.last_modified)?;
    Ok(UploadPartCopyResponse::Complete(CopyPartResult {
        e_tag,
        last_modified,
        checksum,
    }))
}

fn parse_last_modified(value: Option<String>) -> Result<Option<OffsetDateTime>, ProtocolError> {
    value
        .map(|value| {
            OffsetDateTime::parse(&value, &Rfc3339).map_err(|error| ProtocolError::InvalidField {
                field: "LastModified",
                reason: error.to_string(),
            })
        })
        .transpose()
}

impl CopyResultDocument {
    fn checksum(&self) -> Checksum {
        Checksum {
            crc32: self.checksum_crc32.clone(),
            crc32c: self.checksum_crc32c.clone(),
            crc64_nvme: self.checksum_crc64_nvme.clone(),
            sha1: self.checksum_sha1.clone(),
            sha256: self.checksum_sha256.clone(),
        }
    }
}
