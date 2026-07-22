use std::collections::HashSet;

use http::{HeaderMap, Method};

use super::query::list_parts_query;
use crate::client::S3Client;
use crate::client::request::{OperationDeadline, protocol_error};
use crate::error::{ErrorCategory, RetryClassification, S3Error};
use crate::operation::{
    ListMultipartUploadsOutput, ListMultipartUploadsRequest, ListPartsOutput, ListPartsRequest,
};
use crate::protocol::parse_list_parts;

impl S3Client {
    /// Lists one bounded page of parts belonging to an in-progress upload.
    ///
    /// # Errors
    ///
    /// Returns an error when signing, transport, or bounded XML parsing fails.
    pub async fn list_parts(&self, request: ListPartsRequest) -> Result<ListPartsOutput, S3Error> {
        let deadline = self.deadline();
        self.list_parts_with_deadline(&request, &deadline).await
    }

    /// Retrieves at most `maximum_pages` of parts under one operation deadline.
    ///
    /// Missing, non-advancing, or repeated markers are rejected rather than
    /// risking an unbounded pagination loop.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero page bound, invalid pagination, exhaustion
    /// of the page bound, or any page request failure.
    pub async fn list_parts_all(
        &self,
        mut request: ListPartsRequest,
        maximum_pages: usize,
    ) -> Result<Vec<ListPartsOutput>, S3Error> {
        validate_page_bound(maximum_pages)?;
        let deadline = self.deadline();
        let mut seen = HashSet::new();
        if let Some(marker) = request.part_number_marker {
            seen.insert(marker.get());
        }
        let mut pages = Vec::with_capacity(maximum_pages.min(10));
        for _ in 0..maximum_pages {
            let page = self.list_parts_with_deadline(&request, &deadline).await?;
            let truncated = page.is_truncated;
            let next = page.next_part_number_marker;
            pages.push(page);
            if !truncated {
                return Ok(pages);
            }
            let next = next.ok_or_else(|| {
                S3Error::invalid_response("truncated ListParts response omitted its next marker")
            })?;
            if request
                .part_number_marker
                .is_some_and(|current| next <= current)
                || !seen.insert(next.get())
            {
                return Err(S3Error::invalid_response(
                    "ListParts response returned a non-advancing marker",
                ));
            }
            request.part_number_marker = Some(next);
        }
        Err(page_limit_error("ListParts"))
    }

    /// Retrieves at most `maximum_pages` of in-progress uploads under one deadline.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero page bound, invalid pagination, exhaustion
    /// of the page bound, or any page request failure.
    pub async fn list_multipart_uploads_all(
        &self,
        mut request: ListMultipartUploadsRequest,
        maximum_pages: usize,
    ) -> Result<Vec<ListMultipartUploadsOutput>, S3Error> {
        validate_page_bound(maximum_pages)?;
        let deadline = self.deadline();
        let mut seen = HashSet::new();
        if let Some(key) = &request.key_marker {
            seen.insert((key.clone(), request.upload_id_marker.clone()));
        }
        let mut pages = Vec::with_capacity(maximum_pages.min(16));
        for _ in 0..maximum_pages {
            let page = self
                .list_multipart_uploads_with_deadline(&request, &deadline)
                .await?;
            let truncated = page.is_truncated;
            let next_key = page.next_key_marker.clone();
            let next_upload_id = page.next_upload_id_marker.clone();
            pages.push(page);
            if !truncated {
                return Ok(pages);
            }
            let next_key = next_key.ok_or_else(|| {
                S3Error::invalid_response(
                    "truncated multipart-upload listing omitted its next key marker",
                )
            })?;
            if !seen.insert((next_key.clone(), next_upload_id.clone())) {
                return Err(S3Error::invalid_response(
                    "multipart-upload listing repeated its pagination markers",
                ));
            }
            request.key_marker = Some(next_key);
            request.upload_id_marker = next_upload_id;
        }
        Err(page_limit_error("multipart-upload listing"))
    }

    pub(in crate::client) async fn list_parts_with_deadline(
        &self,
        request: &ListPartsRequest,
        deadline: &OperationDeadline,
    ) -> Result<ListPartsOutput, S3Error> {
        let target = self.operation_target(Some(request.key().as_str()))?;
        let response = self
            .send_signed(
                Method::GET,
                target,
                &list_parts_query(request),
                HeaderMap::new(),
                None,
                deadline,
            )
            .await?;
        let response_headers = response.headers().clone();
        let maximum = self.config().max_xml_response_size();
        let body = self.collect_response(response, maximum, deadline).await?;
        let mut output = parse_list_parts(&body, maximum).map_err(protocol_error)?;
        output.request_ids = super::headers::request_ids(&response_headers);
        Ok(output)
    }
}

fn validate_page_bound(maximum_pages: usize) -> Result<(), S3Error> {
    if maximum_pages == 0 {
        return Err(S3Error::configuration(
            "maximum listing page count must be greater than zero",
        ));
    }
    Ok(())
}

fn page_limit_error(operation: &str) -> S3Error {
    S3Error::new(
        ErrorCategory::OversizedResponse,
        format!("{operation} exceeded the configured page limit"),
        RetryClassification::Never,
    )
}
