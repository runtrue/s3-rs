use std::fmt;

use serde::{Serialize, de::DeserializeOwned};

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProtocolError {
    #[error("XML response is {actual} bytes; configured maximum is {maximum}")]
    Oversized { actual: usize, maximum: usize },
    #[error("invalid S3 XML: {0}")]
    InvalidXml(String),
    #[error("invalid S3 response field {field}: {reason}")]
    InvalidField { field: &'static str, reason: String },
}

pub(super) fn parse_bounded<T: DeserializeOwned>(
    body: &[u8],
    maximum: usize,
) -> Result<T, ProtocolError> {
    if body.len() > maximum {
        return Err(ProtocolError::Oversized {
            actual: body.len(),
            maximum,
        });
    }
    reject_dangerous_declarations(body)?;
    let text =
        std::str::from_utf8(body).map_err(|error| ProtocolError::InvalidXml(error.to_string()))?;
    quick_xml::de::from_str(text).map_err(|error| ProtocolError::InvalidXml(error.to_string()))
}

pub(super) fn expect_root(
    body: &[u8],
    maximum: usize,
    expected: &[&[u8]],
) -> Result<(), ProtocolError> {
    if body.len() > maximum {
        return Err(ProtocolError::Oversized {
            actual: body.len(),
            maximum,
        });
    }
    reject_dangerous_declarations(body)?;
    let text =
        std::str::from_utf8(body).map_err(|error| ProtocolError::InvalidXml(error.to_string()))?;
    let mut reader = quick_xml::Reader::from_str(text);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(start) | quick_xml::events::Event::Empty(start)) => {
                let local_name = start.local_name();
                if expected.contains(&local_name.as_ref()) {
                    return Ok(());
                }
                return Err(ProtocolError::InvalidXml(format!(
                    "unexpected root element {}",
                    String::from_utf8_lossy(local_name.as_ref())
                )));
            }
            Ok(
                quick_xml::events::Event::Decl(_)
                | quick_xml::events::Event::Text(_)
                | quick_xml::events::Event::Comment(_)
                | quick_xml::events::Event::PI(_),
            ) => {}
            Ok(quick_xml::events::Event::DocType(_)) => {
                return Err(ProtocolError::InvalidXml(
                    "DTD declarations are not accepted".to_owned(),
                ));
            }
            Ok(quick_xml::events::Event::Eof) => {
                return Err(ProtocolError::InvalidXml("missing root element".to_owned()));
            }
            Ok(_) => {}
            Err(error) => return Err(ProtocolError::InvalidXml(error.to_string())),
        }
    }
}

pub(super) fn serialize_bounded<T: Serialize>(
    value: &T,
    maximum: usize,
) -> Result<Vec<u8>, ProtocolError> {
    let mut output = LimitedString::new(maximum);
    quick_xml::se::to_writer(&mut output, value).map_err(|error| {
        if output.exceeded {
            ProtocolError::Oversized {
                actual: maximum.saturating_add(1),
                maximum,
            }
        } else {
            ProtocolError::InvalidXml(error.to_string())
        }
    })?;
    Ok(output.value.into_bytes())
}

fn reject_dangerous_declarations(body: &[u8]) -> Result<(), ProtocolError> {
    // quick-xml does not retrieve external resources, but explicitly rejecting
    // DTDs and entity declarations keeps that invariant independent of parser
    // configuration and prevents entity-expansion behavior from being enabled.
    if ascii_contains_ignore_case(body, b"<!DOCTYPE")
        || ascii_contains_ignore_case(body, b"<!ENTITY")
    {
        return Err(ProtocolError::InvalidXml(
            "DTD and entity declarations are not accepted".to_owned(),
        ));
    }
    Ok(())
}

fn ascii_contains_ignore_case(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}

struct LimitedString {
    value: String,
    maximum: usize,
    exceeded: bool,
}

impl LimitedString {
    fn new(maximum: usize) -> Self {
        Self {
            value: String::with_capacity(maximum.min(8 * 1_024)),
            maximum,
            exceeded: false,
        }
    }
}

impl fmt::Write for LimitedString {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let new_length = self
            .value
            .len()
            .checked_add(value.len())
            .ok_or(fmt::Error)?;
        if new_length > self.maximum {
            self.exceeded = true;
            return Err(fmt::Error);
        }
        self.value.push_str(value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, Deserialize, Serialize)]
    #[serde(rename = "Value")]
    struct Value {
        #[serde(rename = "$text")]
        text: String,
    }

    #[test]
    fn parsing_enforces_the_bound_before_decoding() {
        let error = parse_bounded::<Value>(b"<Value>content</Value>", 4).unwrap_err();
        assert!(matches!(error, ProtocolError::Oversized { .. }));
    }

    #[test]
    fn dtd_and_entities_are_rejected() {
        let body = b"<!DOCTYPE Value [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><Value>&x;</Value>";
        assert!(matches!(
            parse_bounded::<Value>(body, body.len()),
            Err(ProtocolError::InvalidXml(_))
        ));
    }

    #[test]
    fn serialization_never_grows_beyond_its_bound() {
        let value = Value {
            text: "content".to_owned(),
        };
        assert!(matches!(
            serialize_bounded(&value, 4),
            Err(ProtocolError::Oversized { .. })
        ));
        assert_eq!(
            serialize_bounded(&value, 64).unwrap(),
            b"<Value>content</Value>"
        );
    }
}
