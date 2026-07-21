//! Structured client errors.

use std::error::Error;
use std::fmt;

use http::StatusCode;
use time::OffsetDateTime;

/// Broad category suitable for programmatic error handling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorCategory {
    /// Client configuration is invalid.
    Configuration,
    /// Credentials or a signature were rejected.
    Authentication,
    /// The authenticated principal is not permitted to perform the operation.
    Authorization,
    /// The requested bucket, object, or upload does not exist.
    NotFound,
    /// The request conflicts with existing S3 state.
    Conflict,
    /// A conditional request precondition failed.
    Precondition,
    /// S3 asked the caller to reduce its request rate.
    Throttling,
    /// S3 returned a server-side failure.
    Server,
    /// The HTTP transport failed.
    Transport,
    /// TLS negotiation or validation failed.
    Tls,
    /// A configured timeout expired.
    Timeout,
    /// The operation was cancelled.
    Cancellation,
    /// The service response was malformed or internally inconsistent.
    InvalidResponse,
    /// A bounded response exceeded its configured limit.
    OversizedResponse,
    /// The requested behavior is intentionally unsupported.
    UnsupportedOperation,
    /// A checksum, length, or other integrity check failed.
    Integrity,
}

/// Whether an operation may be retried after an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryClassification {
    /// Retrying is not expected to succeed.
    Never,
    /// The failure is transient, subject to operation replayability.
    Retryable,
    /// The service throttled the request, subject to operation replayability.
    Throttled,
}

/// The phase in which a timeout occurred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TimeoutPhase {
    /// Establishing the network connection.
    Connect,
    /// Sending the request or waiting for response headers.
    Request,
    /// Waiting for progress while streaming a response body.
    ResponseBody,
    /// The overall operation deadline.
    Operation,
}

#[derive(Debug)]
struct RedactedSource {
    error_type: &'static str,
}

impl fmt::Display for RedactedSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "underlying {} error (details redacted)",
            self.error_type
        )
    }
}

impl Error for RedactedSource {}

/// A structured S3 client error.
///
/// Formatting an error never prints its underlying source or cleanup error. Source
/// errors are retained only as redacted type markers, preventing URLs, headers, or
/// response data from escaping through an otherwise innocent log statement.
pub struct S3Error {
    details: Box<S3ErrorDetails>,
}

struct S3ErrorDetails {
    category: ErrorCategory,
    code: Option<String>,
    status: Option<StatusCode>,
    message: String,
    request_id: Option<String>,
    host_id: Option<String>,
    retry: RetryClassification,
    timeout_phase: Option<TimeoutPhase>,
    server_time: Option<OffsetDateTime>,
    clock_skew: Option<time::Duration>,
    source: Option<RedactedSource>,
    cleanup_failure: Option<S3Error>,
}

impl S3Error {
    /// Creates an error with a category, safe message, and retry classification.
    pub fn new(
        category: ErrorCategory,
        message: impl Into<String>,
        retry: RetryClassification,
    ) -> Self {
        Self {
            details: Box::new(S3ErrorDetails {
                category,
                code: None,
                status: None,
                message: message.into(),
                request_id: None,
                host_id: None,
                retry,
                timeout_phase: None,
                server_time: None,
                clock_skew: None,
                source: None,
                cleanup_failure: None,
            }),
        }
    }

    /// Creates a configuration error.
    pub fn configuration(message: impl Into<String>) -> Self {
        Self::new(
            ErrorCategory::Configuration,
            message,
            RetryClassification::Never,
        )
    }

