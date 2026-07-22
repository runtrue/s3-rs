//! Endpoint validation and exact request-target construction.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use http::Uri;
use http::uri::{Authority, Scheme};

use crate::error::S3Error;

/// How a bucket name is represented in request URLs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AddressingStyle {
    /// Put the bucket name in the request path.
    #[default]
    Path,
    /// Put the bucket name before the endpoint hostname.
    VirtualHosted,
}

/// An absolute endpoint URL whose path retains its exact wire representation.
///
/// Unlike a general-purpose browser URL, this value does not resolve dot segments
/// or collapse repeated slashes. This matters because every byte in an S3 object
/// key is significant and is covered by the request signature.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct EndpointUrl {
    serialized: String,
    scheme: String,
    authority: String,
    path_and_query: String,
}

impl EndpointUrl {
    fn from_parts(scheme: &str, authority: &str, path_and_query: &str) -> Result<Self, S3Error> {
        let serialized = format!("{scheme}://{authority}{path_and_query}");
        serialized
            .parse::<Uri>()
            .map_err(|_| S3Error::configuration("endpoint does not form a valid HTTP URI"))?;
        Ok(Self {
            serialized,
            scheme: scheme.to_owned(),
            authority: authority.to_owned(),
            path_and_query: path_and_query.to_owned(),
        })
    }

    /// Returns the URL scheme.
    pub(crate) fn scheme(&self) -> &str {
        &self.scheme
    }

    /// Returns the URL authority, including an explicit port when present.
    pub(crate) fn authority(&self) -> &str {
        &self.authority
    }

    /// Returns the exact encoded path and optional query sent on the wire.
    pub(crate) fn path_and_query(&self) -> &str {
        &self.path_and_query
    }

    /// Returns the complete absolute URL without changing its request target.
    pub(crate) fn as_str(&self) -> &str {
        &self.serialized
    }

    pub(crate) fn request_uri(&self) -> Uri {
        self.serialized
            .parse()
            .expect("EndpointUrl construction validates its HTTP URI")
    }

    pub(crate) fn with_query(&self, query: &str) -> Result<Self, S3Error> {
        if query.starts_with('?') || query.contains('#') {
            return Err(S3Error::configuration(
                "request query must not include '?' or a fragment",
            ));
        }
        let path = self
            .path_and_query
            .split_once('?')
            .map_or(self.path_and_query.as_str(), |(path, _)| path);
        let path_and_query = if query.is_empty() {
            path.to_owned()
        } else {
            format!("{path}?{query}")
        };
        Self::from_parts(&self.scheme, &self.authority, &path_and_query)
    }

    pub(crate) fn with_authority(&self, authority: &str) -> Result<Self, S3Error> {
        let parsed = Authority::from_str(authority)
            .map_err(|_| S3Error::configuration("redirect authority is invalid"))?;
        if parsed.as_str().contains('@') || parsed.host().is_empty() {
            return Err(S3Error::configuration("redirect authority is invalid"));
        }
        Self::from_parts(&self.scheme, parsed.as_str(), &self.path_and_query)
    }
}

impl fmt::Debug for EndpointUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (path, query) = self
            .path_and_query
            .split_once('?')
            .map_or((self.path_and_query.as_str(), false), |(path, _)| {
                (path, true)
            });
        formatter
            .debug_struct("EndpointUrl")
            .field("scheme", &self.scheme)
            .field("authority", &self.authority)
            .field("path", &path)
            .field("has_redacted_query", &query)
            .finish_non_exhaustive()
    }
}

/// A validated S3 service endpoint.
#[derive(Clone, Eq, PartialEq)]
pub struct Endpoint {
    url: EndpointUrl,
    base_path: String,
}

