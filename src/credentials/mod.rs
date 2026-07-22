//! Credential values and providers.

use std::env;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use time::OffsetDateTime;
use tokio::sync::Mutex;

use crate::error::{ErrorCategory, RetryClassification, S3Error};

#[cfg(feature = "aws-credentials")]
use aws_credential_types::provider::{ProvideCredentials as _, SharedCredentialsProvider};

/// AWS-compatible credentials and optional expiration.
///
/// Secret values are zeroized on drop. Its `Debug` implementation redacts all
/// credential material, including the access-key identifier.
#[derive(Clone)]
pub struct Credentials {
    access_key_id: SecretString,
    secret_access_key: SecretString,
    session_token: Option<SecretString>,
    expires_at: Option<OffsetDateTime>,
}

impl Credentials {
    /// Creates non-expiring credentials.
    pub fn new(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
    ) -> Result<Self, S3Error> {
        Self::with_expiration(access_key_id, secret_access_key, session_token, None)
    }

    /// Creates credentials with an optional expiration time.
    pub fn with_expiration(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
        expires_at: Option<OffsetDateTime>,
    ) -> Result<Self, S3Error> {
        let access_key_id = access_key_id.into();
        let secret_access_key = secret_access_key.into();
        if access_key_id.is_empty() {
            return Err(S3Error::configuration(
                "credential access key identifier must not be empty",
            ));
        }
        if secret_access_key.is_empty() {
            return Err(S3Error::configuration(
                "credential secret access key must not be empty",
            ));
        }
        if session_token.as_deref() == Some("") {
            return Err(S3Error::configuration(
                "credential session token must not be empty when present",
            ));
        }
        Ok(Self {
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            session_token: session_token.map(Into::into),
            expires_at,
        })
    }

    /// Returns the access-key identifier for signing.
    pub fn access_key_id(&self) -> &str {
        self.access_key_id.expose_secret()
    }

    /// Returns the secret access key for signing.
    pub fn secret_access_key(&self) -> &SecretString {
        &self.secret_access_key
    }

    /// Returns the optional session token for signing.
    pub fn session_token(&self) -> Option<&SecretString> {
        self.session_token.as_ref()
    }

    /// Returns the credential expiration time, when supplied by the provider.
    pub fn expires_at(&self) -> Option<OffsetDateTime> {
        self.expires_at
    }

    /// Returns whether the credentials expire at or before `time`.
    pub fn expires_by(&self, time: OffsetDateTime) -> bool {
        self.expires_at.is_some_and(|expiration| expiration <= time)
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("access_key_id", &"[REDACTED]")
            .field("secret_access_key", &"[REDACTED]")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Asynchronous source of AWS-compatible credentials.
#[async_trait]
pub trait CredentialsProvider: Send + Sync {
    /// Resolves credentials for a request.
    async fn provide_credentials(&self) -> Result<Credentials, S3Error>;
}

/// Adapter for AWS's standard renewable credential provider chain.
///
/// This provider is available only with the `aws-credentials` feature so the
/// default `s3-wire` dependency graph remains small. The chain covers shared
/// profiles, environment credentials, web identity, ECS container credentials,
/// EC2 IMDSv2, credential processes, and assume-role profiles.
#[cfg(feature = "aws-credentials")]
#[derive(Clone)]
pub struct AwsDefaultCredentialsProvider {
    inner: SharedCredentialsProvider,
}

#[cfg(feature = "aws-credentials")]
impl AwsDefaultCredentialsProvider {
    /// Builds the standard AWS credential chain.
    pub async fn new() -> Self {
        let chain = aws_config::default_provider::credentials::DefaultCredentialsChain::builder()
            .build()
            .await;
        Self {
            inner: SharedCredentialsProvider::new(chain),
        }
    }

    /// Builds the standard AWS credential chain for a named shared profile.
    pub async fn for_profile(profile: &str) -> Result<Self, S3Error> {
        if profile.is_empty() {
            return Err(S3Error::configuration("AWS profile name must not be empty"));
        }
        let chain = aws_config::default_provider::credentials::DefaultCredentialsChain::builder()
            .profile_name(profile)
            .build()
            .await;
        Ok(Self {
            inner: SharedCredentialsProvider::new(chain),
        })
    }
}

#[cfg(feature = "aws-credentials")]
impl fmt::Debug for AwsDefaultCredentialsProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AwsDefaultCredentialsProvider")
            .field("inner", &"[REDACTED]")
            .finish()
    }
}

