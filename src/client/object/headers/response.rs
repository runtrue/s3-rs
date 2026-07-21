use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use http::header::{CONTENT_LENGTH, CONTENT_TYPE, ETAG, LAST_MODIFIED};
use http::{HeaderMap, HeaderValue};

use crate::error::S3Error;
use crate::operation::{Checksum, ObjectMetadata, RequestIds};

pub(in crate::client::object) fn parse_object_metadata(
    headers: &HeaderMap,
) -> Result<ObjectMetadata, S3Error> {
    let content_length = headers
        .get(CONTENT_LENGTH)
        .map(parse_u64_value)
        .transpose()?
        .ok_or_else(|| S3Error::invalid_response("S3 omitted the content-length header"))?;
    let last_modified = headers
        .get(LAST_MODIFIED)
        .map(parse_http_date_value)
        .transpose()?;
    let mut user_metadata = BTreeMap::new();
    for (name, value) in headers {
        if let Some(suffix) = name.as_str().strip_prefix("x-amz-meta-") {
            let value = response_header_value(value)?;
            user_metadata.insert(suffix.to_owned(), value);
        }
    }
    Ok(ObjectMetadata {
        e_tag: response_header(headers, ETAG.as_str())?,
        content_length,
        content_type: response_header(headers, CONTENT_TYPE.as_str())?,
        last_modified,
        version_id: response_header(headers, "x-amz-version-id")?,
        user_metadata,
        checksum: parse_checksum(headers)?,
        request_ids: parse_request_ids(headers)?,
    })
}

pub(in crate::client::object) fn parse_checksum(headers: &HeaderMap) -> Result<Checksum, S3Error> {
    Ok(Checksum {
        crc32: response_header(headers, "x-amz-checksum-crc32")?,
        crc32c: response_header(headers, "x-amz-checksum-crc32c")?,
        crc64_nvme: response_header(headers, "x-amz-checksum-crc64nvme")?,
        sha1: response_header(headers, "x-amz-checksum-sha1")?,
        sha256: response_header(headers, "x-amz-checksum-sha256")?,
    })
}

pub(in crate::client::object) fn verified_download_sha256(
    headers: &HeaderMap,
) -> Result<Option<[u8; 32]>, S3Error> {
    let Some(encoded) = response_header(headers, "x-amz-checksum-sha256")? else {
        return Ok(None);
    };
    if response_header(headers, "x-amz-checksum-type")?
        .is_some_and(|checksum_type| checksum_type != "FULL_OBJECT")
    {
        return Ok(None);
    }
    let decoded = BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| S3Error::invalid_response("S3 returned an invalid base64 SHA-256 checksum"))?;
    let digest = decoded.try_into().map_err(|_| {
        S3Error::invalid_response("S3 returned a SHA-256 checksum with an invalid length")
    })?;
    Ok(Some(digest))
}

pub(in crate::client::object) fn merge_checksum(target: &mut Checksum, headers: Checksum) {
    target.crc32 = headers.crc32.or_else(|| target.crc32.take());
    target.crc32c = headers.crc32c.or_else(|| target.crc32c.take());
    target.crc64_nvme = headers.crc64_nvme.or_else(|| target.crc64_nvme.take());
    target.sha1 = headers.sha1.or_else(|| target.sha1.take());
    target.sha256 = headers.sha256.or_else(|| target.sha256.take());
}

pub(in crate::client::object) fn parse_request_ids(
    headers: &HeaderMap,
) -> Result<RequestIds, S3Error> {
    Ok(RequestIds {
        request_id: response_header(headers, "x-amz-request-id")?,
        host_id: response_header(headers, "x-amz-id-2")?,
    })
}

pub(in crate::client::object) fn response_header(
    headers: &HeaderMap,
    name: &str,
) -> Result<Option<String>, S3Error> {
    headers.get(name).map(response_header_value).transpose()
}

fn response_header_value(value: &HeaderValue) -> Result<String, S3Error> {
    value
        .to_str()
        .map(str::to_owned)
        .map_err(|_| S3Error::invalid_response("S3 returned a non-text response header"))
}

pub(in crate::client::object) fn parse_u64_value(value: &HeaderValue) -> Result<u64, S3Error> {
    value
        .to_str()
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| S3Error::invalid_response("S3 returned an invalid content length"))
}

fn parse_http_date_value(value: &HeaderValue) -> Result<time::OffsetDateTime, S3Error> {
    let value = value
        .to_str()
        .map_err(|_| S3Error::invalid_response("S3 returned an invalid HTTP date"))?;
    let parsed = httpdate::parse_http_date(value)
        .map_err(|_| S3Error::invalid_response("S3 returned an invalid HTTP date"))?;
    Ok(time::OffsetDateTime::from(parsed))
}

