use http::header::{CONTENT_TYPE, ETAG};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};

use super::S3Client;
use crate::error::{ErrorCategory, RetryClassification, S3Error};
use crate::operation::{
    AbortMultipartUploadRequest, Checksum, ChecksumAlgorithm, CompleteMultipartUploadOutput,
    CompleteMultipartUploadRequest, CreateMultipartUploadOutput, CreateMultipartUploadRequest,
    ListMultipartUploadsOutput, ListMultipartUploadsRequest, RequestIds, UploadId,
    UploadPartOutput, UploadPartRequest,
};
use crate::protocol::{
    CompleteMultipartResponse, ParsedS3Error, parse_complete_multipart_upload,
    parse_create_multipart_upload, parse_list_multipart_uploads,
    serialize_complete_multipart_upload,
};
use crate::stream::ByteStream;

impl S3Client {
    /// Initiates a multipart upload for an object.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid headers, transport or service failures, or
    /// a malformed or oversized service response.
    pub async fn create_multipart_upload(
        &self,
        request: CreateMultipartUploadRequest,
    ) -> Result<CreateMultipartUploadOutput, S3Error> {
        let target = self.object_target(Some(request.key.as_str()))?;
        let headers = create_headers(&request)?;
        let deadline = self.deadline();
        let response = self
            .send_signed(
                Method::POST,
                target,
                &create_query(),
                headers,
                None,
                &deadline,
            )
            .await?;
        let request_ids = request_ids(response.headers());
        let body = self
            .collect_response(response, self.config().max_xml_response_size(), &deadline)
            .await?;
        let mut output =
            parse_create_multipart_upload(&body, self.config().max_xml_response_size())
                .map_err(super::request::protocol_error)?;
        output.request_ids = request_ids;
        Ok(output)
    }

    /// Uploads one part of an in-progress multipart upload.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid checksum, body preparation failure,
    /// transport or service failure, or malformed response headers.
    pub async fn upload_part(
        &self,
        request: UploadPartRequest<ByteStream>,
    ) -> Result<UploadPartOutput, S3Error> {
        let target = self.object_target(Some(request.key().as_str()))?;
        let query = upload_part_query(request.part_number().get(), request.upload_id());
        let headers = checksum_headers(request.checksum())?;
        let part_number = request.part_number();
        let body = request.into_body().prepare().await?;
        let deadline = self.deadline();
        let response = self
            .send_signed(Method::PUT, target, &query, headers, Some(&body), &deadline)
            .await?;
        let headers = response.headers();
        let e_tag = required_header(
            headers,
            ETAG.as_str(),
            "upload-part response has no usable ETag",
        )?;
        let checksum = response_checksums(headers)?;
        let request_ids = request_ids(headers);
        Ok(UploadPartOutput {
            part_number,
            e_tag,
            checksum,
            request_ids,
        })
    }

    /// Completes an in-progress multipart upload.
    ///
    /// # Errors
    ///
    /// Returns an error when the completion document cannot be serialized, the
    /// request fails, or S3 returns a malformed, oversized, or embedded error.
    pub async fn complete_multipart_upload(
        &self,
        request: CompleteMultipartUploadRequest,
    ) -> Result<CompleteMultipartUploadOutput, S3Error> {
        let target = self.object_target(Some(request.key.as_str()))?;
        let query = upload_query(request.upload_id());
        let document =
            serialize_complete_multipart_upload(&request, self.config().max_xml_response_size())
                .map_err(super::request::protocol_error)?;
        let body = ByteStream::from(document).prepare().await?;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/xml"));
        let deadline = self.deadline();
        let response = self
            .send_signed(
                Method::POST,
                target,
                &query,
                headers,
                Some(&body),
                &deadline,
            )
            .await?;
        let response_headers = response.headers().clone();
        let request_ids = request_ids(&response_headers);
        let version_id = optional_header(&response_headers, "x-amz-version-id")?;
        let body = self
            .collect_response(response, self.config().max_xml_response_size(), &deadline)
            .await?;
        match parse_complete_multipart_upload(&body, self.config().max_xml_response_size())
            .map_err(super::request::protocol_error)?
        {
            CompleteMultipartResponse::Complete(mut output) => {
                output.version_id = version_id;
                output.request_ids = request_ids;
                Ok(output)
            }
            CompleteMultipartResponse::EmbeddedError(parsed) => {
                Err(embedded_complete_error(&response_headers, parsed))
            }
        }
    }

