use http::header::{CONTENT_TYPE, ETAG};
use http::{HeaderMap, HeaderValue, Method};

use super::headers::{
    checksum_headers, create_headers, optional_header, request_ids, required_header,
    response_checksums,
};
use super::query::{create_query, list_query, upload_part_query, upload_query};
use crate::client::S3Client;
use crate::client::request::OperationDeadline;
use crate::error::S3Error;
use crate::operation::{
    AbortMultipartUploadRequest, CompleteMultipartUploadOutput, CompleteMultipartUploadRequest,
    CreateMultipartUploadOutput, CreateMultipartUploadRequest, ListMultipartUploadsOutput,
    ListMultipartUploadsRequest, RequestIds, UploadPartOutput, UploadPartRequest,
};
use crate::protocol::{
    CompleteMultipartResponse, ParsedS3Error, ProtocolError, parse_complete_multipart_upload,
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
        let deadline = self.deadline();
        self.create_multipart_upload_with_deadline(request, &deadline)
            .await
    }

    pub(in crate::client) async fn create_multipart_upload_with_deadline(
        &self,
        request: CreateMultipartUploadRequest,
        deadline: &OperationDeadline,
    ) -> Result<CreateMultipartUploadOutput, S3Error> {
        let target = self.operation_target(Some(request.key.as_str()))?;
        let headers = create_headers(&request)?;
        let response = self
            .send_signed(
                Method::POST,
                target,
                &create_query(),
                headers,
                None,
                deadline,
            )
            .await?;
        let request_ids = request_ids(response.headers());
        let body = self
            .collect_response(response, self.config().max_xml_response_size(), deadline)
            .await?;
        let mut output =
            parse_create_multipart_upload(&body, self.config().max_xml_response_size())
                .map_err(crate::client::request::protocol_error)?;
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
        request: UploadPartRequest,
    ) -> Result<UploadPartOutput, S3Error> {
        let deadline = self.deadline();
        self.upload_part_with_deadline(request, &deadline).await
    }

    pub(in crate::client) async fn upload_part_with_deadline(
        &self,
        request: UploadPartRequest,
        deadline: &OperationDeadline,
    ) -> Result<UploadPartOutput, S3Error> {
        let target = self.operation_target(Some(request.key().as_str()))?;
        let query = upload_part_query(request.part_number().get(), request.upload_id());
        let headers = checksum_headers(request.checksum())?;
        let part_number = request.part_number();
        let body = deadline.prepare_body(request.into_body()).await?;
        let response = self
            .send_signed(Method::PUT, target, &query, headers, Some(&body), deadline)
            .await?;
        let headers = response.headers().clone();
        self.drain_success_response(response, deadline).await?;
        let e_tag = required_header(
            &headers,
            ETAG.as_str(),
            "upload-part response has no usable ETag",
        )?;
        let checksum = response_checksums(&headers)?;
        let request_ids = request_ids(&headers);
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
        let deadline = self.deadline();
        self.complete_multipart_upload_with_deadline(request, &deadline)
            .await
    }

    pub(in crate::client) async fn complete_multipart_upload_with_deadline(
        &self,
        request: CompleteMultipartUploadRequest,
        deadline: &OperationDeadline,
    ) -> Result<CompleteMultipartUploadOutput, S3Error> {
        let target = self.operation_target(Some(request.key.as_str()))?;
        let query = upload_query(request.upload_id());
        let document =
            serialize_complete_multipart_upload(&request, self.config().max_xml_response_size())
                .map_err(crate::client::request::protocol_error)?;
        let body = deadline.prepare_body(ByteStream::from(document)).await?;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/xml"));
        let maximum = self.config().max_xml_response_size();
        let response = self
            .send_signed_collected_xml(
                Method::POST,
                target,
                &query,
                headers,
                Some(&body),
                deadline,
                maximum,
                complete_multipart_embedded_error,
            )
            .await?;
        let response_headers = response.headers;
        let request_ids = request_ids(&response_headers);
        let version_id = optional_header(&response_headers, "x-amz-version-id")?;
        match parse_complete_multipart_upload(&response.body, maximum)
            .map_err(crate::client::request::protocol_error)?
        {
            CompleteMultipartResponse::Complete(mut output) => {
                output.version_id = version_id;
                output.request_ids = request_ids;
                Ok(output)
            }
            CompleteMultipartResponse::EmbeddedError(_) => {
                unreachable!("embedded completion errors are handled by request execution")
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
        let deadline = self.deadline();
        self.abort_multipart_upload_with_deadline(request, &deadline)
            .await
    }

    pub(in crate::client) async fn abort_multipart_upload_with_deadline(
        &self,
        request: AbortMultipartUploadRequest,
        deadline: &OperationDeadline,
    ) -> Result<RequestIds, S3Error> {
        let target = self.operation_target(Some(request.key().as_str()))?;
        let query = upload_query(request.upload_id());
        let response = self
            .send_signed(
                Method::DELETE,
                target,
                &query,
                HeaderMap::new(),
                None,
                deadline,
            )
            .await?;
        let request_ids = request_ids(response.headers());
        self.drain_success_response(response, deadline).await?;
        Ok(request_ids)
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
        let deadline = self.deadline();
        self.list_multipart_uploads_with_deadline(&request, &deadline)
            .await
    }

    pub(in crate::client) async fn list_multipart_uploads_with_deadline(
        &self,
        request: &ListMultipartUploadsRequest,
        deadline: &OperationDeadline,
    ) -> Result<ListMultipartUploadsOutput, S3Error> {
        let target = self.operation_target(None)?;
        let query = list_query(request)?;
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
        let request_ids = request_ids(response.headers());
        let body = self
            .collect_response(response, self.config().max_xml_response_size(), deadline)
            .await?;
        let mut output = parse_list_multipart_uploads(&body, self.config().max_xml_response_size())
            .map_err(crate::client::request::protocol_error)?;
        output.request_ids = request_ids;
        Ok(output)
    }
}

fn complete_multipart_embedded_error(
    body: &[u8],
    maximum: usize,
) -> Result<Option<ParsedS3Error>, ProtocolError> {
    match parse_complete_multipart_upload(body, maximum)? {
        CompleteMultipartResponse::Complete(_) => Ok(None),
        CompleteMultipartResponse::EmbeddedError(error) => Ok(Some(error)),
    }
}
