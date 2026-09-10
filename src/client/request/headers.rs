use http::{HeaderMap, HeaderName, HeaderValue};

use crate::error::S3Error;

const PROTECTED_HEADERS: [&str; 7] = [
    "host",
    "authorization",
    "x-amz-date",
    "x-amz-security-token",
    "x-amz-content-sha256",
    "content-length",
    "transfer-encoding",
];

pub(in crate::client) fn request_headers(headers: HeaderMap) -> Result<HeaderMap, S3Error> {
    for name in PROTECTED_HEADERS {
        if headers.contains_key(name) {
            return Err(header_collision(name));
        }
    }
    Ok(headers)
}

pub(in crate::client) fn insert_header(
    headers: &mut HeaderMap,
    name: HeaderName,
    value: &str,
) -> Result<(), S3Error> {
    if headers.contains_key(&name) {
        return Err(header_collision(name.as_str()));
    }
    let value = HeaderValue::from_str(value)
        .map_err(|_| S3Error::configuration("request contains an invalid header value"))?;
    headers.insert(name, value);
    Ok(())
}

pub(in crate::client) fn insert_optional_header(
    headers: &mut HeaderMap,
    name: HeaderName,
    value: Option<&str>,
) -> Result<(), S3Error> {
    if let Some(value) = value {
        insert_header(headers, name, value)?;
    }
    Ok(())
}

pub(in crate::client) fn insert_named_header(
    headers: &mut HeaderMap,
    name: &str,
    value: &str,
) -> Result<(), S3Error> {
    let name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| S3Error::configuration("request contains an invalid header name"))?;
    insert_header(headers, name, value)
}

pub(in crate::client) fn insert_optional_named_header(
    headers: &mut HeaderMap,
    name: &str,
    value: Option<&str>,
) -> Result<(), S3Error> {
    if let Some(value) = value {
        insert_named_header(headers, name, value)?;
    }
    Ok(())
}

fn header_collision(name: &str) -> S3Error {
    S3Error::configuration(format!(
        "request header `{name}` conflicts with a generated, signing-owned, or transport-owned header"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_names_are_case_insensitive_and_values_are_not_reported() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Amz-Date", HeaderValue::from_static("sentinel-secret"));
        let error = request_headers(headers).unwrap_err();
        assert!(error.message().contains("x-amz-date"));
        assert!(!error.message().contains("sentinel-secret"));
    }

    #[test]
    fn custom_duplicates_are_preserved() {
        let mut headers = HeaderMap::new();
        headers.append("x-example", HeaderValue::from_static("one"));
        headers.append("x-example", HeaderValue::from_static("two"));
        let headers = request_headers(headers).unwrap();
        assert_eq!(headers.get_all("x-example").iter().count(), 2);
    }

    #[test]
    fn generated_headers_reject_custom_collisions() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("sentinel-secret"));
        let error = insert_header(
            &mut headers,
            HeaderName::from_static("content-type"),
            "text/plain",
        )
        .unwrap_err();
        assert!(error.message().contains("content-type"));
        assert!(!error.message().contains("sentinel-secret"));
    }

    #[test]
    fn unset_optional_headers_do_not_reserve_their_names() {
        let mut headers = HeaderMap::new();
        headers.insert("if-match", HeaderValue::from_static("custom"));
        insert_optional_header(&mut headers, HeaderName::from_static("if-match"), None).unwrap();
        assert_eq!(headers["if-match"], "custom");
    }
}