    /// Aborts an in-progress multipart upload.
    ///
    /// # Errors
    ///
    /// Returns an error when target construction, signing, transport, or the
    /// S3 service rejects the abort request.
    pub async fn abort_multipart_upload(
        &self,
        request: AbortMultipartUploadRequest,
    ) -> Result<RequestIds, S3Error> {
        let target = self.object_target(Some(request.key().as_str()))?;
        let query = upload_query(request.upload_id());
        let deadline = self.deadline();
        let response = self
            .send_signed(
                Method::DELETE,
                target,
                &query,
                HeaderMap::new(),
                None,
                &deadline,
            )
            .await?;
        Ok(request_ids(response.headers()))
    }

    /// Retrieves one bounded page of in-progress multipart uploads.
    ///
    /// # Errors
    ///
    /// Returns an error for inconsistent pagination markers, a failed request,
    /// or a malformed or oversized listing response.
    pub async fn list_multipart_uploads(
        &self,
        request: ListMultipartUploadsRequest,
    ) -> Result<ListMultipartUploadsOutput, S3Error> {
        let target = self.object_target(None)?;
        let query = list_query(&request)?;
        let deadline = self.deadline();
        let response = self
            .send_signed(
                Method::GET,
                target,
                &query,
                HeaderMap::new(),
                None,
                &deadline,
            )
            .await?;
        let request_ids = request_ids(response.headers());
        let body = self
            .collect_response(response, self.config().max_xml_response_size(), &deadline)
            .await?;
        let mut output = parse_list_multipart_uploads(&body, self.config().max_xml_response_size())
            .map_err(super::request::protocol_error)?;
        output.request_ids = request_ids;
        Ok(output)
    }

    fn object_target(&self, key: Option<&str>) -> Result<crate::endpoint::EndpointUrl, S3Error> {
        self.config().endpoint().object_url(
            self.config().bucket(),
            key,
            self.config().addressing_style(),
        )
    }
}

fn create_query() -> Vec<(String, String)> {
    vec![("uploads".to_owned(), String::new())]
}

fn upload_query(upload_id: &UploadId) -> Vec<(String, String)> {
    vec![("uploadId".to_owned(), upload_id.as_str().to_owned())]
}

fn upload_part_query(part_number: u16, upload_id: &UploadId) -> Vec<(String, String)> {
    vec![
        ("partNumber".to_owned(), part_number.to_string()),
        ("uploadId".to_owned(), upload_id.as_str().to_owned()),
    ]
}

fn list_query(request: &ListMultipartUploadsRequest) -> Result<Vec<(String, String)>, S3Error> {
    if request.upload_id_marker.is_some() && request.key_marker.is_none() {
        return Err(S3Error::configuration(
            "an upload-ID marker requires a key marker",
        ));
    }
    let mut query = create_query();
    if let Some(delimiter) = &request.delimiter {
        query.push(("delimiter".to_owned(), delimiter.clone()));
    }
    if let Some(key_marker) = &request.key_marker {
        query.push(("key-marker".to_owned(), key_marker.clone()));
    }
    query.push((
        "max-uploads".to_owned(),
        request.max_uploads.get().to_string(),
    ));
    if let Some(prefix) = &request.prefix {
        query.push(("prefix".to_owned(), prefix.clone()));
    }
    if let Some(upload_id_marker) = &request.upload_id_marker {
        query.push((
            "upload-id-marker".to_owned(),
            upload_id_marker.as_str().to_owned(),
        ));
    }
    Ok(query)
}

