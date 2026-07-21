use serde::Deserialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::common::parse_upload_id;
use crate::operation::{ListMultipartUploadsOutput, MultipartUploadEntry, ObjectKey};
use crate::protocol::ProtocolError;
use crate::protocol::xml::{expect_root, parse_bounded};

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
    let next_key_marker = document.next_key_marker.filter(|value| !value.is_empty());
    let next_upload_id_marker = document
        .next_upload_id_marker
        .filter(|value| !value.is_empty())
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
        next_key_marker,
        next_upload_id_marker,
        request_ids: Default::default(),
    })
}
