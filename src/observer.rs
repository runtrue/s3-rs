//! Dependency-free, secret-safe request lifecycle observation.

use std::time::Duration;

use crate::{ErrorCategory, RetryClassification};

/// Stage of one request attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestEventKind {
    /// A signed request attempt is about to be sent.
    AttemptStarted,
    /// An attempt completed successfully.
    AttemptSucceeded,
    /// An attempt failed and will not be retried.
    AttemptFailed,
    /// A retry was scheduled after an attempt failed.
    RetryScheduled,
    /// AWS supplied a validated region correction and the request will be re-signed.
    RegionRedirected,
}

/// Sanitized lifecycle event for an S3 request attempt.
///
/// Object keys, bucket names, endpoints, headers, upload identifiers, service
/// messages, and signing material are deliberately absent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestEvent<'a> {
    /// Lifecycle stage.
    pub kind: RequestEventKind,
    /// HTTP method without its URL or headers.
    pub method: &'a str,
    /// One-based attempt number.
    pub attempt: u32,
    /// Stable error category, when this event follows a failure.
    pub error_category: Option<ErrorCategory>,
    /// Retry classification, when this event follows a failure.
    pub retry_classification: Option<RetryClassification>,
    /// Selected retry delay for `RetryScheduled`.
    pub retry_delay: Option<Duration>,
}

/// Receives sanitized request lifecycle events.
///
/// Implementations must return quickly and must not block the async executor.
/// Panics are isolated by the client and cannot fail an S3 operation.
pub trait RequestObserver: Send + Sync {
    /// Records one request lifecycle event.
    fn on_event(&self, event: RequestEvent<'_>);
}
