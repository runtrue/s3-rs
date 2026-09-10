use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use futures_util::TryStreamExt;
use http::header::{CONTENT_LENGTH, CONTENT_TYPE, ETAG, IF_MATCH, RANGE};
use http::{HeaderName, Method};
use http_body_util::BodyExt;
use md5::{Digest as _, Md5};

use super::super::S3Client;
use super::super::request::{protocol_error, request_headers};
use super::headers::{
    copy_source_header, insert_conditions, insert_header, insert_optional_header,
    insert_upload_checksum, insert_user_metadata, merge_checksum, optional_query,
    parse_bool_header, parse_checksum, parse_content_range, parse_object_metadata,
    parse_request_ids, parse_u64_value, response_header, verified_download_sha256,
};
use crate::error::S3Error;
use crate::operation::{
    CopyMetadataDirective, CopyObjectOutput, CopyObjectRequest, DeleteObjectOutput,
    DeleteObjectRequest, DeleteObjectsOutput, DeleteObjectsRequest, GetObjectOutput,
    GetObjectRequest, HeadObjectOutput, HeadObjectRequest, PutObjectOutput, PutObjectRequest,
};
use crate::protocol::{
    CopyObjectResponse, ParsedS3Error, ProtocolError, parse_copy_object, parse_delete_objects,
    serialize_delete_objects,
};
use crate::stream::{ByteStream, ResponseStream};

impl S3Client {
    /// Uploads one object to the configured bucket.
    ///
    /// # Errors
    ///
    /// Returns an error when request preparation, signing, transport, or response parsing fails.
    pub async fn put_object(&self, request: PutObjectRequest) -> Result<PutObjectOutput, S3Error> {
        let deadline = self.deadline();
        let PutObjectRequest {
            key,
            body,
            content_type,
            user_metadata,
            conditions,
            checksum_algorithm,
            headers,
        } = request;
        let prepared = deadline.prepare_body(body).await?;
        let mut headers = request_headers(headers)?;
        insert_optional_header(&mut headers, CONTENT_TYPE, content_type.as_deref())?;
        insert_conditions(&mut headers, &conditions, "")?;
        insert_user_metadata(&mut headers, &user_metadata)?;
        insert_upload_checksum(&mut headers, checksum_algorithm, &prepared)?;
        let target = self.operation_target(Some(key.as_str()))?;
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
    pub async fn get_object(&self, request: GetObjectRequest) -> Result<GetObjectOutput, S3Error> {
        let mut headers = request_headers(request.headers)?;
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
        let target = self.operation_target(Some(request.key.as_str()))?;
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
        let mut headers = request_headers(request.headers)?;
        insert_conditions(&mut headers, &request.conditions, "")?;
        insert_header(
            &mut headers,
            HeaderName::from_static("x-amz-checksum-mode"),
            "ENABLED",
        )?;
        let query = optional_query("versionId", request.version_id.as_deref());
        let target = self.operation_target(Some(request.key.as_str()))?;
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
        let mut headers = request_headers(request.headers)?;
        insert_optional_header(&mut headers, IF_MATCH, request.if_match.as_deref())?;
        let query = optional_query("versionId", request.version_id.as_deref());
        let target = self.operation_target(Some(request.key.as_str()))?;
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
        let deadline = self.deadline();
        let maximum = self.inner.config.max_xml_response_size();
        let xml = serialize_delete_objects(&request, maximum).map_err(protocol_error)?;
        let digest = Md5::digest(&xml);
        let content_md5 = BASE64_STANDARD.encode(digest);
        let prepared = deadline.prepare_body(ByteStream::from_bytes(xml)).await?;
        let mut headers = request_headers(request.headers)?;
        insert_header(&mut headers, CONTENT_TYPE, "application/xml")?;
        insert_header(
            &mut headers,
            HeaderName::from_static("content-md5"),
            &content_md5,
        )?;
        let target = self.operation_target(None)?;
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
        let mut headers = request_headers(request.headers)?;
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
        match &request.metadata {
            CopyMetadataDirective::Copy => {}
            CopyMetadataDirective::Replace {
                content_type,
                user_metadata,
            } => {
                insert_optional_header(&mut headers, CONTENT_TYPE, content_type.as_deref())?;
                insert_user_metadata(&mut headers, user_metadata)?;
                insert_header(
                    &mut headers,
                    HeaderName::from_static("x-amz-metadata-directive"),
                    "REPLACE",
                )?;
            }
        }
        let target = self.operation_target(Some(request.destination.as_str()))?;
        let deadline = self.deadline();
        let maximum = self.inner.config.max_xml_response_size();
        let response = self
            .send_signed_collected_xml(
                Method::PUT,
                target,
                &[],
                headers,
                None,
                &deadline,
                maximum,
                copy_embedded_error,
            )
            .await?;
        let response_headers = response.headers;
        let mut output = match parse_copy_object(&response.body, maximum).map_err(protocol_error)? {
            CopyObjectResponse::Complete(output) => output,
            CopyObjectResponse::EmbeddedError(_) => {
                unreachable!("embedded copy errors are handled by request execution")
            }
        };
        output.version_id = response_header(&response_headers, "x-amz-version-id")?;
        output.request_ids = parse_request_ids(&response_headers)?;
        merge_checksum(&mut output.checksum, parse_checksum(&response_headers)?);
        Ok(output)
    }
}

fn copy_embedded_error(
    body: &[u8],
    maximum: usize,
) -> Result<Option<ParsedS3Error>, ProtocolError> {
    match parse_copy_object(body, maximum)? {
        CopyObjectResponse::Complete(_) => Ok(None),
        CopyObjectResponse::EmbeddedError(error) => Ok(Some(error)),
    }
}
