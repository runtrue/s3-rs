//! Typed client configuration.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use http::HeaderValue;

use crate::credentials::{CredentialsProvider, EnvironmentCredentialsProvider};
use crate::endpoint::Endpoint;
use crate::error::S3Error;
use crate::observer::RequestObserver;
use crate::retry::RetryPolicy;

pub use crate::endpoint::AddressingStyle;

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
    user_agent: String,
    credentials_provider: Arc<dyn CredentialsProvider>,
    observer: Option<Arc<dyn RequestObserver>>,
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

    /// Returns the HTTP user-agent value.
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    /// Returns the credential provider.
    pub fn credentials_provider(&self) -> &Arc<dyn CredentialsProvider> {
        &self.credentials_provider
    }

    /// Returns the optional sanitized request observer.
    pub fn observer(&self) -> Option<&Arc<dyn RequestObserver>> {
        self.observer.as_ref()
    }

    /// Clones this service configuration for another bucket.
    ///
    /// This is used by [`crate::S3Client::for_bucket`] to retain the same
    /// connection pool and credential provider while validating the new bucket
    /// for the configured addressing style.
    ///
    /// # Errors
    ///
    /// Returns an error when the bucket is empty or invalid for the configured
    /// endpoint and addressing style.
    pub fn for_bucket(&self, bucket: impl Into<String>) -> Result<Self, S3Error> {
        let bucket = bucket.into();
        if bucket.is_empty() {
            return Err(S3Error::configuration("bucket must not be empty"));
        }
        self.endpoint
            .object_url(&bucket, None, self.addressing_style)?;
        let mut config = self.clone();
        config.bucket = bucket;
        Ok(config)
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
            .field("user_agent", &self.user_agent)
            .field("credentials_provider", &"[REDACTED]")
            .field("observer", &self.observer.as_ref().map(|_| "configured"))
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
    user_agent: String,
    credentials_provider: Arc<dyn CredentialsProvider>,
    observer: Option<Arc<dyn RequestObserver>>,
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

    /// Sets the overall timeout for one primitive S3 operation across retries.
    ///
    /// Managed multipart uploads instead use the transfer and cleanup deadlines
    /// in [`crate::MultipartOptions`].
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

    /// Installs a dependency-free observer for sanitized request lifecycle events.
    pub fn observer(mut self, observer: Arc<dyn RequestObserver>) -> Self {
        self.observer = Some(observer);
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
            if std::time::Instant::now().checked_add(timeout).is_none() {
                return Err(S3Error::configuration(format!(
                    "{name} is too large to represent as a deadline"
                )));
            }
        }
        if self.max_xml_response_size == 0 || self.max_error_response_size == 0 {
            return Err(S3Error::configuration(
                "response body limits must be greater than zero",
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
            user_agent: self.user_agent,
            credentials_provider: self.credentials_provider,
            observer: self.observer,
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
            user_agent: format!("s3-wire/{}", env!("CARGO_PKG_VERSION")),
            credentials_provider: Arc::new(EnvironmentCredentialsProvider::new()),
            observer: None,
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
                .endpoint(endpoint)
                .allow_http_for_local_testing()
                .bucket("bucket")
                .build()
                .is_ok()
        );
    }

    #[test]
    fn timeouts_must_fit_in_an_instant_deadline() {
        assert!(
            S3Config::builder()
                .bucket("bucket")
                .operation_timeout(Duration::MAX)
                .build()
                .is_err()
        );
    }
}
