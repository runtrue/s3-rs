//! S3 client implementation.

mod download;
mod multipart;
mod multipart_upload;
mod object;
mod presign;
mod request;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::config::S3Config;
use crate::error::S3Error;
use crate::transport::Transport;
use crate::{RequestEvent, RequestObserver};

pub use download::DownloadToPathOutput;

struct ClientInner {
    config: S3Config,
    transport: Transport,
    retry_quota: Arc<RetryQuota>,
}

const RETRY_QUOTA_CAPACITY: u32 = 500;
const TRANSIENT_RETRY_COST: u32 = 14;
const THROTTLING_RETRY_COST: u32 = 5;

struct RetryQuota {
    tokens: AtomicU32,
}

struct RetryPermit {
    quota: Arc<RetryQuota>,
    cost: u32,
    committed: bool,
}

impl RetryQuota {
    fn new() -> Self {
        Self {
            tokens: AtomicU32::new(RETRY_QUOTA_CAPACITY),
        }
    }

    fn acquire(
        self: &Arc<Self>,
        classification: crate::RetryClassification,
    ) -> Option<RetryPermit> {
        let cost = match classification {
            crate::RetryClassification::Throttled => THROTTLING_RETRY_COST,
            crate::RetryClassification::Retryable => TRANSIENT_RETRY_COST,
            crate::RetryClassification::Never => return None,
        };
        self.tokens
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |available| {
                if available >= cost {
                    Some(available - cost)
                } else {
                    None
                }
            })
            .ok()
            .map(|_| RetryPermit {
                quota: Arc::clone(self),
                cost,
                committed: false,
            })
    }

    fn replenish_after_success(&self, last_retry_cost: Option<u32>) {
        let restored = last_retry_cost.unwrap_or(1);
        let _ = self
            .tokens
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |available| {
                Some(available.saturating_add(restored).min(RETRY_QUOTA_CAPACITY))
            });
    }
}

impl RetryPermit {
    fn commit(mut self) -> u32 {
        self.committed = true;
        self.cost
    }
}

impl Drop for RetryPermit {
    fn drop(&mut self) {
        if !self.committed {
            self.quota.replenish_after_success(Some(self.cost));
        }
    }
}

/// An async S3-compatible client.
#[derive(Clone)]
pub struct S3Client {
    inner: Arc<ClientInner>,
}

impl S3Client {
    /// Constructs a client from validated configuration.
    pub fn new(config: S3Config) -> Result<Self, S3Error> {
        let transport = Transport::new(
            config.connect_timeout(),
            config.user_agent(),
            !config.endpoint().is_https(),
        )?;
        Ok(Self {
            inner: Arc::new(ClientInner {
                config,
                transport,
                retry_quota: Arc::new(RetryQuota::new()),
            }),
        })
    }

    /// Returns this client's validated configuration.
    pub fn config(&self) -> &S3Config {
        &self.inner.config
    }

    /// Creates a cheap bucket-scoped client that shares this client's HTTP
    /// connection pool and credential provider.
    ///
    /// # Errors
    ///
    /// Returns an error when `bucket` is invalid for the configured addressing
    /// style.
    pub fn for_bucket(&self, bucket: impl Into<String>) -> Result<Self, S3Error> {
        let config = self.inner.config.for_bucket(bucket)?;
        Ok(Self {
            inner: Arc::new(ClientInner {
                config,
                transport: self.inner.transport.clone(),
                retry_quota: Arc::clone(&self.inner.retry_quota),
            }),
        })
    }

    pub(in crate::client) fn observe(&self, event: RequestEvent<'_>) {
        let Some(observer) = self.inner.config.observer() else {
            return;
        };
        let observer: &dyn RequestObserver = observer.as_ref();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            observer.on_event(event);
        }));
    }

    fn operation_target(&self, key: Option<&str>) -> Result<crate::endpoint::EndpointUrl, S3Error> {
        self.inner.config.endpoint().object_url(
            self.inner.config.bucket(),
            key,
            self.inner.config.addressing_style(),
        )
    }
}

impl fmt::Debug for S3Client {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3Client")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_handles_reuse_validated_service_configuration() {
        let client =
            S3Client::new(S3Config::builder().bucket("first-bucket").build().unwrap()).unwrap();
        let second = client.for_bucket("second-bucket").unwrap();
        assert_eq!(client.config().bucket(), "first-bucket");
        assert_eq!(second.config().bucket(), "second-bucket");
        assert_eq!(client.config().endpoint(), second.config().endpoint());
        assert!(client.for_bucket("").is_err());
    }

    #[test]
    fn retry_quota_is_shared_bounded_and_replenished() {
        let quota = Arc::new(RetryQuota::new());
        for _ in 0..35 {
            let permit = quota
                .acquire(crate::RetryClassification::Retryable)
                .expect("quota remains");
            assert_eq!(permit.commit(), TRANSIENT_RETRY_COST);
        }
        assert!(
            quota
                .acquire(crate::RetryClassification::Retryable)
                .is_none()
        );
        quota.replenish_after_success(Some(TRANSIENT_RETRY_COST));
        let permit = quota
            .acquire(crate::RetryClassification::Retryable)
            .expect("replenished quota");
        assert_eq!(permit.commit(), TRANSIENT_RETRY_COST);
    }

    #[test]
    fn unused_retry_permit_returns_its_tokens() {
        let quota = Arc::new(RetryQuota::new());
        let permit = quota
            .acquire(crate::RetryClassification::Retryable)
            .expect("initial quota");
        drop(permit);
        assert_eq!(quota.tokens.load(Ordering::Acquire), RETRY_QUOTA_CAPACITY);
    }
}
