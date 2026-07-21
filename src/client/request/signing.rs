use http::header::{AUTHORIZATION, CONTENT_LENGTH, HOST};
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use secrecy::ExposeSecret;
use time::OffsetDateTime;

use super::super::S3Client;
use crate::endpoint::EndpointUrl;
use crate::error::{ErrorCategory, RetryClassification, S3Error};
use crate::signing::{
    Header, HeaderSigningRequest, QueryParam, SigningCredentials, SigningPath, SigningScope,
    sign_headers,
};
use crate::stream::{PreparedBody, TransportBody};

pub(super) struct SignedRequestInput<'a> {
    pub(super) method: Method,
    pub(super) target: &'a EndpointUrl,
    pub(super) query: &'a [QueryParam<'a>],
    pub(super) headers: HeaderMap,
    pub(super) body: Option<&'a PreparedBody>,
    pub(super) payload_hash: &'a str,
    pub(super) signing_region: &'a str,
}

impl S3Client {
    pub(super) async fn signed_request(
        &self,
        input: SignedRequestInput<'_>,
    ) -> Result<http::Request<TransportBody>, S3Error> {
        let SignedRequestInput {
            method,
            target,
            query,
            mut headers,
            body,
            payload_hash,
            signing_region,
        } = input;
        headers.insert(
            HOST,
            HeaderValue::from_str(target.authority())
                .map_err(|_| S3Error::configuration("endpoint authority is not a valid header"))?,
        );
        headers.insert(
            HeaderName::from_static("x-amz-content-sha256"),
            HeaderValue::from_str(payload_hash)
                .map_err(|_| S3Error::configuration("payload digest is not a valid header"))?,
        );
        if let Some(body) = body {
            headers.insert(CONTENT_LENGTH, HeaderValue::from(body.length()));
        }

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

        let owned_headers = headers
            .iter()
            .map(|(name, value)| {
                value
                    .to_str()
                    .map(|value| (name.as_str().to_owned(), value.to_owned()))
                    .map_err(|_| S3Error::configuration("a signed header contains non-ASCII bytes"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let signing_headers = owned_headers
            .iter()
            .map(|(name, value)| Header::new(name, value))
            .collect::<Vec<_>>();
        let secret = credentials.secret_access_key().expose_secret();
        let session_token = credentials.session_token().map(ExposeSecret::expose_secret);
        let signing_credentials = SigningCredentials::new(
            credentials.access_key_id(),
            secret.as_bytes(),
            session_token,
        );
        let path = target
            .path_and_query()
            .split_once('?')
            .map_or(target.path_and_query(), |(path, _)| path);
        let signing_request = HeaderSigningRequest {
            method: method.as_str(),
            uri_path: SigningPath::encoded(path),
            query,
            headers: &signing_headers,
            payload_hash,
        };
        let signed: crate::signing::HeaderSigningOutput = sign_headers(
            &signing_credentials,
            SigningScope::new(signing_region, "s3"),
            &signing_request,
            OffsetDateTime::now_utc(),
        )
        .map_err(|error| {
            S3Error::new(
                ErrorCategory::Authentication,
                "request signing failed",
                RetryClassification::Never,
            )
            .with_source(error)
        })?;
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(signed.authorization())
                .map_err(|_| S3Error::configuration("generated authorization header is invalid"))?,
        );
        headers.insert(
            HeaderName::from_static("x-amz-date"),
            HeaderValue::from_str(signed.amz_date())
                .map_err(|_| S3Error::configuration("generated signing date is invalid"))?,
        );
        if let Some(token) = signed.security_token() {
            headers.insert(
                HeaderName::from_static("x-amz-security-token"),
                HeaderValue::from_str(token).map_err(|_| {
                    S3Error::configuration("generated session token header is invalid")
                })?,
            );
        }

        let request_body = match body {
            Some(body) => body.request_body().await?,
            None => TransportBody::empty(),
        };
        let mut request = http::Request::builder()
            .method(method)
            .uri(target.request_uri())
            .body(request_body)
            .map_err(|error| {
                S3Error::configuration("signed request could not be constructed").with_source(error)
            })?;
        *request.headers_mut() = headers;
        Ok(request)
    }
}
