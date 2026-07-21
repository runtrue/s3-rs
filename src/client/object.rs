use std::collections::{BTreeMap, HashSet};
use std::time::SystemTime;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use futures_util::TryStreamExt;
use http::header::{CONTENT_LENGTH, CONTENT_TYPE, ETAG, IF_MATCH, LAST_MODIFIED, RANGE};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use http_body_util::BodyExt;
use md5::{Digest as _, Md5};

use super::S3Client;
use super::request::{OperationDeadline, protocol_error, service_error};
use crate::error::{ErrorCategory, RetryClassification, S3Error};
use crate::operation::{
    Checksum, ChecksumAlgorithm, Conditions, CopyObjectOutput, CopyObjectRequest,
    DeleteObjectOutput, DeleteObjectRequest, DeleteObjectsOutput, DeleteObjectsRequest,
    GetObjectOutput, GetObjectRequest, HeadObjectOutput, HeadObjectRequest, ListObjectsV2Output,
    ListObjectsV2Request, ObjectMetadata, PutObjectOutput, PutObjectRequest, RequestIds,
};
use crate::protocol::{
    CopyObjectResponse, parse_copy_object, parse_delete_objects, parse_list_objects_v2,
    serialize_delete_objects,
};
use crate::stream::{ByteStream, PreparedBody, ResponseStream};

impl S3Client {
    /// Uploads one object to the configured bucket.
    ///
    /// # Errors
    ///
    /// Returns an error when request preparation, signing, transport, or response parsing fails.
    pub async fn put_object(
        &self,
        request: PutObjectRequest<ByteStream>,
    ) -> Result<PutObjectOutput, S3Error> {
        let PutObjectRequest {
            key,
            body,
            content_type,
            user_metadata,
            conditions,
            checksum_algorithm,
        } = request;
        let prepared = body.prepare().await?;
        let mut headers = HeaderMap::new();
        insert_optional_header(&mut headers, CONTENT_TYPE, content_type.as_deref())?;
        insert_conditions(&mut headers, &conditions, "")?;
        insert_user_metadata(&mut headers, &user_metadata)?;
        insert_upload_checksum(&mut headers, checksum_algorithm, &prepared)?;
        let target = self.object_operation_target(Some(key.as_str()))?;
        let deadline = self.deadline();
        let response = self
            .send_signed(
                Method::PUT,
                target,
                &[],
                headers,
                Some(&prepared),
                &deadline,
            )
            .await?;
        let response_headers = response.headers().clone();
        self.collect_response(
            response,
            self.inner.config.max_xml_response_size(),
            &deadline,
        )
        .await?;
        Ok(PutObjectOutput {
            e_tag: response_header(&response_headers, ETAG.as_str())?,
            version_id: response_header(&response_headers, "x-amz-version-id")?,
            checksum: parse_checksum(&response_headers)?,
            request_ids: parse_request_ids(&response_headers)?,
        })
    }

    /// Downloads one object as a bounded, cancellation-safe response stream.
    ///
    /// # Errors
    ///
    /// Returns an error when request validation, signing, transport, or header parsing fails.
    pub async fn get_object(
        &self,
        request: GetObjectRequest,
    ) -> Result<GetObjectOutput<ResponseStream>, S3Error> {
        let mut headers = HeaderMap::new();
        insert_conditions(&mut headers, &request.conditions, "")?;
        insert_header(
            &mut headers,
            HeaderName::from_static("x-amz-checksum-mode"),
            "ENABLED",
        )?;
        if let Some(range) = request.range {
            insert_header(&mut headers, RANGE, &range.to_header_value())?;
        }
        let query = optional_query("versionId", request.version_id.as_deref());
        let target = self.object_operation_target(Some(request.key.as_str()))?;
        let deadline = self.deadline();
        let response = self
            .send_signed(Method::GET, target, &query, headers, None, &deadline)
            .await?;
        let metadata = parse_object_metadata(response.headers())?;
        let content_range = parse_content_range(response.headers())?;
        let expected_sha256 =
            verified_download_sha256(response.headers())?.filter(|_| content_range.is_none());
        let expected_length = response
            .headers()
            .get(CONTENT_LENGTH)
            .map(parse_u64_value)
            .transpose()?;
        let stream = response
            .into_body()
            .into_data_stream()
            .map_err(crate::transport::classify_response_body_error);
        let body = ResponseStream::with_deadline(
            stream,
            expected_length,
            expected_sha256,
            self.inner.config.idle_body_timeout(),
            deadline.instant(),
        );
        Ok(GetObjectOutput {
            metadata,
            body,
            content_range,
        })
    }

