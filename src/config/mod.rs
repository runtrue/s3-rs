//! Typed client configuration.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use http::HeaderValue;

use crate::credentials::{CredentialsProvider, EnvironmentCredentialsProvider};
use crate::endpoint::Endpoint;
use crate::error::S3Error;
use crate::retry::RetryPolicy;

pub use crate::endpoint::AddressingStyle;

const MIN_MULTIPART_PART_SIZE: u64 = 5 * 1024 * 1024;
const MAX_MULTIPART_PART_SIZE: u64 = 5 * 1024 * 1024 * 1024;
const MAX_MULTIPART_CONCURRENCY: usize = 64;

/// Validated configuration used to construct an S3 client.
#[derive(Clone)]
pub struct S3Config {
    endpoint: Endpoint,
    region: String,
    bucket: String,
    addressing_style: AddressingStyle,
    connect_timeout: Duration,
    attempt_timeout: Duration,
    operation_timeout: Duration,
    idle_body_timeout: Duration,
    max_xml_response_size: usize,
    max_error_response_size: usize,
    retry_policy: RetryPolicy,
    multipart_threshold: u64,
    multipart_part_size: u64,
    multipart_concurrency: usize,
    max_multipart_in_flight_bytes: u64,
    user_agent: String,
    credentials_provider: Arc<dyn CredentialsProvider>,
}

impl S3Config {
    /// Starts a configuration builder with HTTPS-safe defaults.
    pub fn builder() -> S3ConfigBuilder {
        S3ConfigBuilder::default()
    }

    /// Returns the service endpoint.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Returns the signing region.
    pub fn region(&self) -> &str {
        &self.region
    }

    /// Returns the configured bucket.
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Returns the configured bucket addressing style.
    pub fn addressing_style(&self) -> AddressingStyle {
        self.addressing_style
    }

    /// Returns the connection-establishment timeout.
    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    /// Returns the timeout applied to a single request attempt.
    pub fn attempt_timeout(&self) -> Duration {
        self.attempt_timeout
    }

    /// Returns the overall operation timeout across all attempts.
    pub fn operation_timeout(&self) -> Duration {
        self.operation_timeout
    }

    /// Returns the maximum permitted gap between response body chunks.
    pub fn idle_body_timeout(&self) -> Duration {
        self.idle_body_timeout
    }

    /// Returns the maximum XML response body size in bytes.
    pub fn max_xml_response_size(&self) -> usize {
        self.max_xml_response_size
    }

    /// Returns the maximum error response body size in bytes.
    pub fn max_error_response_size(&self) -> usize {
        self.max_error_response_size
    }

    /// Returns the retry policy.
    pub fn retry_policy(&self) -> &RetryPolicy {
        &self.retry_policy
    }

    /// Returns the object-size threshold available to application multipart policy.
    pub fn multipart_threshold(&self) -> u64 {
        self.multipart_threshold
    }

    /// Returns the configured multipart part size.
    pub fn multipart_part_size(&self) -> u64 {
        self.multipart_part_size
    }

    /// Returns the maximum number of concurrently buffered multipart parts.
    pub fn multipart_concurrency(&self) -> usize {
        self.multipart_concurrency
    }

    /// Returns the total byte budget for concurrently buffered multipart parts.
    pub fn max_multipart_in_flight_bytes(&self) -> u64 {
        self.max_multipart_in_flight_bytes
    }

    /// Returns the HTTP user-agent value.
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    /// Returns the credential provider.
    pub fn credentials_provider(&self) -> &Arc<dyn CredentialsProvider> {
        &self.credentials_provider
    }
}

impl fmt::Debug for S3Config {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("addressing_style", &self.addressing_style)
            .field("connect_timeout", &self.connect_timeout)
            .field("attempt_timeout", &self.attempt_timeout)
            .field("operation_timeout", &self.operation_timeout)
            .field("idle_body_timeout", &self.idle_body_timeout)
            .field("max_xml_response_size", &self.max_xml_response_size)
            .field("max_error_response_size", &self.max_error_response_size)
            .field("retry_policy", &self.retry_policy)
            .field("multipart_threshold", &self.multipart_threshold)
            .field("multipart_part_size", &self.multipart_part_size)
            .field("multipart_concurrency", &self.multipart_concurrency)
            .field(
                "max_multipart_in_flight_bytes",
                &self.max_multipart_in_flight_bytes,
            )
            .field("user_agent", &self.user_agent)
            .field("credentials_provider", &"[REDACTED]")
            .finish()
    }
}