    /// Creates an unsupported-operation error.
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(
            ErrorCategory::UnsupportedOperation,
            message,
            RetryClassification::Never,
        )
    }

    /// Creates a transport error while retaining only a redacted source marker.
    pub fn transport<E>(source: E) -> Self
    where
        E: Error + Send + Sync + 'static,
    {
        Self::new(
            ErrorCategory::Transport,
            "HTTP transport failed",
            RetryClassification::Retryable,
        )
        .with_source(source)
    }

    /// Creates a timeout error.
    pub fn timeout(phase: TimeoutPhase, message: impl Into<String>) -> Self {
        let mut error = Self::new(
            ErrorCategory::Timeout,
            message,
            RetryClassification::Retryable,
        );
        error.details.timeout_phase = Some(phase);
        error
    }

    /// Creates an invalid-response error.
    pub fn invalid_response(message: impl Into<String>) -> Self {
        Self::new(
            ErrorCategory::InvalidResponse,
            message,
            RetryClassification::Never,
        )
    }

    /// Creates an integrity error.
    pub fn integrity(message: impl Into<String>) -> Self {
        Self::new(
            ErrorCategory::Integrity,
            message,
            RetryClassification::Never,
        )
    }

    /// Creates a cancellation error.
    pub fn cancellation(message: impl Into<String>) -> Self {
        Self::new(
            ErrorCategory::Cancellation,
            message,
            RetryClassification::Never,
        )
    }

    /// Attaches parsed S3 response metadata.
    pub fn with_service_details(
        mut self,
        code: Option<String>,
        status: StatusCode,
        request_id: Option<String>,
        host_id: Option<String>,
    ) -> Self {
        self.details.code = code;
        self.details.status = Some(status);
        self.details.request_id = request_id;
        self.details.host_id = host_id;
        self
    }

    /// Retains the source error's type while discarding potentially sensitive text.
    pub fn with_source<E>(mut self, _source: E) -> Self
    where
        E: Error + Send + Sync + 'static,
    {
        self.details.source = Some(RedactedSource {
            error_type: std::any::type_name::<E>(),
        });
        self
    }

    /// Attaches a cleanup error without replacing the primary failure.
    pub fn with_cleanup_failure(mut self, cleanup_failure: Self) -> Self {
        self.details.cleanup_failure = Some(cleanup_failure);
        self
    }

    /// Attaches the server time and measured local clock offset.
    pub fn with_clock_skew(
        mut self,
        server_time: OffsetDateTime,
        local_time: OffsetDateTime,
    ) -> Self {
        self.details.server_time = Some(server_time);
        self.details.clock_skew = Some(server_time - local_time);
        self
    }

    /// Returns the broad error category.
    pub fn category(&self) -> ErrorCategory {
        self.details.category
    }

    /// Returns the S3 error code, when supplied by the service.
    pub fn code(&self) -> Option<&str> {
        self.details.code.as_deref()
    }

    /// Returns the HTTP response status, when a response was received.
    pub fn status(&self) -> Option<StatusCode> {
        self.details.status
    }

    /// Returns the safe, caller-facing error message.
    pub fn message(&self) -> &str {
        &self.details.message
    }

    /// Returns the S3 request identifier, when supplied by the service.
    pub fn request_id(&self) -> Option<&str> {
        self.details.request_id.as_deref()
    }

    /// Returns the S3 host identifier, when supplied by the service.
    pub fn host_id(&self) -> Option<&str> {
        self.details.host_id.as_deref()
    }

    /// Returns the retry classification.
    pub fn retry_classification(&self) -> RetryClassification {
        self.details.retry
    }

    /// Returns the timeout phase, when this is a timeout error.
    pub fn timeout_phase(&self) -> Option<TimeoutPhase> {
        self.details.timeout_phase
    }

    /// Returns the server's reported time for a clock-skew failure.
    pub fn server_time(&self) -> Option<OffsetDateTime> {
        self.details.server_time
    }

    /// Returns `server_time - local_time` for a clock-skew failure.
    pub fn clock_skew(&self) -> Option<time::Duration> {
        self.details.clock_skew
    }

    /// Returns the cleanup error attached to a primary failure.
    pub fn cleanup_failure(&self) -> Option<&Self> {
        self.details.cleanup_failure.as_ref()
    }
}

impl fmt::Display for S3Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:?}: {}",
            self.details.category, self.details.message
        )?;
        if self.details.cleanup_failure.is_some() {
            formatter.write_str(" (cleanup also failed)")?;
        }
        Ok(())
    }
}

impl fmt::Debug for S3Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3Error")
            .field("category", &self.details.category)
            .field("code", &self.details.code)
            .field("status", &self.details.status)
            .field("message", &self.details.message)
            .field("request_id", &self.details.request_id)
            .field("host_id", &self.details.host_id)
            .field("retry", &self.details.retry)
            .field("timeout_phase", &self.details.timeout_phase)
            .field("server_time", &self.details.server_time)
            .field("clock_skew", &self.details.clock_skew)
            .field(
                "source",
                &self.details.source.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "cleanup_failure",
                &self.details.cleanup_failure.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl Error for S3Error {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.details
            .source
            .as_ref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct SensitiveSource;

    impl fmt::Display for SensitiveSource {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("secret=do-not-print")
        }
    }

    impl Error for SensitiveSource {}

    #[test]
    fn public_error_is_pointer_sized() {
        assert_eq!(std::mem::size_of::<S3Error>(), std::mem::size_of::<usize>());
    }

    #[test]
    fn source_and_cleanup_details_are_redacted() {
        let error = S3Error::transport(SensitiveSource).with_cleanup_failure(
            S3Error::invalid_response("cleanup detail must remain nested"),
        );

        let rendered = format!("{error:?} {error} {}", error.source().unwrap());
        assert!(!rendered.contains("do-not-print"));
        assert!(!rendered.contains("cleanup detail"));
        assert!(rendered.contains("cleanup also failed"));
    }
}