fn create_headers(request: &CreateMultipartUploadRequest) -> Result<HeaderMap, S3Error> {
    let mut headers = HeaderMap::new();
    if let Some(content_type) = &request.content_type {
        insert_header(&mut headers, &CONTENT_TYPE, content_type)?;
    }
    if let Some(algorithm) = request.checksum_algorithm {
        headers.insert(
            HeaderName::from_static("x-amz-checksum-algorithm"),
            HeaderValue::from_static(checksum_algorithm_name(algorithm)),
        );
    }
    for (name, value) in &request.user_metadata {
        if name.is_empty() {
            return Err(S3Error::configuration(
                "user metadata names cannot be empty",
            ));
        }
        let name = HeaderName::from_bytes(format!("x-amz-meta-{name}").as_bytes())
            .map_err(|_| S3Error::configuration("user metadata name is not a valid HTTP header"))?;
        insert_header(&mut headers, &name, value)?;
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

fn checksum_headers(checksum: &Checksum) -> Result<HeaderMap, S3Error> {
    let mut headers = HeaderMap::new();
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
    insert_header(headers, &HeaderName::from_static(name), value)
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

fn insert_header(headers: &mut HeaderMap, name: &HeaderName, value: &str) -> Result<(), S3Error> {
    let value = HeaderValue::from_str(value)
        .map_err(|_| S3Error::configuration("request header value is invalid"))?;
    headers.insert(name.clone(), value);
    Ok(())
}

fn response_checksums(headers: &HeaderMap) -> Result<Checksum, S3Error> {
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

fn required_header(
    headers: &HeaderMap,
    name: &'static str,
    missing_message: &'static str,
) -> Result<String, S3Error> {
    optional_header(headers, name)?.ok_or_else(|| S3Error::invalid_response(missing_message))
}

fn optional_header(headers: &HeaderMap, name: &str) -> Result<Option<String>, S3Error> {
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

fn request_ids(headers: &HeaderMap) -> RequestIds {
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

fn embedded_complete_error(headers: &HeaderMap, parsed: ParsedS3Error) -> S3Error {
    let category = match parsed.code.as_deref() {
        Some("AccessDenied" | "AllAccessDisabled") => ErrorCategory::Authorization,
        Some("NoSuchBucket" | "NoSuchUpload" | "NotFound") => ErrorCategory::NotFound,
        Some("SlowDown" | "Throttling" | "ThrottlingException") => ErrorCategory::Throttling,
        Some("InternalError" | "ServiceUnavailable") => ErrorCategory::Server,
        _ => ErrorCategory::InvalidResponse,
    };
    let retry = match category {
        ErrorCategory::Throttling => RetryClassification::Throttled,
        ErrorCategory::Server => RetryClassification::Retryable,
        _ => RetryClassification::Never,
    };
    let message = parsed
        .message
        .as_deref()
        .and_then(safe_service_message)
        .unwrap_or_else(|| "S3 returned an embedded completion error".to_owned());
    let ids = request_ids(headers);
    S3Error::new(category, message, retry).with_service_details(
        parsed.code,
        StatusCode::OK,
        ids.request_id.or(parsed.request_id),
        ids.host_id.or(parsed.host_id),
    )
}

fn safe_service_message(message: &str) -> Option<String> {
    if message.is_empty()
        || message.len() > 512
        || message.contains("X-Amz-")
        || message.to_ascii_lowercase().contains("authorization")
        || message.to_ascii_lowercase().contains("token")
        || !message.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || character.is_ascii_whitespace()
                || matches!(
                    character,
                    '.' | ',' | ':' | ';' | '_' | '-' | '\'' | '(' | ')'
                )
        })
    {
        return None;
    }
    Some(
        message
            .split_ascii_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::{ObjectKey, PageSize};
    use crate::signing::{QueryParam, canonical_query};

    fn encoded(query: &[(String, String)]) -> String {
        canonical_query(
            &query
                .iter()
                .map(|(name, value)| QueryParam::new(name, value))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn upload_queries_encode_opaque_upload_ids_exactly_once() {
        let upload_id = UploadId::new("id+/= value").unwrap();
        assert_eq!(
            encoded(&upload_part_query(7, &upload_id)),
            "partNumber=7&uploadId=id%2B%2F%3D%20value"
        );
        assert_eq!(
            encoded(&upload_query(&upload_id)),
            "uploadId=id%2B%2F%3D%20value"
        );
    }

    #[test]
    fn list_query_contains_paired_resume_markers_and_bounds() {
        let request = ListMultipartUploadsRequest {
            prefix: Some("a b/".to_owned()),
            delimiter: Some("/".to_owned()),
            key_marker: Some("last+key".to_owned()),
            upload_id_marker: Some(UploadId::new("upload/id=").unwrap()),
            max_uploads: PageSize::new(17).unwrap(),
        };
        assert_eq!(
            encoded(&list_query(&request).unwrap()),
            "delimiter=%2F&key-marker=last%2Bkey&max-uploads=17&prefix=a%20b%2F&upload-id-marker=upload%2Fid%3D&uploads="
        );

        let invalid = ListMultipartUploadsRequest {
            upload_id_marker: Some(UploadId::new("upload-id").unwrap()),
            ..ListMultipartUploadsRequest::default()
        };
        assert!(list_query(&invalid).is_err());
    }

    #[test]
    fn request_header_state_is_validated_before_transport() {
        let mut request = CreateMultipartUploadRequest::new(ObjectKey::new("key").unwrap());
        request
            .user_metadata
            .insert(String::new(), "value".to_owned());
        assert!(create_headers(&request).is_err());

        let checksum = Checksum {
            sha256: Some("not-base64".to_owned()),
            ..Checksum::default()
        };
        assert!(checksum_headers(&checksum).is_err());
        let checksum = Checksum {
            crc32: Some("AAAAAA==".to_owned()),
            ..Checksum::default()
        };
        assert_eq!(
            checksum_headers(&checksum)
                .unwrap()
                .get("x-amz-checksum-crc32")
                .unwrap(),
            "AAAAAA=="
        );
    }
}