impl Endpoint {
    /// Parses and validates an HTTP or HTTPS endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error when the URL is not an absolute HTTP(S) URL, contains
    /// credentials, a query, or a fragment, or has no usable host.
    pub fn new(endpoint: impl AsRef<str>) -> Result<Self, S3Error> {
        let input = endpoint.as_ref();
        if input.contains('#') {
            return Err(S3Error::configuration(
                "endpoint must not contain a query string or fragment",
            ));
        }
        let uri = input
            .parse::<Uri>()
            .map_err(|_| S3Error::configuration("endpoint must be a valid absolute URL"))?;
        let scheme = uri
            .scheme()
            .ok_or_else(|| S3Error::configuration("endpoint must include a scheme"))?;
        if scheme != &Scheme::HTTPS && scheme != &Scheme::HTTP {
            return Err(S3Error::configuration(
                "endpoint scheme must be HTTPS or explicitly enabled HTTP",
            ));
        }
        let authority = uri
            .authority()
            .ok_or_else(|| S3Error::configuration("endpoint must include a host"))?;
        if authority.as_str().contains('@') {
            return Err(S3Error::configuration(
                "endpoint must not contain user information",
            ));
        }
        if authority.host().is_empty() {
            return Err(S3Error::configuration("endpoint must include a host"));
        }
        if uri
            .path_and_query()
            .is_some_and(|value| value.query().is_some())
        {
            return Err(S3Error::configuration(
                "endpoint must not contain a query string or fragment",
            ));
        }

        let mut base_path = uri.path().to_owned();
        if base_path.is_empty() {
            base_path.push('/');
        }
        if !base_path.starts_with('/') {
            return Err(S3Error::configuration("endpoint path must be absolute"));
        }
        if !base_path.ends_with('/') {
            base_path.push('/');
        }
        let url = EndpointUrl::from_parts(scheme.as_str(), authority.as_str(), &base_path)?;
        Ok(Self { url, base_path })
    }

    /// Creates the standard regional AWS S3 endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error when `region` cannot be represented safely in an AWS
    /// regional hostname.
    pub fn for_aws_region(region: &str) -> Result<Self, S3Error> {
        validate_region(region)?;
        Self::new(format!("https://s3.{region}.amazonaws.com"))
    }

    /// Returns the validated endpoint as an absolute URL string.
    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    pub(crate) fn url(&self) -> &EndpointUrl {
        &self.url
    }

    /// Returns whether this endpoint uses HTTPS.
    pub fn is_https(&self) -> bool {
        self.url.scheme() == "https"
    }

    /// Builds an encoded bucket or object URL using the requested addressing style.
    ///
    /// # Errors
    ///
    /// Returns an error when the bucket is invalid for the selected addressing
    /// style or the resulting URL is not a valid absolute HTTP URI.
    pub(crate) fn object_url(
        &self,
        bucket: &str,
        object_key: Option<&str>,
        style: AddressingStyle,
    ) -> Result<EndpointUrl, S3Error> {
        validate_bucket_for_path(bucket)?;
        let authority = if style == AddressingStyle::VirtualHosted {
            validate_virtual_host_bucket(bucket)?;
            let parsed = Authority::from_str(self.url.authority())
                .map_err(|_| S3Error::configuration("endpoint authority is invalid"))?;
            if IpAddr::from_str(parsed.host().trim_matches(['[', ']'])).is_ok() {
                return Err(S3Error::configuration(
                    "virtual-hosted addressing cannot be used with an IP endpoint",
                ));
            }
            parsed.port_u16().map_or_else(
                || format!("{bucket}.{}", parsed.host()),
                |port| format!("{bucket}.{}:{port}", parsed.host()),
            )
        } else {
            self.url.authority().to_owned()
        };

        let mut path = self.base_path.clone();
        if style == AddressingStyle::Path {
            encode_path_component(bucket.as_bytes(), &mut path);
        }
        if let Some(key) = object_key {
            if style == AddressingStyle::Path && !path.ends_with('/') {
                path.push('/');
            }
            encode_object_key(key.as_bytes(), &mut path);
        } else if style == AddressingStyle::VirtualHosted && path.len() > 1 {
            path.pop();
        }
        EndpointUrl::from_parts(self.url.scheme(), &authority, &path)
    }
}

impl Default for Endpoint {
    fn default() -> Self {
        Self::new("https://s3.amazonaws.com").expect("constant AWS endpoint is valid")
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Endpoint").field(&self.url).finish()
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.url.as_str())
    }
}

impl FromStr for Endpoint {
    type Err = S3Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

fn encode_object_key(bytes: &[u8], output: &mut String) {
    for &byte in bytes {
        if byte == b'/' || is_unreserved(byte) {
            output.push(char::from(byte));
        } else {
            push_percent_encoded(byte, output);
        }
    }
}

fn encode_path_component(bytes: &[u8], output: &mut String) {
    for &byte in bytes {
        if is_unreserved(byte) {
            output.push(char::from(byte));
        } else {
            push_percent_encoded(byte, output);
        }
    }
}

const fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn push_percent_encoded(byte: u8, output: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    output.push('%');
    output.push(char::from(HEX[usize::from(byte >> 4)]));
    output.push(char::from(HEX[usize::from(byte & 0x0f)]));
}

fn validate_region(region: &str) -> Result<(), S3Error> {
    if region.is_empty()
        || region.len() > 64
        || !region
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(S3Error::configuration("AWS region is invalid"));
    }
    Ok(())
}