#[cfg(feature = "aws-credentials")]
#[async_trait]
impl CredentialsProvider for AwsDefaultCredentialsProvider {
    async fn provide_credentials(&self) -> Result<Credentials, S3Error> {
        let credentials = self.inner.provide_credentials().await.map_err(|error| {
            S3Error::new(
                ErrorCategory::Authentication,
                "AWS default credential chain did not resolve credentials",
                RetryClassification::Never,
            )
            .with_source(error)
        })?;
        Credentials::with_expiration(
            credentials.access_key_id(),
            credentials.secret_access_key(),
            credentials.session_token().map(ToOwned::to_owned),
            credentials.expiry().map(OffsetDateTime::from),
        )
    }
}

/// Resolves a signing region through AWS's standard environment, shared
/// profile, and IMDS region chain.
///
/// # Errors
///
/// Returns an error when the chain does not resolve a region.
#[cfg(feature = "aws-credentials")]
pub async fn resolve_default_aws_region() -> Result<String, S3Error> {
    aws_config::default_provider::region::DefaultRegionChain::builder()
        .build()
        .region()
        .await
        .map(|region| region.as_ref().to_owned())
        .ok_or_else(|| S3Error::configuration("AWS default region chain did not resolve a region"))
}

/// Resolves a signing region through AWS's standard chain for a named profile.
///
/// # Errors
///
/// Returns an error for an empty profile or when the chain does not resolve a
/// region.
#[cfg(feature = "aws-credentials")]
pub async fn resolve_aws_region_for_profile(profile: &str) -> Result<String, S3Error> {
    if profile.is_empty() {
        return Err(S3Error::configuration("AWS profile name must not be empty"));
    }
    aws_config::default_provider::region::DefaultRegionChain::builder()
        .profile_name(profile)
        .build()
        .region()
        .await
        .map(|region| region.as_ref().to_owned())
        .ok_or_else(|| S3Error::configuration("AWS profile did not resolve a region"))
}

/// Provider backed by an immutable credential value.
#[derive(Clone)]
pub struct StaticCredentialsProvider {
    credentials: Credentials,
}

impl StaticCredentialsProvider {
    /// Creates a static provider.
    pub fn new(credentials: Credentials) -> Self {
        Self { credentials }
    }
}

impl fmt::Debug for StaticCredentialsProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StaticCredentialsProvider")
            .field("credentials", &"[REDACTED]")
            .finish()
    }
}

#[async_trait]
impl CredentialsProvider for StaticCredentialsProvider {
    async fn provide_credentials(&self) -> Result<Credentials, S3Error> {
        Ok(self.credentials.clone())
    }
}

/// Provider for the conventional AWS credential environment variables.
#[derive(Clone, Copy, Debug, Default)]
pub struct EnvironmentCredentialsProvider;

impl EnvironmentCredentialsProvider {
    /// Creates an environment credential provider.
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CredentialsProvider for EnvironmentCredentialsProvider {
    async fn provide_credentials(&self) -> Result<Credentials, S3Error> {
        let access_key_id = read_required_env("AWS_ACCESS_KEY_ID")?;
        let secret_access_key = read_required_env("AWS_SECRET_ACCESS_KEY")?;
        let session_token = read_optional_env("AWS_SESSION_TOKEN")?;
        Credentials::new(access_key_id, secret_access_key, session_token)
    }
}

fn read_required_env(name: &'static str) -> Result<String, S3Error> {
    read_optional_env(name)?.ok_or_else(|| {
        S3Error::new(
            ErrorCategory::Authentication,
            format!("required credential environment variable {name} is not set"),
            RetryClassification::Never,
        )
    })
}

fn read_optional_env(name: &'static str) -> Result<Option<String>, S3Error> {
    env::var_os(name)
        .map(|value| {
            value.into_string().map_err(|_| {
                S3Error::new(
                    ErrorCategory::Authentication,
                    format!("credential environment variable {name} is not valid Unicode"),
                    RetryClassification::Never,
                )
            })
        })
        .transpose()
}

/// Expiration-aware provider that coalesces concurrent refresh requests.
///
/// A refresh is performed while holding a Tokio mutex. Concurrent callers wait for
/// that single refresh and then consume the same cached result, preventing refresh
/// storms. If an early refresh fails, credentials that have not actually expired are
/// returned until a later call can retry.
pub struct CachedCredentialsProvider {
    provider: Arc<dyn CredentialsProvider>,
    refresh_before: Duration,
    cached: Mutex<Option<Credentials>>,
}

impl CachedCredentialsProvider {
    /// Wraps a provider and refreshes expiring credentials one minute early.
    pub fn new(provider: Arc<dyn CredentialsProvider>) -> Self {
        Self::with_refresh_before(provider, Duration::from_secs(60))
    }

