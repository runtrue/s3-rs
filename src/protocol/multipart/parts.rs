use serde::Deserialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::operation::{Checksum, ChecksumType, ListPartsOutput, ListedPart, PartNumber};
use crate::protocol::ProtocolError;
use crate::protocol::xml::{expect_root, parse_bounded};

#[derive(Deserialize)]
#[serde(rename = "ListPartsResult")]
struct ListPartsDocument {
    #[serde(rename = "Part", default)]
    parts: Vec<PartDocument>,
    #[serde(rename = "IsTruncated", default)]
    is_truncated: bool,
    #[serde(rename = "NextPartNumberMarker")]
    next_part_number_marker: Option<u16>,
    #[serde(rename = "ChecksumAlgorithm")]
    checksum_algorithm: Option<String>,
    #[serde(rename = "ChecksumType")]
    checksum_type: Option<String>,
}

#[derive(Deserialize)]
struct PartDocument {
    #[serde(rename = "PartNumber")]
    part_number: u16,
    #[serde(rename = "ETag")]
    e_tag: String,
    #[serde(rename = "LastModified")]
    last_modified: Option<String>,
    #[serde(rename = "Size")]
    size: u64,
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

pub(crate) fn parse_list_parts(
    body: &[u8],
    maximum: usize,
) -> Result<ListPartsOutput, ProtocolError> {
    expect_root(body, maximum, &[b"ListPartsResult"])?;
    let document: ListPartsDocument = parse_bounded(body, maximum)?;
    let mut parts = Vec::with_capacity(document.parts.len());
    let mut previous = None;
    for part in document.parts {
        let part_number =
            PartNumber::new(part.part_number).ok_or_else(|| ProtocolError::InvalidField {
                field: "Part.PartNumber",
                reason: "part number is outside 1..=10000".to_owned(),
            })?;
        if previous.is_some_and(|value| value >= part_number) {
            return Err(ProtocolError::InvalidField {
                field: "Part.PartNumber",
                reason: "parts are not in strictly ascending order".to_owned(),
            });
        }
        previous = Some(part_number);
        if part.e_tag.trim().is_empty() {
            return Err(ProtocolError::InvalidField {
                field: "Part.ETag",
                reason: "ETag is empty".to_owned(),
            });
        }
        let last_modified = part
            .last_modified
            .map(|value| {
                OffsetDateTime::parse(&value, &Rfc3339).map_err(|error| {
                    ProtocolError::InvalidField {
                        field: "Part.LastModified",
                        reason: error.to_string(),
                    }
                })
            })
            .transpose()?;
        parts.push(ListedPart::new(
            part_number,
            part.e_tag,
            last_modified,
            part.size,
            Checksum {
                crc32: part.checksum_crc32,
                crc32c: part.checksum_crc32c,
                crc64_nvme: part.checksum_crc64_nvme,
                sha1: part.checksum_sha1,
                sha256: part.checksum_sha256,
            },
        ));
    }
    let next_part_number_marker = document
        .next_part_number_marker
        .map(|value| {
            PartNumber::new(value).ok_or_else(|| ProtocolError::InvalidField {
                field: "NextPartNumberMarker",
                reason: "part number is outside 1..=10000".to_owned(),
            })
        })
        .transpose()?;
    let checksum_type = document.checksum_type.map(ChecksumType::parse);
    Ok(ListPartsOutput {
        parts,
        is_truncated: document.is_truncated,
        next_part_number_marker,
        checksum_algorithm: document.checksum_algorithm,
        checksum_type,
        request_ids: Default::default(),
    })
}
