use serde::Deserialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::operation::{ChecksumType, ListObjectsV2Output, ListedObject, ObjectKey, ObjectOwner};

use super::xml::{ProtocolError, expect_root, parse_bounded};

#[derive(Deserialize)]
#[serde(rename = "ListBucketResult")]
struct ListBucketDocument {
    #[serde(rename = "EncodingType")]
    encoding_type: Option<String>,
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
    #[serde(rename = "Owner")]
    owner: Option<OwnerDocument>,
    #[serde(rename = "ChecksumAlgorithm", default)]
    checksum_algorithms: Vec<String>,
    #[serde(rename = "ChecksumType")]
    checksum_type: Option<String>,
}

#[derive(Deserialize)]
struct OwnerDocument {
    #[serde(rename = "ID")]
    id: Option<String>,
    #[serde(rename = "DisplayName")]
    display_name: Option<String>,
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
    if parsed.encoding_type.as_deref() != Some("url") {
        return Err(ProtocolError::InvalidField {
            field: "EncodingType",
            reason: "expected url encoding requested by the client".to_owned(),
        });
    }
    let mut objects = Vec::with_capacity(parsed.contents.len());
    for object in parsed.contents {
        let decoded_key = decode_url_field(&object.key, "Contents.Key")?;
        let key = ObjectKey::new(decoded_key).map_err(|error| ProtocolError::InvalidField {
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
            owner: object.owner.map(|owner| ObjectOwner {
                id: owner.id,
                display_name: owner.display_name,
            }),
            checksum_algorithms: object.checksum_algorithms,
            checksum_type: object.checksum_type.map(ChecksumType::parse),
        });
    }

    Ok(ListObjectsV2Output {
        objects,
        common_prefixes: parsed
            .common_prefixes
            .into_iter()
            .map(|prefix| decode_url_field(&prefix.prefix, "CommonPrefixes.Prefix"))
            .collect::<Result<_, _>>()?,
        is_truncated: parsed.is_truncated,
        next_continuation_token: parsed.next_continuation_token,
        key_count: parsed.key_count,
        request_ids: Default::default(),
    })
}

fn decode_url_field(value: &str, field: &'static str) -> Result<String, ProtocolError> {
    let input = value.as_bytes();
    let mut decoded = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' {
            let high = input.get(index + 1).and_then(|byte| hex_value(*byte));
            let low = input.get(index + 2).and_then(|byte| hex_value(*byte));
            let (Some(high), Some(low)) = (high, low) else {
                return Err(ProtocolError::InvalidField {
                    field,
                    reason: "invalid URL percent encoding".to_owned(),
                });
            };
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(input[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|error| ProtocolError::InvalidField {
        field,
        reason: format!("URL-decoded value is not UTF-8: {error}"),
    })
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn decodes_url_encoded_keys_once_and_exposes_listing_metadata() {
        let body = br#"<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><EncodingType>url</EncodingType><IsTruncated>true</IsTruncated><Contents><Key>a/b%2520c%2B%20snowman-%E2%98%83</Key><LastModified>2024-03-12T10:15:30Z</LastModified><ETag>&quot;etag&quot;</ETag><Size>7</Size><StorageClass>STANDARD</StorageClass><Owner><ID>canonical-id</ID><DisplayName>owner</DisplayName></Owner><ChecksumAlgorithm>CRC32C</ChecksumAlgorithm><ChecksumAlgorithm>SHA256</ChecksumAlgorithm><ChecksumType>COMPOSITE</ChecksumType></Contents><Contents><Key>control-%01</Key><Size>0</Size></Contents><CommonPrefixes><Prefix>a%2F</Prefix></CommonPrefixes><KeyCount>2</KeyCount><NextContinuationToken>opaque+/=</NextContinuationToken></ListBucketResult>"#;
        let parsed = parse_list_objects_v2(body, 8_192).unwrap();
        assert!(parsed.is_truncated);
        assert_eq!(parsed.objects[0].key.as_str(), "a/b%20c+ snowman-☃");
        assert_eq!(parsed.objects[1].key.as_str().as_bytes(), b"control-\x01");
        assert_eq!(parsed.objects[0].size, 7);
        assert_eq!(parsed.common_prefixes, ["a/"]);
        assert_eq!(
            parsed.objects[0]
                .owner
                .as_ref()
                .and_then(|owner| owner.id.as_deref()),
            Some("canonical-id")
        );
        assert_eq!(parsed.objects[0].checksum_algorithms, ["CRC32C", "SHA256"]);
        assert_eq!(
            parsed.objects[0].checksum_type,
            Some(ChecksumType::Composite)
        );
        assert_eq!(parsed.next_continuation_token.as_deref(), Some("opaque+/="));
    }

    #[test]
    fn rejects_invalid_remote_keys() {
        let body =
            b"<ListBucketResult><EncodingType>url</EncodingType><Contents><Key></Key><Size>0</Size></Contents></ListBucketResult>";
        assert!(matches!(
            parse_list_objects_v2(body, 4_096),
            Err(ProtocolError::InvalidField {
                field: "Contents.Key",
                ..
            })
        ));
    }

    #[test]
    fn rejects_invalid_url_encoding_without_double_decoding() {
        let malformed = b"<ListBucketResult><EncodingType>url</EncodingType><Contents><Key>bad%2</Key><Size>0</Size></Contents></ListBucketResult>";
        assert!(matches!(
            parse_list_objects_v2(malformed, 4_096),
            Err(ProtocolError::InvalidField {
                field: "Contents.Key",
                ..
            })
        ));

        assert_eq!(
            decode_url_field("literal%252F", "Contents.Key").unwrap(),
            "literal%2F"
        );
    }

    #[test]
    fn preserves_new_checksum_aggregation_types() {
        let body = b"<ListBucketResult><EncodingType>url</EncodingType><Contents><Key>key</Key><Size>0</Size><ChecksumType>NEW_MODE</ChecksumType></Contents></ListBucketResult>";
        let parsed = parse_list_objects_v2(body, 4096).unwrap();
        assert_eq!(
            parsed.objects[0].checksum_type,
            Some(ChecksumType::Unknown("NEW_MODE".to_owned()))
        );
    }

    proptest! {
        #[test]
        fn arbitrary_list_documents_never_panic(body in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let _ = parse_list_objects_v2(&body, 4_096);
        }

        #[test]
        fn pagination_tokens_remain_opaque(token in "[A-Za-z0-9+/=._~-]{0,200}") {
            let body = format!(
                "<ListBucketResult><EncodingType>url</EncodingType><IsTruncated>true</IsTruncated><NextContinuationToken>{token}</NextContinuationToken></ListBucketResult>"
            );
            let parsed = parse_list_objects_v2(body.as_bytes(), 4_096).unwrap();
            prop_assert!(parsed.is_truncated);
            prop_assert_eq!(parsed.next_continuation_token.as_deref(), Some(token.as_str()));
        }
    }
}