    /// Retrieves one object's metadata without downloading its body.
    ///
    /// # Errors
    ///
    /// Returns an error when signing, transport, or response parsing fails.
    pub async fn head_object(
        &self,
        request: HeadObjectRequest,
    ) -> Result<HeadObjectOutput, S3Error> {
        let mut headers = HeaderMap::new();
        insert_conditions(&mut headers, &request.conditions, "")?;
        insert_header(
            &mut headers,
            HeaderName::from_static("x-amz-checksum-mode"),
            "ENABLED",
        )?;
        let query = optional_query("versionId", request.version_id.as_deref());
        let target = self.object_operation_target(Some(request.key.as_str()))?;
        let deadline = self.deadline();
        let response = self
            .send_signed(Method::HEAD, target, &query, headers, None, &deadline)
            .await?;
        let metadata = parse_object_metadata(response.headers())?;
        self.collect_response(
            response,
            self.inner.config.max_xml_response_size(),
            &deadline,
        )
        .await?;
        Ok(metadata)
    }

    /// Deletes one object or object version.
    ///
    /// # Errors
    ///
    /// Returns an error when signing, transport, or response parsing fails.
    pub async fn delete_object(
        &self,
        request: DeleteObjectRequest,
    ) -> Result<DeleteObjectOutput, S3Error> {
        let mut headers = HeaderMap::new();
        insert_optional_header(&mut headers, IF_MATCH, request.if_match.as_deref())?;
        let query = optional_query("versionId", request.version_id.as_deref());
        let target = self.object_operation_target(Some(request.key.as_str()))?;
        let deadline = self.deadline();
        let response = self
            .send_signed(Method::DELETE, target, &query, headers, None, &deadline)
            .await?;
        let response_headers = response.headers().clone();
        self.collect_response(
            response,
            self.inner.config.max_xml_response_size(),
            &deadline,
        )
        .await?;
        Ok(DeleteObjectOutput {
            delete_marker: parse_bool_header(&response_headers, "x-amz-delete-marker")?
                .unwrap_or(false),
            version_id: response_header(&response_headers, "x-amz-version-id")?,
            request_ids: parse_request_ids(&response_headers)?,
        })
    }

    /// Deletes a validated batch of objects in one request.
    ///
    /// # Errors
    ///
    /// Returns an error when XML serialization, signing, transport, or response parsing fails.
    pub async fn delete_objects(
        &self,
        request: DeleteObjectsRequest,
    ) -> Result<DeleteObjectsOutput, S3Error> {
        let maximum = self.inner.config.max_xml_response_size();
        let xml = serialize_delete_objects(&request, maximum).map_err(protocol_error)?;
        let digest = Md5::digest(&xml);
        let content_md5 = BASE64_STANDARD.encode(digest);
        let prepared = ByteStream::from_bytes(xml).prepare().await?;
        let mut headers = HeaderMap::new();
        insert_header(&mut headers, CONTENT_TYPE, "application/xml")?;
        insert_header(
            &mut headers,
            HeaderName::from_static("content-md5"),
            &content_md5,
        )?;
        let target = self.object_operation_target(None)?;
        let deadline = self.deadline();
        let response = self
            .send_signed(
                Method::POST,
                target,
                &[("delete".to_owned(), String::new())],
                headers,
                Some(&prepared),
                &deadline,
            )
            .await?;
        let response_headers = response.headers().clone();
        let body = self.collect_response(response, maximum, &deadline).await?;
        let mut output = parse_delete_objects(&body, maximum).map_err(protocol_error)?;
        output.request_ids = parse_request_ids(&response_headers)?;
        Ok(output)
    }

    /// Copies an object entirely within S3.
    ///
    /// # Errors
    ///
    /// Returns an error when request validation, signing, transport, or response parsing fails.
    pub async fn copy_object(
        &self,
        request: CopyObjectRequest,
    ) -> Result<CopyObjectOutput, S3Error> {
        let mut headers = HeaderMap::new();
        insert_header(
            &mut headers,
            HeaderName::from_static("x-amz-copy-source"),
            &copy_source_header(&request.source),
        )?;
        insert_conditions(
            &mut headers,
            &request.source_conditions,
            "x-amz-copy-source-",
        )?;
        let replaces_metadata = request.content_type.is_some() || request.user_metadata.is_some();
        insert_optional_header(&mut headers, CONTENT_TYPE, request.content_type.as_deref())?;
        if let Some(metadata) = &request.user_metadata {
            insert_user_metadata(&mut headers, metadata)?;
        }
        if replaces_metadata {
            insert_header(
                &mut headers,
                HeaderName::from_static("x-amz-metadata-directive"),
                "REPLACE",
            )?;
        }
        let target = self.object_operation_target(Some(request.destination.as_str()))?;
        let deadline = self.deadline();
        let response = self
            .send_signed(Method::PUT, target, &[], headers, None, &deadline)
            .await?;
        let response_headers = response.headers().clone();
        let maximum = self.inner.config.max_xml_response_size();
        let body = self.collect_response(response, maximum, &deadline).await?;
        let mut output = match parse_copy_object(&body, maximum).map_err(protocol_error)? {
            CopyObjectResponse::Complete(output) => output,
            CopyObjectResponse::EmbeddedError(parsed) => {
                return Err(service_error(
                    StatusCode::OK,
                    &response_headers,
                    Some(parsed),
                ));
            }
        };
        output.version_id = response_header(&response_headers, "x-amz-version-id")?;
        output.request_ids = parse_request_ids(&response_headers)?;
        merge_checksum(&mut output.checksum, parse_checksum(&response_headers)?);
        Ok(output)
    }

