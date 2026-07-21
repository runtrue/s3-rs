//! Retry policy and classification.

use std::time::Duration;

use crate::error::{RetryClassification, S3Error};

/// Bounded exponential-backoff policy with full jitter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    max_attempts: u32,
    base_delay: Duration,
    max_delay: Duration,
    max_elapsed: Duration,
}

impl RetryPolicy {
    /// Creates a policy with production-oriented defaults.
    pub fn standard() -> Self {
        Self {
            max_attempts: 4,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            max_elapsed: Duration::from_secs(30),
        }
    }

    /// Creates and validates a retry policy.
    pub fn new(
        max_attempts: u32,
        base_delay: Duration,
        max_delay: Duration,
        max_elapsed: Duration,
    ) -> Result<Self, S3Error> {
        if max_attempts == 0 {
            return Err(S3Error::configuration(
                "retry max_attempts must be at least one",
            ));
        }
        if base_delay.is_zero() {
            return Err(S3Error::configuration(
                "retry base_delay must be greater than zero",
            ));
        }
        if max_delay < base_delay {
            return Err(S3Error::configuration(
                "retry max_delay must not be shorter than base_delay",
            ));
        }
        if max_elapsed.is_zero() {
            return Err(S3Error::configuration(
                "retry max_elapsed must be greater than zero",
            ));
        }
        Ok(Self {
            max_attempts,
            base_delay,
            max_delay,
            max_elapsed,
        })
    }

    /// Returns the total number of allowed attempts, including the initial attempt.
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// Returns the initial backoff delay.
    pub fn base_delay(&self) -> Duration {
        self.base_delay
    }

    /// Returns the maximum delay between attempts.
    pub fn max_delay(&self) -> Duration {
        self.max_delay
    }

    /// Returns the maximum elapsed time in which another attempt may begin.
    pub fn max_elapsed(&self) -> Duration {
        self.max_elapsed
    }

    /// Returns whether another attempt is allowed by classification and bounds.
    pub fn permits_retry(
        &self,
        classification: RetryClassification,
        attempts_completed: u32,
        elapsed: Duration,
    ) -> bool {
        classification != RetryClassification::Never
            && attempts_completed < self.max_attempts
            && elapsed < self.max_elapsed
    }

    /// Computes a full-jitter delay after `attempts_completed` attempts.
    ///
    /// A server-provided `Retry-After` value is honored after clamping it to the
    /// policy's delay and elapsed-time bounds.
    pub fn delay(
        &self,
        attempts_completed: u32,
        elapsed: Duration,
        retry_after: Option<Duration>,
    ) -> Option<Duration> {
        if attempts_completed == 0
            || attempts_completed >= self.max_attempts
            || elapsed >= self.max_elapsed
        {
            return None;
        }

        let remaining = self.max_elapsed.saturating_sub(elapsed);
        if let Some(delay) = retry_after {
            return (delay <= remaining).then_some(delay);
        }

        let exponent = attempts_completed.saturating_sub(1).min(31);
        let multiplier = 1_u32 << exponent;
        let ceiling = self.base_delay.saturating_mul(multiplier);
        let ceiling = ceiling.min(self.max_delay).min(remaining);
        let ceiling_nanos = u64::try_from(ceiling.as_nanos()).unwrap_or(u64::MAX);
        let nanos = fastrand::u64(0..=ceiling_nanos);
        Some(Duration::from_nanos(nanos))
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::standard()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_rejects_invalid_bounds() {
        assert!(
            RetryPolicy::new(
                0,
                Duration::from_millis(1),
                Duration::from_secs(1),
                Duration::from_secs(2)
            )
            .is_err()
        );
        assert!(
            RetryPolicy::new(
                2,
                Duration::from_secs(2),
                Duration::from_secs(1),
                Duration::from_secs(3)
            )
            .is_err()
        );
    }

    #[test]
    fn delay_is_bounded_by_policy_and_elapsed_time() {
        let policy = RetryPolicy::new(
            10,
            Duration::from_secs(1),
            Duration::from_secs(8),
            Duration::from_secs(10),
        )
        .unwrap();

        assert_eq!(
            policy.delay(1, Duration::from_secs(1), Some(Duration::from_secs(9))),
            Some(Duration::from_secs(9))
        );
        assert!(
            policy
                .delay(9, Duration::from_secs(9), Some(Duration::from_secs(30)))
                .is_none()
        );
        assert!(policy.delay(10, Duration::from_secs(9), None).is_none());
    }
}
