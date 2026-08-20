use http::{HeaderName, Method};

use crate::client::S3Client;
use crate::client::object::{copy_source_header, insert_conditions};
use crate::client::request::{insert_header, protocol_error, request_headers};
use crate::error::S3Error;
use crate::operation::{UploadPartCopyOutput, UploadPartCopyRequest};
use crate::protocol::{
    ParsedS3Error, ProtocolError, UploadPartCopyResponse, parse_upload_part_copy,
};

use super::headers::request_ids;
use super::query::upload_part_query;

impl S3Client {
    /// Uploads one multipart part by copying an existing S3 object or range.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid headers, transport or service failures,
    /// malformed bounded XML, embedded S3 errors, or a missing part ETag.
    pub async fn upload_part_copy(
        &self,
        request: UploadPartCopyRequest,
    ) -> Result<UploadPartCopyOutput, S3Error> {
        let mut headers = request_headers(request.headers.clone())?;
        insert_header(
            &mut headers,
            HeaderName::from_static("x-amz-copy-source"),
            &copy_source_header(&request.source),
        )?;
        if let Some(range) = request.source_range {
            insert_header(
                &mut headers,
                HeaderName::from_static("x-amz-copy-source-range"),
                &range.header_value(),
            )?;
        }
        insert_conditions(
            &mut headers,
            &request.source_conditions,
            "x-amz-copy-source-",
        )?;
        let target = self.operation_target(Some(request.destination().as_str()))?;
        let query = upload_part_query(request.part_number().get(), request.upload_id());
        let deadline = self.deadline();
        let maximum = self.config().max_xml_response_size();
        let response = self
            .send_signed_collected_xml(
                Method::PUT,
                target,
                &query,
                headers,
                None,
                &deadline,
                maximum,
                upload_part_copy_embedded_error,
            )
            .await?;
        match parse_upload_part_copy(&response.body, maximum).map_err(protocol_error)? {
            UploadPartCopyResponse::Complete(result) => Ok(UploadPartCopyOutput {
                part_number: request.part_number(),
                e_tag: result.e_tag,
                last_modified: result.last_modified,
                checksum: result.checksum,
                request_ids: request_ids(&response.headers),
            }),
            UploadPartCopyResponse::EmbeddedError(_) => {
                unreachable!("embedded part-copy errors are handled by request execution")
            }
        }
    }
}

fn upload_part_copy_embedded_error(
    body: &[u8],
    maximum: usize,
) -> Result<Option<ParsedS3Error>, ProtocolError> {
    match parse_upload_part_copy(body, maximum)? {
        UploadPartCopyResponse::Complete(_) => Ok(None),
        UploadPartCopyResponse::EmbeddedError(error) => Ok(Some(error)),
    }
}