fn validate_bucket_for_path(bucket: &str) -> Result<(), S3Error> {
    if bucket.is_empty()
        || bucket.len() > 255
        || bucket
            .bytes()
            .any(|byte| byte == b'/' || byte == b'\\' || byte.is_ascii_control())
    {
        return Err(S3Error::configuration("bucket name is invalid"));
    }
    Ok(())
}

fn validate_virtual_host_bucket(bucket: &str) -> Result<(), S3Error> {
    if !(3..=63).contains(&bucket.len())
        || bucket.starts_with(['-', '.'])
        || bucket.ends_with(['-', '.'])
        || bucket.contains("..")
        || !bucket.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
        })
        || IpAddr::from_str(bucket).is_ok()
    {
        return Err(S3Error::configuration(
            "bucket is not valid for virtual-hosted addressing",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rejects_credential_bearing_and_ambiguous_endpoints() {
        assert!(Endpoint::new("https://user:password@s3.example.test").is_err());
        assert!(Endpoint::new("https://s3.example.test?target=elsewhere").is_err());
        assert!(Endpoint::new("https://s3.example.test/#fragment").is_err());
        assert!(Endpoint::new("ftp://s3.example.test").is_err());
    }

    #[test]
    fn constructs_both_addressing_styles() {
        let endpoint = Endpoint::new("https://storage.example.test/api").unwrap();
        let path = endpoint
            .object_url("my-bucket", Some("folder/a b"), AddressingStyle::Path)
            .unwrap();
        assert_eq!(
            path.as_str(),
            "https://storage.example.test/api/my-bucket/folder/a%20b"
        );

        let hosted = endpoint
            .object_url(
                "my-bucket",
                Some("folder/a b"),
                AddressingStyle::VirtualHosted,
            )
            .unwrap();
        assert_eq!(
            hosted.as_str(),
            "https://my-bucket.storage.example.test/api/folder/a%20b"
        );
    }

    #[test]
    fn exact_s3_key_paths_are_never_normalized() {
        let endpoint = Endpoint::new("https://storage.example.test:9443/root//").unwrap();
        for (key, expected) in [
            (".", "/root//bucket/."),
            ("..", "/root//bucket/.."),
            ("a/../b", "/root//bucket/a/../b"),
            ("//a///b", "/root//bucket///a///b"),
        ] {
            let url = endpoint
                .object_url("bucket", Some(key), AddressingStyle::Path)
                .unwrap();
            assert_eq!(url.scheme(), "https");
            assert_eq!(url.authority(), "storage.example.test:9443");
            assert_eq!(url.path_and_query(), expected);
            assert_eq!(
                url.request_uri().path_and_query().unwrap().as_str(),
                expected
            );
        }
    }

    #[test]
    fn query_is_appended_without_changing_path_or_origin() {
        let url = Endpoint::new("https://storage.example.test")
            .unwrap()
            .object_url("bucket", Some("a/../b"), AddressingStyle::Path)
            .unwrap()
            .with_query("partNumber=7&uploadId=a%2Fb")
            .unwrap();
        assert_eq!(url.authority(), "storage.example.test");
        assert_eq!(
            url.path_and_query(),
            "/bucket/a/../b?partNumber=7&uploadId=a%2Fb"
        );
        assert_eq!(
            url.request_uri().path_and_query().unwrap().as_str(),
            "/bucket/a/../b?partNumber=7&uploadId=a%2Fb"
        );
    }

    proptest! {
        #[test]
        fn encoded_object_keys_do_not_change_origin(key in "[A-Za-z0-9 ./_%+-]{0,80}") {
            let endpoint = Endpoint::new("https://storage.example.test").unwrap();
            let url = endpoint.object_url("bucket", Some(&key), AddressingStyle::Path).unwrap();
            prop_assert_eq!(url.scheme(), "https");
            prop_assert_eq!(url.authority(), "storage.example.test");
            prop_assert!(!url.path_and_query().contains('?'));
            prop_assert!(!url.path_and_query().contains('#'));
        }
    }
}
