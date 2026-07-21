use serde::Deserialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::operation::{ListObjectsV2Output, ListedObject, ObjectKey};

use super::xml::{ProtocolError, expect_root, parse_bounded};

#[derive(Deserialize)]
#[serde(rename = "ListBucketResult")]
struct ListBucketDocument {
    #[serde(rename = "Contents", default)]
    contents: Vec<ObjectDocument>,
    #[serde(rename = "CommonPrefixes", default)]
    common_prefixes: Vec<CommonPrefixDocument>,
    #[serde(rename = "IsTruncated", default)]
    is_truncated: bool,
    #[serde(rename = "NextContinuationToken")]
    next_continuation_token: Option<String>,
    #[serde(rename = "KeyCount")]
    key_count: Option<u32>,
}

#[derive(Deserialize)]
struct ObjectDocument {
    #[serde(rename = "Key")]
    key: String,
    #[serde(rename = "LastModified")]
    last_modified: Option<String>,
    #[serde(rename = "ETag")]
    e_tag: Option<String>,
    #[serde(rename = "Size")]
    size: u64,
    #[serde(rename = "StorageClass")]
    storage_class: Option<String>,
}

#[derive(Deserialize)]
struct CommonPrefixDocument {
    #[serde(rename = "Prefix")]
    prefix: String,
}

pub(crate) fn parse_list_objects_v2(
    body: &[u8],
    maximum: usize,
) -> Result<ListObjectsV2Output, ProtocolError> {
    expect_root(body, maximum, &[b"ListBucketResult"])?;
    let parsed: ListBucketDocument = parse_bounded(body, maximum)?;
    let mut objects = Vec::with_capacity(parsed.contents.len());
    for object in parsed.contents {
        let key = ObjectKey::new(object.key).map_err(|error| ProtocolError::InvalidField {
            field: "Contents.Key",
            reason: error.to_string(),
        })?;
        let last_modified = object
            .last_modified
            .map(|value| {
                OffsetDateTime::parse(&value, &Rfc3339).map_err(|error| {
                    ProtocolError::InvalidField {
                        field: "Contents.LastModified",
                        reason: error.to_string(),
                    }
                })
            })
            .transpose()?;
        objects.push(ListedObject {
            key,
            last_modified,
            e_tag: object.e_tag,
            size: object.size,
            storage_class: object.storage_class,
        });
    }

    Ok(ListObjectsV2Output {
        objects,
        common_prefixes: parsed
            .common_prefixes
            .into_iter()
            .map(|prefix| prefix.prefix)
            .collect(),
        is_truncated: parsed.is_truncated,
        next_continuation_token: parsed.next_continuation_token,
        key_count: parsed.key_count,
        request_ids: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parses_listing_page_without_decoding_key_semantics() {
        let body = br#"<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><IsTruncated>true</IsTruncated><Contents><Key>a/b%20c</Key><LastModified>2024-03-12T10:15:30Z</LastModified><ETag>&quot;etag&quot;</ETag><Size>7</Size><StorageClass>STANDARD</StorageClass></Contents><CommonPrefixes><Prefix>a/</Prefix></CommonPrefixes><KeyCount>1</KeyCount><NextContinuationToken>opaque+/=</NextContinuationToken></ListBucketResult>"#;
        let parsed = parse_list_objects_v2(body, 8_192).unwrap();
        assert!(parsed.is_truncated);
        assert_eq!(parsed.objects[0].key.as_str(), "a/b%20c");
        assert_eq!(parsed.objects[0].size, 7);
        assert_eq!(parsed.common_prefixes, ["a/"]);
        assert_eq!(parsed.next_continuation_token.as_deref(), Some("opaque+/="));
    }

    #[test]
    fn rejects_invalid_remote_keys() {
        let body =
            b"<ListBucketResult><Contents><Key></Key><Size>0</Size></Contents></ListBucketResult>";
        assert!(matches!(
            parse_list_objects_v2(body, 4_096),
            Err(ProtocolError::InvalidField {
                field: "Contents.Key",
                ..
            })
        ));
    }

    proptest! {
        #[test]
        fn arbitrary_list_documents_never_panic(body in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let _ = parse_list_objects_v2(&body, 4_096);
        }

        #[test]
        fn pagination_tokens_remain_opaque(token in "[A-Za-z0-9+/=._~-]{0,200}") {
            let body = format!(
                "<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>{token}</NextContinuationToken></ListBucketResult>"
            );
            let parsed = parse_list_objects_v2(body.as_bytes(), 4_096).unwrap();
            prop_assert!(parsed.is_truncated);
            prop_assert_eq!(parsed.next_continuation_token.as_deref(), Some(token.as_str()));
        }
    }
}
