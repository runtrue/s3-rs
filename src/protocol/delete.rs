use serde::{Deserialize, Serialize};

use crate::operation::{
    DeleteError, DeleteObjectsOutput, DeleteObjectsRequest, DeletedObject, ObjectKey,
};

use super::xml::{ProtocolError, expect_root, parse_bounded, serialize_bounded};

#[derive(Serialize)]
#[serde(rename = "Delete")]
struct DeleteDocument<'a> {
    #[serde(rename = "Object")]
    objects: Vec<DeleteEntry<'a>>,
    #[serde(rename = "Quiet")]
    quiet: bool,
}

#[derive(Serialize)]
struct DeleteEntry<'a> {
    #[serde(rename = "Key")]
    key: &'a str,
    #[serde(rename = "VersionId", skip_serializing_if = "Option::is_none")]
    version_id: Option<&'a str>,
    #[serde(rename = "ETag", skip_serializing_if = "Option::is_none")]
    e_tag: Option<&'a str>,
}

#[derive(Deserialize)]
#[serde(rename = "DeleteResult")]
struct DeleteResultDocument {
    #[serde(rename = "Deleted", default)]
    deleted: Vec<DeletedDocument>,
    #[serde(rename = "Error", default)]
    errors: Vec<DeleteErrorDocument>,
}

#[derive(Deserialize)]
struct DeletedDocument {
    #[serde(rename = "Key")]
    key: String,
    #[serde(rename = "VersionId")]
    version_id: Option<String>,
    #[serde(rename = "DeleteMarker", default)]
    delete_marker: bool,
    #[serde(rename = "DeleteMarkerVersionId")]
    delete_marker_version_id: Option<String>,
}

#[derive(Deserialize)]
struct DeleteErrorDocument {
    #[serde(rename = "Key")]
    key: String,
    #[serde(rename = "VersionId")]
    version_id: Option<String>,
    #[serde(rename = "Code")]
    code: Option<String>,
    #[serde(rename = "Message")]
    message: Option<String>,
}

pub(crate) fn serialize_delete_objects(
    request: &DeleteObjectsRequest,
    maximum: usize,
) -> Result<Vec<u8>, ProtocolError> {
    let document = DeleteDocument {
        objects: request
            .objects()
            .iter()
            .map(|object| DeleteEntry {
                key: object.key.as_str(),
                version_id: object.version_id.as_deref(),
                e_tag: object.if_match.as_deref(),
            })
            .collect(),
        quiet: request.quiet,
    };
    serialize_bounded(&document, maximum)
}

pub(crate) fn parse_delete_objects(
    body: &[u8],
    maximum: usize,
) -> Result<DeleteObjectsOutput, ProtocolError> {
    expect_root(body, maximum, &[b"DeleteResult"])?;
    let document: DeleteResultDocument = parse_bounded(body, maximum)?;
    let mut deleted = Vec::with_capacity(document.deleted.len());
    for item in document.deleted {
        deleted.push(DeletedObject {
            key: ObjectKey::new(item.key).map_err(|error| ProtocolError::InvalidField {
                field: "Deleted.Key",
                reason: error.to_string(),
            })?,
            version_id: item.version_id,
            delete_marker: item.delete_marker,
            delete_marker_version_id: item.delete_marker_version_id,
        });
    }
    Ok(DeleteObjectsOutput {
        deleted,
        errors: document
            .errors
            .into_iter()
            .map(|item| DeleteError {
                key: item.key,
                version_id: item.version_id,
                code: item.code,
                message: item.message,
            })
            .collect(),
        request_ids: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use crate::operation::DeleteObjectRequest;

    use super::*;

    #[test]
    fn delete_serialization_escapes_keys_and_honors_bounds() {
        let request = DeleteObjectsRequest::new(vec![DeleteObjectRequest::new(
            ObjectKey::new("a<&b").unwrap(),
        )])
        .unwrap();
        let body = serialize_delete_objects(&request, 1_024).unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("a&lt;&amp;b"));
        assert!(matches!(
            serialize_delete_objects(&request, 8),
            Err(ProtocolError::Oversized { .. })
        ));
    }

    #[test]
    fn delete_serialization_retains_per_object_etag_conditions() {
        let mut object = DeleteObjectRequest::new(ObjectKey::new("key").unwrap());
        object.if_match = Some("\"etag\"".to_owned());
        let request = DeleteObjectsRequest::new(vec![object]).unwrap();
        let body = serialize_delete_objects(&request, 1_024).unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("<ETag>\"etag\"</ETag>"));
    }

    #[test]
    fn parses_mixed_delete_results() {
        let body = br"<DeleteResult><Deleted><Key>good</Key></Deleted><Error><Key>bad</Key><Code>AccessDenied</Code><Message>denied</Message></Error></DeleteResult>";
        let result = parse_delete_objects(body, 4_096).unwrap();
        assert_eq!(result.deleted[0].key.as_str(), "good");
        assert_eq!(result.errors[0].code.as_deref(), Some("AccessDenied"));
    }
}
