use http::header::{CONTENT_TYPE, ETAG};
use http::{HeaderMap, HeaderValue, Method};

use super::errors::embedded_complete_error;
use super::headers::{
    checksum_headers, create_headers, optional_header, request_ids, required_header,
    response_checksums,
};
use super::query::{create_query, list_query, upload_part_query, upload_query};
use crate::client::S3Client;
use crate::error::S3Error;
use crate::operation::{
    AbortMultipartUploadRequest, CompleteMultipartUploadOutput, CompleteMultipartUploadRequest,
    CreateMultipartUploadOutput, CreateMultipartUploadRequest, ListMultipartUploadsOutput,
    ListMultipartUploadsRequest, RequestIds, UploadPartOutput, UploadPartRequest,
};
use crate::protocol::{
    CompleteMultipartResponse, parse_complete_multipart_upload, parse_create_multipart_upload,
    parse_list_multipart_uploads, serialize_complete_multipart_upload,
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
        let target = self.multipart_object_target(Some(request.key.as_str()))?;
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
        request: UploadPartRequest<ByteStream>,
    ) -> Result<UploadPartOutput, S3Error> {
        let target = self.multipart_object_target(Some(request.key().as_str()))?;
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
        let target = self.multipart_object_target(Some(request.key.as_str()))?;
        let query = upload_query(request.upload_id());
        let document =
            serialize_complete_multipart_upload(&request, self.config().max_xml_response_size())
                .map_err(crate::client::request::protocol_error)?;
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
            .map_err(crate::client::request::protocol_error)?
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
        let target = self.multipart_object_target(Some(request.key().as_str()))?;
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
        let target = self.multipart_object_target(None)?;
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
            .map_err(crate::client::request::protocol_error)?;
        output.request_ids = request_ids;
        Ok(output)
    }

    fn multipart_object_target(
        &self,
        key: Option<&str>,
    ) -> Result<crate::endpoint::EndpointUrl, S3Error> {
        self.config().endpoint().object_url(
            self.config().bucket(),
            key,
            self.config().addressing_style(),
        )
    }
}
