use std::collections::BTreeMap;
use std::time::SystemTime;

use http::{HeaderMap, HeaderName};

use crate::client::request::{insert_header, insert_named_header, insert_optional_named_header};
use crate::error::S3Error;
use crate::operation::{ChecksumAlgorithm, Conditions};
use crate::stream::PreparedBody;

pub(in crate::client::object) fn insert_upload_checksum(
    headers: &mut HeaderMap,
    algorithm: Option<ChecksumAlgorithm>,
    body: &PreparedBody,
) -> Result<(), S3Error> {
    let Some(algorithm) = algorithm else {
        return Ok(());
    };
    match algorithm {
        ChecksumAlgorithm::Sha256 => {
            insert_header(
                headers,
                HeaderName::from_static("x-amz-sdk-checksum-algorithm"),
                "SHA256",
            )?;
            insert_header(
                headers,
                HeaderName::from_static("x-amz-checksum-sha256"),
                &body.sha256_base64(),
            )
        }
        ChecksumAlgorithm::Crc32
        | ChecksumAlgorithm::Crc32c
        | ChecksumAlgorithm::Crc64Nvme
        | ChecksumAlgorithm::Sha1 => Err(S3Error::unsupported(
            "the selected upload checksum algorithm is not implemented",
        )),
    }
}

pub(in crate::client) fn insert_conditions(
    headers: &mut HeaderMap,
    conditions: &Conditions,
    prefix: &str,
) -> Result<(), S3Error> {
    insert_optional_named_header(
        headers,
        &format!("{prefix}if-match"),
        conditions.if_match.as_deref(),
    )?;
    insert_optional_named_header(
        headers,
        &format!("{prefix}if-none-match"),
        conditions.if_none_match.as_deref(),
    )?;
    if let Some(value) = conditions.if_modified_since {
        insert_named_header(
            headers,
            &format!("{prefix}if-modified-since"),
            &format_http_date(value),
        )?;
    }
    if let Some(value) = conditions.if_unmodified_since {
        insert_named_header(
            headers,
            &format!("{prefix}if-unmodified-since"),
            &format_http_date(value),
        )?;
    }
    Ok(())
}

fn format_http_date(value: time::OffsetDateTime) -> String {
    let system_time = SystemTime::from(value);
    httpdate::fmt_http_date(system_time)
}

pub(in crate::client::object) fn insert_user_metadata(
    headers: &mut HeaderMap,
    metadata: &BTreeMap<String, String>,
) -> Result<(), S3Error> {
    for (name, value) in metadata {
        insert_named_header(headers, &format!("x-amz-meta-{name}"), value)?;
    }
    Ok(())
}

pub(in crate::client::object) fn optional_query(
    name: &str,
    value: Option<&str>,
) -> Vec<(String, String)> {
    value.map_or_else(Vec::new, |value| vec![(name.to_owned(), value.to_owned())])
}

pub(in crate::client::object) fn push_optional_query(
    query: &mut Vec<(String, String)>,
    name: &str,
    value: Option<&str>,
) {
    if let Some(value) = value {
        query.push((name.to_owned(), value.to_owned()));
    }
}

pub(in crate::client) fn copy_source_header(source: &crate::operation::CopySource) -> String {
    let mut output = String::from("/");
    percent_encode(source.bucket.as_bytes(), false, &mut output);
    output.push('/');
    percent_encode(source.key.as_str().as_bytes(), true, &mut output);
    if let Some(version_id) = &source.version_id {
        output.push_str("?versionId=");
        percent_encode(version_id.as_bytes(), false, &mut output);
    }
    output
}

fn percent_encode(bytes: &[u8], preserve_slash: bool, output: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in bytes {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~')
            || (preserve_slash && byte == b'/')
        {
            output.push(char::from(byte));
        } else {
            output.push('%');
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::{CopySource, ObjectKey};
    use time::macros::datetime;

    #[test]
    fn copy_source_encodes_path_and_version_exactly_once() {
        let source = CopySource {
            bucket: "source.bucket".to_owned(),
            key: ObjectKey::new("folder/a b%+c").unwrap(),
            version_id: Some("v +/=".to_owned()),
        };
        assert_eq!(
            copy_source_header(&source),
            "/source.bucket/folder/a%20b%25%2Bc?versionId=v%20%2B%2F%3D"
        );
    }

    #[test]
    fn conditional_dates_use_http_wire_format() {
        assert_eq!(
            format_http_date(datetime!(2024-03-12 10:15:30 UTC)),
            "Tue, 12 Mar 2024 10:15:30 GMT"
        );
    }
}
