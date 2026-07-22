use std::time::Duration;

use secrecy::ExposeSecret;
use time::OffsetDateTime;

use super::S3Client;
use crate::error::{ErrorCategory, RetryClassification, S3Error};
use crate::operation::{ObjectKey, PresignedUrl};
use crate::signing::{
    Header, PresigningRequest, SigningCredentials, SigningPath, SigningScope, sign_presigned,
};

impl S3Client {
    /// Generates a presigned GET URL whose standard formatting is redacted.
    pub async fn presigned_get(
        &self,
        key: &ObjectKey,
        expires: Duration,
    ) -> Result<PresignedUrl, S3Error> {
        self.presign("GET", key, expires).await
    }

    /// Generates a presigned PUT URL whose standard formatting is redacted.
    pub async fn presigned_put(
        &self,
        key: &ObjectKey,
        expires: Duration,
    ) -> Result<PresignedUrl, S3Error> {
        self.presign("PUT", key, expires).await
    }

    async fn presign(
        &self,
        method: &'static str,
        key: &ObjectKey,
        expires: Duration,
    ) -> Result<PresignedUrl, S3Error> {
        let target = self.operation_target(Some(key.as_str()))?;
        let credentials = self
            .inner
            .config
            .credentials_provider()
            .provide_credentials()
            .await?;
        if credentials.expires_by(OffsetDateTime::now_utc()) {
            return Err(S3Error::new(
                ErrorCategory::Authentication,
                "credential provider returned expired credentials",
                RetryClassification::Never,
            ));
        }
        let secret = credentials.secret_access_key().expose_secret();
        let session_token = credentials.session_token().map(ExposeSecret::expose_secret);
        let signing_credentials = SigningCredentials::new(
            credentials.access_key_id(),
            secret.as_bytes(),
            session_token,
        );
        let headers = [Header::new("host", target.authority())];
        let signing_request = PresigningRequest {
            method,
            uri_path: SigningPath::encoded(target.path_and_query()),
            query: &[],
            headers: &headers,
            expires,
            payload_hash: None,
        };
        let query: crate::signing::PresignedQuery = sign_presigned(
            &signing_credentials,
            SigningScope::new(self.inner.config.region(), "s3"),
            &signing_request,
            OffsetDateTime::now_utc(),
        )
        .map_err(|error| {
            S3Error::new(
                ErrorCategory::Authentication,
                "presigned URL generation failed",
                RetryClassification::Never,
            )
            .with_source(error)
        })?;
        Ok(PresignedUrl::new(format!(
            "{}?{}",
            target.as_str(),
            query.as_str()
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::config::S3Config;
    use crate::credentials::{Credentials, StaticCredentialsProvider};

    #[tokio::test]
    async fn presigned_output_requires_explicit_exposure() {
        let credentials = Credentials::new("access", "secret", None).unwrap();
        let config = S3Config::builder()
            .bucket("bucket")
            .credentials_provider(Arc::new(StaticCredentialsProvider::new(credentials)))
            .build()
            .unwrap();
        let client = S3Client::new(config).unwrap();
        let url = client
            .presigned_get(&ObjectKey::new("a/../b").unwrap(), Duration::from_secs(60))
            .await
            .unwrap();
        assert!(!format!("{url:?}").contains("X-Amz-Signature"));
        assert!(url.expose().contains("/bucket/a/../b?"));
        assert!(url.expose().contains("X-Amz-Signature="));
    }
}
