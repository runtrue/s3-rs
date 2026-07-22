use std::collections::HashSet;

use http::{HeaderMap, Method};

use super::super::S3Client;
use super::super::request::{OperationDeadline, protocol_error};
use super::headers::{parse_request_ids, push_optional_query};
use crate::error::{ErrorCategory, RetryClassification, S3Error};
use crate::operation::{ListObjectsV2Output, ListObjectsV2Request};
use crate::protocol::parse_list_objects_v2;

impl S3Client {
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
            ("encoding-type".to_owned(), "url".to_owned()),
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
        let target = self.operation_target(None)?;
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
}