/// Builder for [`S3Config`].
pub struct S3ConfigBuilder {
    endpoint: Endpoint,
    region: String,
    bucket: Option<String>,
    addressing_style: AddressingStyle,
    allow_http: bool,
    connect_timeout: Duration,
    attempt_timeout: Duration,
    operation_timeout: Duration,
    idle_body_timeout: Duration,
    max_xml_response_size: usize,
    max_error_response_size: usize,
    retry_policy: RetryPolicy,
    multipart_threshold: u64,
    multipart_part_size: u64,
    multipart_concurrency: usize,
    max_multipart_in_flight_bytes: u64,
    user_agent: String,
    credentials_provider: Arc<dyn CredentialsProvider>,
}

impl S3ConfigBuilder {
    /// Sets a custom endpoint. HTTP endpoints still require explicit opt-in.
    pub fn endpoint(mut self, endpoint: Endpoint) -> Self {
        self.endpoint = endpoint;
        self
    }

    /// Sets the SigV4 signing region.
    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = region.into();
        self
    }

    /// Sets the target bucket.
    pub fn bucket(mut self, bucket: impl Into<String>) -> Self {
        self.bucket = Some(bucket.into());
        self
    }

    /// Sets the bucket addressing style.
    pub fn addressing_style(mut self, style: AddressingStyle) -> Self {
        self.addressing_style = style;
        self
    }

    /// Explicitly permits plain HTTP for local S3-compatible testing.
    ///
    /// This does not disable TLS verification for HTTPS endpoints and should not be
    /// enabled for endpoints reached over an untrusted network.
    pub fn allow_http_for_local_testing(mut self) -> Self {
        self.allow_http = true;
        self
    }

    /// Sets the connection-establishment timeout.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Sets the timeout for each individual request attempt.
    pub fn attempt_timeout(mut self, timeout: Duration) -> Self {
        self.attempt_timeout = timeout;
        self
    }

    /// Sets the overall timeout across retries and cleanup.
    pub fn operation_timeout(mut self, timeout: Duration) -> Self {
        self.operation_timeout = timeout;
        self
    }

    /// Sets the maximum idle period while streaming a response body.
    pub fn idle_body_timeout(mut self, timeout: Duration) -> Self {
        self.idle_body_timeout = timeout;
        self
    }

    /// Sets the maximum XML response body size.
    pub fn max_xml_response_size(mut self, bytes: usize) -> Self {
        self.max_xml_response_size = bytes;
        self
    }

    /// Sets the maximum error response body size.
    pub fn max_error_response_size(mut self, bytes: usize) -> Self {
        self.max_error_response_size = bytes;
        self
    }

    /// Sets the retry policy.
    pub fn retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    /// Sets the object-size threshold available to application multipart policy.
    pub fn multipart_threshold(mut self, bytes: u64) -> Self {
        self.multipart_threshold = bytes;
        self
    }

    /// Sets the multipart part size.
    pub fn multipart_part_size(mut self, bytes: u64) -> Self {
        self.multipart_part_size = bytes;
        self
    }

    /// Sets the maximum multipart upload concurrency.
    pub fn multipart_concurrency(mut self, concurrency: usize) -> Self {
        self.multipart_concurrency = concurrency;
        self
    }

    /// Sets the total byte budget for concurrently buffered multipart parts.
    pub fn max_multipart_in_flight_bytes(mut self, bytes: u64) -> Self {
        self.max_multipart_in_flight_bytes = bytes;
        self
    }

    /// Sets the HTTP user-agent value.
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    /// Sets the asynchronous credential provider.
    pub fn credentials_provider(mut self, provider: Arc<dyn CredentialsProvider>) -> Self {
        self.credentials_provider = provider;
        self
    }

    /// Validates and creates the configuration.
    pub fn build(self) -> Result<S3Config, S3Error> {
        let bucket = self
            .bucket
            .ok_or_else(|| S3Error::configuration("bucket is required"))?;
        if !self.endpoint.is_https() && !self.allow_http {
            return Err(S3Error::configuration(
                "plain HTTP requires allow_http_for_local_testing",
            ));
        }
        validate_nonempty_token("region", &self.region)?;
        if bucket.is_empty() {
            return Err(S3Error::configuration("bucket must not be empty"));
        }
        for (name, timeout) in [
            ("connect timeout", self.connect_timeout),
            ("attempt timeout", self.attempt_timeout),
            ("operation timeout", self.operation_timeout),
            ("idle body timeout", self.idle_body_timeout),
        ] {
            if timeout.is_zero() {
                return Err(S3Error::configuration(format!(
                    "{name} must be greater than zero"
                )));
            }
        }
        if self.max_xml_response_size == 0 || self.max_error_response_size == 0 {
            return Err(S3Error::configuration(
                "response body limits must be greater than zero",
            ));
        }
        if !(MIN_MULTIPART_PART_SIZE..=MAX_MULTIPART_PART_SIZE).contains(&self.multipart_part_size)
        {
            return Err(S3Error::configuration(
                "multipart part size must be between 5 MiB and 5 GiB",
            ));
        }
        if self.multipart_threshold < self.multipart_part_size {
            return Err(S3Error::configuration(
                "multipart threshold must not be smaller than multipart part size",
            ));
        }
        if !(1..=MAX_MULTIPART_CONCURRENCY).contains(&self.multipart_concurrency) {
            return Err(S3Error::configuration(
                "multipart concurrency must be between 1 and 64",
            ));
        }
        let in_flight_bytes = self
            .multipart_part_size
            .checked_mul(
                u64::try_from(self.multipart_concurrency).map_err(|_| {
                    S3Error::configuration("multipart concurrency does not fit in u64")
                })?,
            )
            .ok_or_else(|| S3Error::configuration("multipart in-flight byte budget overflow"))?;
        if in_flight_bytes > self.max_multipart_in_flight_bytes {
            return Err(S3Error::configuration(
                "multipart part size times concurrency exceeds the in-flight byte budget",
            ));
        }
        HeaderValue::from_str(&self.user_agent)
            .map_err(|_| S3Error::configuration("user agent is not a valid HTTP header value"))?;
        if self.user_agent.is_empty() {
            return Err(S3Error::configuration("user agent must not be empty"));
        }

        // Ensure this bucket is valid for the selected style before any signed request.
        self.endpoint
            .object_url(&bucket, None, self.addressing_style)?;

        Ok(S3Config {
            endpoint: self.endpoint,
            region: self.region,
            bucket,
            addressing_style: self.addressing_style,
            connect_timeout: self.connect_timeout,
            attempt_timeout: self.attempt_timeout,
            operation_timeout: self.operation_timeout,
            idle_body_timeout: self.idle_body_timeout,
            max_xml_response_size: self.max_xml_response_size,
            max_error_response_size: self.max_error_response_size,
            retry_policy: self.retry_policy,
            multipart_threshold: self.multipart_threshold,
            multipart_part_size: self.multipart_part_size,
            multipart_concurrency: self.multipart_concurrency,
            max_multipart_in_flight_bytes: self.max_multipart_in_flight_bytes,
            user_agent: self.user_agent,
            credentials_provider: self.credentials_provider,
        })
    }
}

