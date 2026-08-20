use std::time::{Duration, SystemTime};

use http::header::RETRY_AFTER;
use http::{HeaderMap, StatusCode};

use super::super::S3Client;

impl S3Client {
    pub(super) fn redirected_aws_authority(&self, region: &str) -> Option<String> {
        let endpoint = self.inner.config.endpoint().url();
        if endpoint.scheme() != "https" {
            return None;
        }
        let authority = endpoint.authority();
        let host = authority
            .split_once(':')
            .map_or(authority, |(host, _)| host);
        let (name, suffix) = if let Some(name) = host.strip_suffix(".amazonaws.com.cn") {
            (name, "amazonaws.com.cn")
        } else {
            let name = host.strip_suffix(".amazonaws.com")?;
            (name, "amazonaws.com")
        };
        let prefix = if name == "s3" || is_regional_name(name, "s3") {
            "s3"
        } else if is_regional_name(name, "s3.dualstack") {
            "s3.dualstack"
        } else if is_regional_name(name, "s3-fips") {
            "s3-fips"
        } else if is_regional_name(name, "s3-fips.dualstack") {
            "s3-fips.dualstack"
        } else {
            return None;
        };
        Some(format!("{prefix}.{region}.{suffix}"))
    }
}

fn is_regional_name(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

pub(super) fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(RETRY_AFTER)
        .and_then(parse_retry_after_value)
        .or_else(|| {
            headers
                .get("x-amz-retry-after")
                .and_then(parse_retry_after_value)
        })
}

fn parse_retry_after_value(value: &http::HeaderValue) -> Option<Duration> {
    let value = value.to_str().ok()?;
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
    use crate::config::S3Config;
    use crate::endpoint::Endpoint;

    fn client_for_endpoint(endpoint: &str) -> S3Client {
        let config = S3Config::builder()
            .endpoint(Endpoint::new(endpoint).unwrap())
            .bucket("bucket")
            .build()
            .unwrap();
        S3Client::new(config).unwrap()
    }

    #[test]
    fn region_redirect_authority_preserves_whitelisted_aws_endpoint_shapes() {
        for (endpoint, region, expected) in [
            (
                "https://s3.amazonaws.com",
                "eu-west-1",
                "s3.eu-west-1.amazonaws.com",
            ),
            (
                "https://s3.us-east-1.amazonaws.com",
                "us-west-2",
                "s3.us-west-2.amazonaws.com",
            ),
            (
                "https://s3.dualstack.us-east-1.amazonaws.com",
                "us-west-2",
                "s3.dualstack.us-west-2.amazonaws.com",
            ),
            (
                "https://s3-fips.us-east-1.amazonaws.com",
                "us-gov-west-1",
                "s3-fips.us-gov-west-1.amazonaws.com",
            ),
            (
                "https://s3-fips.dualstack.us-east-1.amazonaws.com",
                "us-gov-west-1",
                "s3-fips.dualstack.us-gov-west-1.amazonaws.com",
            ),
            (
                "https://s3.cn-north-1.amazonaws.com.cn",
                "cn-northwest-1",
                "s3.cn-northwest-1.amazonaws.com.cn",
            ),
        ] {
            assert_eq!(
                client_for_endpoint(endpoint).redirected_aws_authority(region),
                Some(expected.to_owned())
            );
        }
    }

    #[test]
    fn region_redirect_authority_rejects_non_aws_endpoints() {
        assert_eq!(
            client_for_endpoint("https://objects.example.com")
                .redirected_aws_authority("us-west-2"),
            None
        );
    }

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
    fn accepts_amazon_retry_after_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-amz-retry-after"),
            HeaderValue::from_static("7"),
        );
        assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(7)));
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

    #[test]
    fn recognizes_regional_endpoint_shapes() {
        assert!(is_regional_name("s3.us-west-2", "s3"));
        assert!(is_regional_name(
            "s3-fips.dualstack.us-gov-west-1",
            "s3-fips.dualstack"
        ));
        assert!(!is_regional_name("s3.attacker.example", "s3"));
    }
}