    /// Retrieves one page from S3's version-two object listing API.
    ///
    /// # Errors
    ///
    /// Returns an error when signing, transport, or response parsing fails.
    pub async fn list_objects_v2(
        &self,
        request: ListObjectsV2Request,
    ) -> Result<ListObjectsV2Output, S3Error> {
        let deadline = self.deadline();
        self.list_objects_v2_with_deadline(&request, &deadline)
            .await
    }

    /// Retrieves at most `maximum_pages`, rejecting missing or repeating page tokens.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero page bound, a malformed pagination sequence, an exhausted
    /// page bound, or any page request failure.
    pub async fn list_objects_v2_all(
        &self,
        mut request: ListObjectsV2Request,
        maximum_pages: usize,
    ) -> Result<Vec<ListObjectsV2Output>, S3Error> {
        if maximum_pages == 0 {
            return Err(S3Error::configuration(
                "maximum listing page count must be greater than zero",
            ));
        }
        let deadline = self.deadline();
        let mut seen = HashSet::new();
        if let Some(token) = &request.continuation_token {
            seen.insert(token.clone());
        }
        let mut pages = Vec::with_capacity(maximum_pages.min(16));
        for _ in 0..maximum_pages {
            let page = self
                .list_objects_v2_with_deadline(&request, &deadline)
                .await?;
            let is_truncated = page.is_truncated;
            let next = page.next_continuation_token.clone();
            pages.push(page);
            if !is_truncated {
                return Ok(pages);
            }
            let next = next.ok_or_else(|| {
                S3Error::invalid_response(
                    "truncated listing response omitted its continuation token",
                )
            })?;
            if !seen.insert(next.clone()) {
                return Err(S3Error::invalid_response(
                    "listing response repeated a continuation token",
                ));
            }
            request.continuation_token = Some(next);
            request.start_after = None;
        }
        Err(S3Error::new(
            ErrorCategory::OversizedResponse,
            "listing exceeded the configured page limit",
            RetryClassification::Never,
        ))
    }

    async fn list_objects_v2_with_deadline(
        &self,
        request: &ListObjectsV2Request,
        deadline: &OperationDeadline,
    ) -> Result<ListObjectsV2Output, S3Error> {
        let mut query = vec![
            ("list-type".to_owned(), "2".to_owned()),
            ("max-keys".to_owned(), request.max_keys.get().to_string()),
        ];
        push_optional_query(&mut query, "prefix", request.prefix.as_deref());
        push_optional_query(&mut query, "delimiter", request.delimiter.as_deref());
        push_optional_query(
            &mut query,
            "continuation-token",
            request.continuation_token.as_deref(),
        );
        push_optional_query(
            &mut query,
            "start-after",
            request
                .start_after
                .as_ref()
                .map(crate::operation::ObjectKey::as_str),
        );
        if request.fetch_owner {
            query.push(("fetch-owner".to_owned(), "true".to_owned()));
        }
        let target = self.object_operation_target(None)?;
        let response = self
            .send_signed(
                Method::GET,
                target,
                &query,
                HeaderMap::new(),
                None,
                deadline,
            )
            .await?;
        let response_headers = response.headers().clone();
        let maximum = self.inner.config.max_xml_response_size();
        let body = self.collect_response(response, maximum, deadline).await?;
        let mut output = parse_list_objects_v2(&body, maximum).map_err(protocol_error)?;
        output.request_ids = parse_request_ids(&response_headers)?;
        Ok(output)
    }

    fn object_operation_target(
        &self,
        key: Option<&str>,
    ) -> Result<crate::endpoint::EndpointUrl, S3Error> {
        self.inner.config.endpoint().object_url(
            self.inner.config.bucket(),
            key,
            self.inner.config.addressing_style(),
        )
    }
}

