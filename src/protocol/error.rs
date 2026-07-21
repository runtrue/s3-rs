use serde::Deserialize;

use super::xml::{ProtocolError, expect_root, parse_bounded};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ParsedS3Error {
    pub code: Option<String>,
    pub message: Option<String>,
    pub request_id: Option<String>,
    pub host_id: Option<String>,
    pub resource: Option<String>,
    pub region: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename = "Error")]
struct ErrorDocument {
    #[serde(rename = "Code")]
    code: Option<String>,
    #[serde(rename = "Message")]
    message: Option<String>,
    #[serde(rename = "RequestId")]
    request_id: Option<String>,
    #[serde(rename = "HostId")]
    host_id: Option<String>,
    #[serde(rename = "Resource")]
    resource: Option<String>,
    #[serde(rename = "Region")]
    region: Option<String>,
}

pub(crate) fn parse_s3_error(body: &[u8], maximum: usize) -> Result<ParsedS3Error, ProtocolError> {
    expect_root(body, maximum, &[b"Error"])?;
    let parsed: ErrorDocument = parse_bounded(body, maximum)?;
    Ok(ParsedS3Error {
        code: parsed.code,
        message: parsed.message,
        request_id: parsed.request_id,
        host_id: parsed.host_id,
        resource: parsed.resource,
        region: parsed.region,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parses_namespaced_error_documents() {
        let body = br#"<Error xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Code>NoSuchKey</Code><Message>missing</Message><RequestId>req</RequestId><HostId>host</HostId></Error>"#;
        let parsed = parse_s3_error(body, 4_096).unwrap();
        assert_eq!(parsed.code.as_deref(), Some("NoSuchKey"));
        assert_eq!(parsed.request_id.as_deref(), Some("req"));
        assert_eq!(parsed.host_id.as_deref(), Some("host"));
    }

    proptest! {
        #[test]
        fn arbitrary_error_documents_never_panic(body in proptest::collection::vec(any::<u8>(), 0..2048)) {
            let _ = parse_s3_error(&body, 2_048);
        }
    }
}
