use std::time::Duration;

use secrecy::ExposeSecret;
use time::OffsetDateTime;

use super::S3Client;
use crate::error::{ErrorCategory, RetryClassification, S3Error};
use crate::operation::{ObjectKey, PartNumber, PresignedUrl, UploadId};
use crate::signing::{
    Header, PresigningRequest, QueryParam, SigningCredentials, SigningPath, SigningScope,
    sign_presigned,
};

impl S3Client {
    /// Generates a presigned GET URL whose standard formatting is redacted.
    pub async fn presigned_get(
        &self,
        key: &ObjectKey,
        expires: Duration,
    ) -> Result<PresignedUrl, S3Error> {
        self.presign("GET", key, &[], expires).await
    }

    /// Generates a presigned PUT URL whose standard formatting is redacted.
    pub async fn presigned_put(
        &self,
        key: &ObjectKey,
        expires: Duration,
    ) -> Result<PresignedUrl, S3Error> {
        self.presign("PUT", key, &[], expires).await
    }

    /// Generates a presigned HEAD URL whose standard formatting is redacted.
    pub async fn presigned_head(
        &self,
        key: &ObjectKey,
        expires: Duration,
    ) -> Result<PresignedUrl, S3Error> {
        self.presign("HEAD", key, &[], expires).await
    }

    /// Generates a presigned DELETE URL whose standard formatting is redacted.
    pub async fn presigned_delete(
        &self,
        key: &ObjectKey,
        expires: Duration,
    ) -> Result<PresignedUrl, S3Error> {
        self.presign("DELETE", key, &[], expires).await
    }

    /// Generates a presigned request that initiates a multipart upload.
    pub async fn presigned_create_multipart_upload(
        &self,
        key: &ObjectKey,
        expires: Duration,
    ) -> Result<PresignedUrl, S3Error> {
        self.presign("POST", key, &[("uploads", "")], expires).await
    }

    /// Generates a presigned multipart part-upload URL.
    pub async fn presigned_upload_part(
        &self,
        key: &ObjectKey,
        upload_id: &UploadId,
        part_number: PartNumber,
        expires: Duration,
    ) -> Result<PresignedUrl, S3Error> {
        let part_number = part_number.get().to_string();
        self.presign(
            "PUT",
            key,
            &[
                ("partNumber", part_number.as_str()),
                ("uploadId", upload_id.as_str()),
            ],
            expires,
        )
        .await
    }

    /// Generates a presigned multipart abort URL.
    pub async fn presigned_abort_multipart_upload(
        &self,
        key: &ObjectKey,
        upload_id: &UploadId,
        expires: Duration,
    ) -> Result<PresignedUrl, S3Error> {
        self.presign("DELETE", key, &[("uploadId", upload_id.as_str())], expires)
            .await
    }

    async fn presign(
        &self,
        method: &'static str,
        key: &ObjectKey,
        query: &[(&str, &str)],
        expires: Duration,
    ) -> Result<PresignedUrl, S3Error> {
        let target = self.operation_target(Some(key.as_str()))?;
        let credentials = self
            .inner
            .config
            .credentials_provider()
            .provide_credentials()
            .await?;
        let now = OffsetDateTime::now_utc();
        if credentials.expires_by(now) {
            return Err(S3Error::new(
                ErrorCategory::Authentication,
                "credential provider returned expired credentials",
                RetryClassification::Never,
            ));
        }
        let expires_at = time::Duration::try_from(expires)
            .ok()
            .and_then(|duration| now.checked_add(duration));
        if credentials.expires_at().is_some_and(|credential_expiry| {
            expires_at.is_none_or(|url_expiry| url_expiry >= credential_expiry)
        }) {
            return Err(S3Error::new(
                ErrorCategory::Authentication,
                "presigned URL lifetime must end before the credentials expire",
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
        let query = query
            .iter()
            .map(|(name, value)| QueryParam::new(name, value))
            .collect::<Vec<_>>();
        let signing_request = PresigningRequest {
            method,
            uri_path: SigningPath::encoded(target.path_and_query()),
            query: &query,
            headers: &headers,
            expires,
            payload_hash: None,
        };
        let query: crate::signing::PresignedQuery = sign_presigned(
            &signing_credentials,
            SigningScope::new(self.inner.config.region(), "s3"),
            &signing_request,
            now,
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

        let upload_id = UploadId::new("opaque/upload+id").unwrap();
        let multipart = client
            .presigned_upload_part(
                &ObjectKey::new("multipart").unwrap(),
                &upload_id,
                PartNumber::new(7).unwrap(),
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        assert!(multipart.expose().contains("partNumber=7"));
        assert!(multipart.expose().contains("uploadId=opaque%2Fupload%2Bid"));
        assert!(multipart.expose().contains("X-Amz-Signature="));
    }

    #[tokio::test]
    async fn presigned_url_must_expire_before_session_credentials() {
        let credentials = Credentials::with_expiration(
            "access",
            "secret",
            Some("session".to_owned()),
            Some(OffsetDateTime::now_utc() + time::Duration::minutes(5)),
        )
        .unwrap();
        let config = S3Config::builder()
            .bucket("bucket")
            .credentials_provider(Arc::new(StaticCredentialsProvider::new(credentials)))
            .build()
            .unwrap();
        let client = S3Client::new(config).unwrap();

        let error = client
            .presigned_get(&ObjectKey::new("key").unwrap(), Duration::from_secs(5 * 60))
            .await
            .expect_err("URL outliving credentials is rejected");

        assert_eq!(error.category(), ErrorCategory::Authentication);
    }
}
