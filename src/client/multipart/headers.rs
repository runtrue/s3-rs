use http::header::CONTENT_TYPE;
use http::{HeaderMap, HeaderName};

use crate::client::request::{insert_header, request_headers};
use crate::error::S3Error;
use crate::operation::{Checksum, ChecksumAlgorithm, CreateMultipartUploadRequest, RequestIds};

pub(super) fn create_headers(request: &CreateMultipartUploadRequest) -> Result<HeaderMap, S3Error> {
    let mut headers = request_headers(request.headers.clone())?;
    if let Some(content_type) = &request.content_type {
        insert_header(&mut headers, CONTENT_TYPE, content_type)?;
    }
    if let Some(algorithm) = request.checksum_algorithm {
        insert_header(
            &mut headers,
            HeaderName::from_static("x-amz-checksum-algorithm"),
            checksum_algorithm_name(algorithm),
        )?;
    }
    if let Some(checksum_type) = &request.checksum_type {
        insert_header(
            &mut headers,
            HeaderName::from_static("x-amz-checksum-type"),
            checksum_type.as_str(),
        )?;
    }
    for (name, value) in &request.user_metadata {
        if name.is_empty() {
            return Err(S3Error::configuration(
                "user metadata names cannot be empty",
            ));
        }
        let name = HeaderName::from_bytes(format!("x-amz-meta-{name}").as_bytes())
            .map_err(|_| S3Error::configuration("user metadata name is not a valid HTTP header"))?;
        insert_header(&mut headers, name, value)?;
    }
    Ok(headers)
}

const fn checksum_algorithm_name(algorithm: ChecksumAlgorithm) -> &'static str {
    match algorithm {
        ChecksumAlgorithm::Crc32 => "CRC32",
        ChecksumAlgorithm::Crc32c => "CRC32C",
        ChecksumAlgorithm::Crc64Nvme => "CRC64NVME",
        ChecksumAlgorithm::Sha1 => "SHA1",
        ChecksumAlgorithm::Sha256 => "SHA256",
    }
}

pub(super) fn checksum_headers(
    checksum: &Checksum,
    headers: HeaderMap,
) -> Result<HeaderMap, S3Error> {
    let mut headers = request_headers(headers)?;
    insert_checksum(
        &mut headers,
        "x-amz-checksum-crc32",
        checksum.crc32.as_deref(),
        4,
    )?;
    insert_checksum(
        &mut headers,
        "x-amz-checksum-crc32c",
        checksum.crc32c.as_deref(),
        4,
    )?;
    insert_checksum(
        &mut headers,
        "x-amz-checksum-crc64nvme",
        checksum.crc64_nvme.as_deref(),
        8,
    )?;
    insert_checksum(
        &mut headers,
        "x-amz-checksum-sha1",
        checksum.sha1.as_deref(),
        20,
    )?;
    insert_checksum(
        &mut headers,
        "x-amz-checksum-sha256",
        checksum.sha256.as_deref(),
        32,
    )?;
    Ok(headers)
}

fn insert_checksum(
    headers: &mut HeaderMap,
    name: &'static str,
    value: Option<&str>,
    digest_length: usize,
) -> Result<(), S3Error> {
    let Some(value) = value else {
        return Ok(());
    };
    if !valid_base64_digest(value, digest_length) {
        return Err(S3Error::configuration(
            "checksum is not valid standard base64 for its algorithm",
        ));
    }
    insert_header(headers, HeaderName::from_static(name), value)
}

fn valid_base64_digest(value: &str, digest_length: usize) -> bool {
    let encoded_length = digest_length.div_ceil(3) * 4;
    if value.len() != encoded_length {
        return false;
    }
    let padding = match digest_length % 3 {
        0 => 0,
        1 => 2,
        2 => 1,
        _ => unreachable!(),
    };
    let data_length = encoded_length - padding;
    if !value.as_bytes()[..data_length]
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
        || !value.as_bytes()[data_length..]
            .iter()
            .all(|byte| *byte == b'=')
    {
        return false;
    }
    let Some(last) = value
        .as_bytes()
        .get(data_length - 1)
        .and_then(|byte| match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        })
    else {
        return false;
    };
    match padding {
        0 => true,
        1 => last.trailing_zeros() >= 2,
        2 => last.trailing_zeros() >= 4,
        _ => unreachable!(),
    }
}

pub(super) fn response_checksums(headers: &HeaderMap) -> Result<Checksum, S3Error> {
    Ok(Checksum {
        crc32: optional_checksum_header(headers, "x-amz-checksum-crc32", 4)?,
        crc32c: optional_checksum_header(headers, "x-amz-checksum-crc32c", 4)?,
        crc64_nvme: optional_checksum_header(headers, "x-amz-checksum-crc64nvme", 8)?,
        sha1: optional_checksum_header(headers, "x-amz-checksum-sha1", 20)?,
        sha256: optional_checksum_header(headers, "x-amz-checksum-sha256", 32)?,
    })
}

fn optional_checksum_header(
    headers: &HeaderMap,
    name: &'static str,
    digest_length: usize,
) -> Result<Option<String>, S3Error> {
    let value = optional_header(headers, name)?;
    if value
        .as_deref()
        .is_some_and(|value| !valid_base64_digest(value, digest_length))
    {
        return Err(S3Error::invalid_response(
            "S3 returned a malformed checksum header",
        ));
    }
    Ok(value)
}

pub(super) fn required_header(
    headers: &HeaderMap,
    name: &'static str,
    missing_message: &'static str,
) -> Result<String, S3Error> {
    optional_header(headers, name)?.ok_or_else(|| S3Error::invalid_response(missing_message))
}

pub(super) fn optional_header(headers: &HeaderMap, name: &str) -> Result<Option<String>, S3Error> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .ok()
                .filter(|value| {
                    !value.is_empty()
                        && value.len() <= 2_048
                        && value.bytes().all(|byte| !byte.is_ascii_control())
                })
                .map(str::to_owned)
                .ok_or_else(|| S3Error::invalid_response("S3 returned an invalid response header"))
        })
        .transpose()
}

pub(super) fn request_ids(headers: &HeaderMap) -> RequestIds {
    RequestIds {
        request_id: diagnostic_header(headers, "x-amz-request-id"),
        host_id: diagnostic_header(headers, "x-amz-id-2"),
    }
}

fn diagnostic_header(headers: &HeaderMap, name: &'static str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 1_024
                && value.bytes().all(|byte| !byte.is_ascii_control())
        })
        .map(str::to_owned)
}