pub(in crate::client::object) fn parse_bool_header(
    headers: &HeaderMap,
    name: &str,
) -> Result<Option<bool>, S3Error> {
    let Some(value) = response_header(headers, name)? else {
        return Ok(None);
    };
    match value.as_str() {
        "true" => Ok(Some(true)),
        "false" => Ok(Some(false)),
        _ => Err(S3Error::invalid_response(
            "S3 returned an invalid boolean response header",
        )),
    }
}

pub(in crate::client::object) fn parse_content_range(
    headers: &HeaderMap,
) -> Result<Option<(u64, u64, Option<u64>)>, S3Error> {
    let Some(value) = response_header(headers, "content-range")? else {
        return Ok(None);
    };
    let remainder = value
        .strip_prefix("bytes ")
        .ok_or_else(|| S3Error::invalid_response("S3 returned an invalid content-range header"))?;
    let (range, complete) = remainder
        .split_once('/')
        .ok_or_else(|| S3Error::invalid_response("S3 returned an invalid content-range header"))?;
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| S3Error::invalid_response("S3 returned an invalid content-range header"))?;
    let start = start
        .parse::<u64>()
        .map_err(|_| S3Error::invalid_response("S3 returned an invalid content-range header"))?;
    let end = end
        .parse::<u64>()
        .map_err(|_| S3Error::invalid_response("S3 returned an invalid content-range header"))?;
    if end < start {
        return Err(S3Error::invalid_response(
            "S3 returned an invalid content-range header",
        ));
    }
    let complete = if complete == "*" {
        None
    } else {
        let complete = complete.parse::<u64>().map_err(|_| {
            S3Error::invalid_response("S3 returned an invalid content-range header")
        })?;
        if end >= complete {
            return Err(S3Error::invalid_response(
                "S3 returned an inconsistent content-range header",
            ));
        }
        Some(complete)
    };
    Ok(Some((start, end, complete)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCategory;
    use sha2::{Digest as _, Sha256};
    use time::macros::datetime;

    #[test]
    fn parses_complete_metadata_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("7"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        headers.insert(ETAG, HeaderValue::from_static("\"tag\""));
        headers.insert(
            LAST_MODIFIED,
            HeaderValue::from_static("Tue, 12 Mar 2024 10:15:30 GMT"),
        );
        headers.insert("x-amz-version-id", HeaderValue::from_static("version"));
        headers.insert("x-amz-meta-owner", HeaderValue::from_static("runtrue"));
        headers.insert("x-amz-checksum-sha256", HeaderValue::from_static("sum="));
        headers.insert("x-amz-request-id", HeaderValue::from_static("request"));
        headers.insert("x-amz-id-2", HeaderValue::from_static("host"));

        let parsed = parse_object_metadata(&headers).unwrap();
        assert_eq!(parsed.content_length, 7);
        assert_eq!(parsed.content_type.as_deref(), Some("text/plain"));
        assert_eq!(parsed.e_tag.as_deref(), Some("\"tag\""));
        assert_eq!(parsed.user_metadata["owner"], "runtrue");
        assert_eq!(parsed.checksum.sha256.as_deref(), Some("sum="));
        assert_eq!(parsed.request_ids.request_id.as_deref(), Some("request"));
        assert_eq!(
            parsed.last_modified,
            Some(datetime!(2024-03-12 10:15:30 UTC))
        );
    }

    #[test]
    fn validates_only_full_object_sha256_checksums() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-amz-checksum-sha256",
            HeaderValue::from_static("ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0="),
        );
        assert_eq!(
            verified_download_sha256(&headers).unwrap(),
            Some(Sha256::digest(b"abc").into())
        );

        headers.insert("x-amz-checksum-type", HeaderValue::from_static("COMPOSITE"));
        assert_eq!(verified_download_sha256(&headers).unwrap(), None);

        headers.insert(
            "x-amz-checksum-type",
            HeaderValue::from_static("FULL_OBJECT"),
        );
        headers.insert(
            "x-amz-checksum-sha256",
            HeaderValue::from_static("not-base64"),
        );
        assert_eq!(
            verified_download_sha256(&headers).unwrap_err().category(),
            ErrorCategory::InvalidResponse
        );
        headers.insert("x-amz-checksum-sha256", HeaderValue::from_static("YQ=="));
        assert_eq!(
            verified_download_sha256(&headers).unwrap_err().category(),
            ErrorCategory::InvalidResponse
        );
    }

    #[test]
    fn content_ranges_are_strictly_validated() {
        let mut headers = HeaderMap::new();
        headers.insert("content-range", HeaderValue::from_static("bytes 2-8/10"));
        assert_eq!(
            parse_content_range(&headers).unwrap(),
            Some((2, 8, Some(10)))
        );
        headers.insert("content-range", HeaderValue::from_static("bytes 8-2/10"));
        assert!(parse_content_range(&headers).is_err());
        headers.insert("content-range", HeaderValue::from_static("bytes 2-10/10"));
        assert!(parse_content_range(&headers).is_err());
    }
}
