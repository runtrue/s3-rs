use http::{HeaderMap, HeaderName, HeaderValue};

use crate::error::S3Error;

pub(in crate::client) fn insert_header(
    headers: &mut HeaderMap,
    name: HeaderName,
    value: &str,
) -> Result<(), S3Error> {
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