    /// Wraps a provider with a custom early-refresh window.
    pub fn with_refresh_before(
        provider: Arc<dyn CredentialsProvider>,
        refresh_before: Duration,
    ) -> Self {
        Self {
            provider,
            refresh_before,
            cached: Mutex::new(None),
        }
    }

    fn refresh_deadline(&self, now: OffsetDateTime) -> OffsetDateTime {
        let refresh_before =
            time::Duration::try_from(self.refresh_before).unwrap_or(time::Duration::MAX);
        now.saturating_add(refresh_before)
    }

    /// Clears the current cache entry so the next request refreshes it.
    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }
}

impl fmt::Debug for CachedCredentialsProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CachedCredentialsProvider")
            .field("provider", &"[REDACTED]")
            .field("refresh_before", &self.refresh_before)
            .field("cached", &"[REDACTED]")
            .finish()
    }
}

#[async_trait]
impl CredentialsProvider for CachedCredentialsProvider {
    async fn provide_credentials(&self) -> Result<Credentials, S3Error> {
        let mut cached = self.cached.lock().await;
        let now = OffsetDateTime::now_utc();
        if let Some(credentials) = cached.as_ref()
            && !credentials.expires_by(self.refresh_deadline(now))
        {
            return Ok(credentials.clone());
        }

        match self.provider.provide_credentials().await {
            Ok(credentials) => {
                *cached = Some(credentials.clone());
                Ok(credentials)
            }
            Err(refresh_error) => {
                if let Some(credentials) = cached.as_ref()
                    && !credentials.expires_by(now)
                {
                    return Ok(credentials.clone());
                }
                Err(refresh_error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures_util::future::join_all;
    use proptest::prelude::*;

    use super::*;

    struct CountingProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl CredentialsProvider for CountingProvider {
        async fn provide_credentials(&self) -> Result<Credentials, S3Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            Credentials::new(
                "visible-access-id",
                "visible-secret",
                Some("visible-token".into()),
            )
        }
    }

    #[test]
    fn debug_redacts_all_credential_material() {
        let credentials = Credentials::new(
            "visible-access-id",
            "visible-secret",
            Some("visible-token".into()),
        )
        .unwrap();
        let debug = format!("{credentials:?}");
        assert!(!debug.contains("visible-access-id"));
        assert!(!debug.contains("visible-secret"));
        assert!(!debug.contains("visible-token"));
    }

    proptest! {
        #[test]
        fn arbitrary_credential_values_are_redacted(
            suffix in "[A-Za-z0-9]{12,32}"
        ) {
            let access = format!("ACCESS-{suffix}");
            let secret = format!("SECRET-{suffix}");
            let token = format!("TOKEN-{suffix}");
            let credentials = Credentials::new(
                access.clone(),
                secret.clone(),
                Some(token.clone()),
            ).unwrap();
            let debug = format!("{credentials:?}");
            prop_assert!(!debug.contains(&access));
            prop_assert!(!debug.contains(&secret));
            prop_assert!(!debug.contains(&token));
        }
    }

    #[tokio::test]
    async fn concurrent_cache_misses_share_one_refresh() {
        let provider = Arc::new(CountingProvider {
            calls: AtomicUsize::new(0),
        });
        let cached = Arc::new(CachedCredentialsProvider::new(provider.clone()));
        let requests = (0..32).map(|_| {
            let cached = cached.clone();
            async move { cached.provide_credentials().await.unwrap() }
        });

        let results = join_all(requests).await;
        assert_eq!(results.len(), 32);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}
