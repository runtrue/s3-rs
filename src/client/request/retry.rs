use std::time::{Duration, SystemTime};

use http::header::RETRY_AFTER;
use http::{HeaderMap, StatusCode};

use super::super::S3Client;

impl S3Client {
    pub(super) fn uses_standard_aws_endpoint(&self) -> bool {
        let endpoint = self.inner.config.endpoint().url();
        if endpoint.scheme() != "https" {
            return false;
        }
        let authority = endpoint.authority();
        let host = authority
            .split_once(':')
            .map_or(authority, |(host, _)| host);
        host == "s3.amazonaws.com" || (host.starts_with("s3.") && host.ends_with(".amazonaws.com"))
    }
}

pub(super) fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let retry_at = httpdate::parse_http_date(value).ok()?;
    Some(
        retry_at
            .duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO),
    )
}

pub(super) fn controlled_region_redirect(status: StatusCode, headers: &HeaderMap) -> Option<&str> {
    if !matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
            | StatusCode::BAD_REQUEST
    ) {
        return None;
    }
    headers
        .get("x-amz-bucket-region")?
        .to_str()
        .ok()
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

#[cfg(test)]
mod tests {
    use http::{HeaderName, HeaderValue};

    use super::*;

    #[test]
    fn retry_after_in_the_past_means_no_delay() {
        let mut headers = HeaderMap::new();
        headers.insert(
            RETRY_AFTER,
            HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT"),
        );
        assert_eq!(parse_retry_after(&headers), Some(Duration::ZERO));
    }

    #[test]
    fn region_redirects_require_a_valid_aws_region_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-amz-bucket-region"),
            HeaderValue::from_static("eu-west-1"),
        );
        assert_eq!(
            controlled_region_redirect(StatusCode::MOVED_PERMANENTLY, &headers),
            Some("eu-west-1")
        );
        headers.insert(
            HeaderName::from_static("x-amz-bucket-region"),
            HeaderValue::from_static("amazonaws.com"),
        );
        assert_eq!(
            controlled_region_redirect(StatusCode::MOVED_PERMANENTLY, &headers),
            None
        );
    }
}