impl Default for S3ConfigBuilder {
    fn default() -> Self {
        Self {
            endpoint: Endpoint::default(),
            region: "us-east-1".to_owned(),
            bucket: None,
            addressing_style: AddressingStyle::Path,
            allow_http: false,
            connect_timeout: Duration::from_secs(10),
            attempt_timeout: Duration::from_secs(30),
            operation_timeout: Duration::from_secs(5 * 60),
            idle_body_timeout: Duration::from_secs(30),
            max_xml_response_size: 1024 * 1024,
            max_error_response_size: 64 * 1024,
            retry_policy: RetryPolicy::default(),
            multipart_threshold: 16 * 1024 * 1024,
            multipart_part_size: 8 * 1024 * 1024,
            multipart_concurrency: 4,
            max_multipart_in_flight_bytes: 64 * 1024 * 1024,
            user_agent: format!("s3-wire/{}", env!("CARGO_PKG_VERSION")),
            credentials_provider: Arc::new(EnvironmentCredentialsProvider::new()),
        }
    }
}

fn validate_nonempty_token(name: &str, value: &str) -> Result<(), S3Error> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(S3Error::configuration(format!("{name} is invalid")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_is_the_default_and_http_requires_opt_in() {
        let default_config = S3Config::builder().bucket("bucket").build().unwrap();
        assert!(default_config.endpoint().is_https());

        let endpoint = Endpoint::new("http://127.0.0.1:9000").unwrap();
        assert!(
            S3Config::builder()
                .endpoint(endpoint.clone())
                .bucket("bucket")
                .build()
                .is_err()
        );
        assert!(
            S3Config::builder()
                .bucket("bucket")
                .multipart_part_size(64 * 1024 * 1024)
                .multipart_concurrency(2)
                .build()
                .is_err()
        );
        assert!(
            S3Config::builder()
                .endpoint(endpoint)
                .allow_http_for_local_testing()
                .bucket("bucket")
                .build()
                .is_ok()
        );
    }

    #[test]
    fn multipart_memory_bounds_are_validated() {
        assert!(
            S3Config::builder()
                .bucket("bucket")
                .multipart_part_size(MIN_MULTIPART_PART_SIZE - 1)
                .build()
                .is_err()
        );
        assert!(
            S3Config::builder()
                .bucket("bucket")
                .multipart_concurrency(MAX_MULTIPART_CONCURRENCY + 1)
                .build()
                .is_err()
        );
    }
}