fn insert_upload_checksum(
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

fn insert_conditions(
    headers: &mut HeaderMap,
    conditions: &Conditions,
    prefix: &str,
) -> Result<(), S3Error> {
    insert_named_optional(
        headers,
        &format!("{prefix}if-match"),
        conditions.if_match.as_deref(),
    )?;
    insert_named_optional(
        headers,
        &format!("{prefix}if-none-match"),
        conditions.if_none_match.as_deref(),
    )?;
    if let Some(value) = conditions.if_modified_since {
        insert_named(
            headers,
            &format!("{prefix}if-modified-since"),
            &format_http_date(value),
        )?;
    }
    if let Some(value) = conditions.if_unmodified_since {
        insert_named(
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

fn insert_user_metadata(
    headers: &mut HeaderMap,
    metadata: &BTreeMap<String, String>,
) -> Result<(), S3Error> {
    for (name, value) in metadata {
        insert_named(headers, &format!("x-amz-meta-{name}"), value)?;
    }
    Ok(())
}

fn insert_optional_header(
    headers: &mut HeaderMap,
    name: HeaderName,
    value: Option<&str>,
) -> Result<(), S3Error> {
    if let Some(value) = value {
        insert_header(headers, name, value)?;
    }
    Ok(())
}

fn insert_named_optional(
    headers: &mut HeaderMap,
    name: &str,
    value: Option<&str>,
) -> Result<(), S3Error> {
    if let Some(value) = value {
        insert_named(headers, name, value)?;
    }
    Ok(())
}

fn insert_named(headers: &mut HeaderMap, name: &str, value: &str) -> Result<(), S3Error> {
    let name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| S3Error::configuration("request contains an invalid header name"))?;
    insert_header(headers, name, value)
}

fn insert_header(headers: &mut HeaderMap, name: HeaderName, value: &str) -> Result<(), S3Error> {
    let value = HeaderValue::from_str(value)
        .map_err(|_| S3Error::configuration("request contains an invalid header value"))?;
    headers.insert(name, value);
    Ok(())
}

fn optional_query(name: &str, value: Option<&str>) -> Vec<(String, String)> {
    value.map_or_else(Vec::new, |value| vec![(name.to_owned(), value.to_owned())])
}

fn push_optional_query(query: &mut Vec<(String, String)>, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        query.push((name.to_owned(), value.to_owned()));
    }
}

fn parse_object_metadata(headers: &HeaderMap) -> Result<ObjectMetadata, S3Error> {
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

fn parse_checksum(headers: &HeaderMap) -> Result<Checksum, S3Error> {
    Ok(Checksum {
        crc32: response_header(headers, "x-amz-checksum-crc32")?,
        crc32c: response_header(headers, "x-amz-checksum-crc32c")?,
        crc64_nvme: response_header(headers, "x-amz-checksum-crc64nvme")?,
        sha1: response_header(headers, "x-amz-checksum-sha1")?,
        sha256: response_header(headers, "x-amz-checksum-sha256")?,
    })
}

fn verified_download_sha256(headers: &HeaderMap) -> Result<Option<[u8; 32]>, S3Error> {
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

fn merge_checksum(target: &mut Checksum, headers: Checksum) {
    target.crc32 = headers.crc32.or_else(|| target.crc32.take());
    target.crc32c = headers.crc32c.or_else(|| target.crc32c.take());
    target.crc64_nvme = headers.crc64_nvme.or_else(|| target.crc64_nvme.take());
    target.sha1 = headers.sha1.or_else(|| target.sha1.take());
    target.sha256 = headers.sha256.or_else(|| target.sha256.take());
}

fn parse_request_ids(headers: &HeaderMap) -> Result<RequestIds, S3Error> {
    Ok(RequestIds {
        request_id: response_header(headers, "x-amz-request-id")?,
        host_id: response_header(headers, "x-amz-id-2")?,
    })
}

fn response_header(headers: &HeaderMap, name: &str) -> Result<Option<String>, S3Error> {
    headers.get(name).map(response_header_value).transpose()
}

fn response_header_value(value: &HeaderValue) -> Result<String, S3Error> {
    value
        .to_str()
        .map(str::to_owned)
        .map_err(|_| S3Error::invalid_response("S3 returned a non-text response header"))
}

fn parse_u64_value(value: &HeaderValue) -> Result<u64, S3Error> {
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

fn parse_bool_header(headers: &HeaderMap, name: &str) -> Result<Option<bool>, S3Error> {
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

fn parse_content_range(headers: &HeaderMap) -> Result<Option<(u64, u64, Option<u64>)>, S3Error> {
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

fn copy_source_header(source: &crate::operation::CopySource) -> String {
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
    use sha2::Sha256;
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
