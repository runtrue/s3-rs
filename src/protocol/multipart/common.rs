use crate::operation::UploadId;
use crate::protocol::ProtocolError;

pub(super) fn parse_upload_id(
    value: String,
    field: &'static str,
) -> Result<UploadId, ProtocolError> {
    UploadId::new(value).map_err(|error| ProtocolError::InvalidField {
        field,
        reason: error.to_string(),
    })
}

pub(super) fn root_is_error(body: &[u8], maximum: usize) -> Result<bool, ProtocolError> {
    if body.len() > maximum {
        return Err(ProtocolError::Oversized {
            actual: body.len(),
            maximum,
        });
    }
    // XML declarations and whitespace are allowed. This scanner only chooses
    // the schema; the bounded parser still validates the complete document.
    let text =
        std::str::from_utf8(body).map_err(|error| ProtocolError::InvalidXml(error.to_string()))?;
    let mut reader = quick_xml::Reader::from_str(text);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(start) | quick_xml::events::Event::Empty(start)) => {
                return Ok(start.local_name().as_ref() == "Error");
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
