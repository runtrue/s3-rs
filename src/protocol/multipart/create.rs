use serde::Deserialize;

use super::common::parse_upload_id;
use crate::operation::{ChecksumType, CreateMultipartUploadOutput, ObjectKey};
use crate::protocol::ProtocolError;
use crate::protocol::xml::{expect_root, parse_bounded};

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
    #[serde(rename = "ChecksumType")]
    checksum_type: Option<String>,
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
    let checksum_type = document.checksum_type.map(ChecksumType::parse);
    Ok(CreateMultipartUploadOutput::new(
        document.bucket,
        key,
        upload_id,
        document.checksum_algorithm,
        checksum_type,
    ))
}
