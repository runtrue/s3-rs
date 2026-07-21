use std::collections::BTreeMap;

use super::SigningError;

/// One unencoded query-string name and value.
#[derive(Clone, Copy)]
pub(crate) struct QueryParam<'a> {
    /// Query-string name before URI encoding.
    pub(crate) name: &'a str,
    /// Query-string value before URI encoding.
    pub(crate) value: &'a str,
}

impl<'a> QueryParam<'a> {
    pub(crate) const fn new(name: &'a str, value: &'a str) -> Self {
        Self { name, value }
    }
}

/// One header name and value before `SigV4` normalization.
#[derive(Clone, Copy)]
pub(crate) struct Header<'a> {
    /// Header name.
    pub(crate) name: &'a str,
    /// Header value.
    pub(crate) value: &'a str,
}

impl<'a> Header<'a> {
    pub(crate) const fn new(name: &'a str, value: &'a str) -> Self {
        Self { name, value }
    }
}

/// Canonical header block and its matching semicolon-separated name list.
pub(crate) struct CanonicalHeaders {
    canonical: String,
    signed: String,
}

impl CanonicalHeaders {
    pub(crate) fn canonical(&self) -> &str {
        &self.canonical
    }

    pub(crate) fn signed(&self) -> &str {
        &self.signed
    }
}

/// Apply the AWS percent-encoding rules to UTF-8 bytes.
///
/// Unlike a form-url encoder, this always encodes spaces as `%20`, encodes
/// `+`, and emits uppercase hexadecimal digits. `encode_slash` is false for
/// complete S3 object paths and true for query names and values.
pub(crate) fn aws_uri_encode(input: &str, encode_slash: bool) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~')
            || (byte == b'/' && !encode_slash)
        {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

/// Construct the canonical URI without normalizing repeated slashes or dot
/// segments. S3 treats those bytes as part of the object key.
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) fn canonical_uri(path: &str) -> String {
    if path.is_empty() {
        return "/".to_owned();
    }

    let encoded = aws_uri_encode(path, false);
    if encoded.starts_with('/') {
        encoded
    } else {
        format!("/{encoded}")
    }
}

/// Construct a canonical URI from an already URL-encoded wire path.
///
/// Valid percent triplets are retained (and their hexadecimal digits are
/// uppercased) instead of encoding `%` a second time. Other bytes still receive
/// the AWS encoding treatment. This is the correct entry point for
/// [`url::Url::path`], which returns an encoded path.
pub(crate) fn canonical_uri_from_encoded(path: &str) -> Result<String, SigningError> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut canonical = String::with_capacity(path.len().max(1));
    if !path.starts_with('/') {
        canonical.push('/');
    }
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            let Some(high) = bytes.get(index + 1).and_then(|value| hex_value(*value)) else {
                return Err(SigningError::InvalidEncodedUri);
            };
            let Some(low) = bytes.get(index + 2).and_then(|value| hex_value(*value)) else {
                return Err(SigningError::InvalidEncodedUri);
            };
            canonical.push('%');
            canonical.push(char::from(HEX[usize::from(high)]));
            canonical.push(char::from(HEX[usize::from(low)]));
            index += 3;
        } else if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            canonical.push(char::from(byte));
            index += 1;
        } else {
            canonical.push_str(&aws_uri_encode(
                // Consume one complete UTF-8 character when a caller supplies
                // a non-ASCII path despite the encoded-path contract.
                path.get(index..)
                    .and_then(|remaining| remaining.chars().next())
                    .map_or("", |character| &path[index..index + character.len_utf8()]),
                true,
            ));
            index += path[index..].chars().next().map_or(1, char::len_utf8);
        }
    }
    if canonical.is_empty() {
        canonical.push('/');
    }
    Ok(canonical)
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Encode and sort query parameters by encoded name and then encoded value.
/// Duplicate name/value pairs are deliberately retained.
pub(crate) fn canonical_query(parameters: &[QueryParam<'_>]) -> String {
    let mut encoded = parameters
        .iter()
        .map(|parameter| {
            (
                aws_uri_encode(parameter.name, true),
                aws_uri_encode(parameter.value, true),
            )
        })
        .collect::<Vec<_>>();
    encoded.sort_unstable();

    let capacity = encoded
        .iter()
        .map(|(name, value)| name.len() + value.len() + 2)
        .sum();
    let mut canonical = String::with_capacity(capacity);
    for (index, (name, value)) in encoded.into_iter().enumerate() {
        if index != 0 {
            canonical.push('&');
        }
        canonical.push_str(&name);
        canonical.push('=');
        canonical.push_str(&value);
    }
    canonical
}

/// Lowercase, normalize, combine, and order headers for `SigV4`.
pub(crate) fn canonical_headers(headers: &[Header<'_>]) -> Result<CanonicalHeaders, SigningError> {
    let mut ordered = BTreeMap::<String, Vec<String>>::new();
    for header in headers {
        if !valid_header_name(header.name) {
            return Err(SigningError::InvalidHeaderName);
        }
        let value = normalize_header_value(header.value)?;
        ordered
            .entry(header.name.to_ascii_lowercase())
            .or_default()
            .push(value);
    }

    if ordered.is_empty() {
        return Err(SigningError::MissingHostHeader);
    }

    let mut canonical = String::new();
    let mut signed = String::new();
    for (index, (name, values)) in ordered.iter().enumerate() {
        if index != 0 {
            signed.push(';');
        }
        signed.push_str(name);
        canonical.push_str(name);
        canonical.push(':');
        canonical.push_str(&values.join(","));
        canonical.push('\n');
    }

    match ordered.get("host") {
        None => return Err(SigningError::MissingHostHeader),
        Some(values) if values.len() != 1 || values[0].is_empty() => {
            return Err(SigningError::InvalidHostHeader);
        }
        Some(_) => {}
    }

    Ok(CanonicalHeaders { canonical, signed })
}

fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn normalize_header_value(value: &str) -> Result<String, SigningError> {
    if value
        .bytes()
        .any(|byte| byte.is_ascii_control() && byte != b'\t')
    {
        return Err(SigningError::InvalidHeaderValue);
    }

    let mut normalized = String::with_capacity(value.len());
    let mut pending_space = false;
    for character in value.chars() {
        if matches!(character, ' ' | '\t') {
            pending_space = !normalized.is_empty();
        } else {
            if pending_space {
                normalized.push(' ');
                pending_space = false;
            }
            normalized.push(character);
        }
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn encodes_aws_documentation_examples() {
        assert_eq!(aws_uri_encode("abcXYZ-._~", true), "abcXYZ-._~");
        assert_eq!(aws_uri_encode("a b+c/%", true), "a%20b%2Bc%2F%25");
        assert_eq!(aws_uri_encode("a b+c/%", false), "a%20b%2Bc/%25");
        assert_eq!(aws_uri_encode("é", true), "%C3%A9");
    }

    #[test]
    fn preserves_s3_path_identity() {
        assert_eq!(canonical_uri(""), "/");
        assert_eq!(canonical_uri("a//./../b"), "/a//./../b");
        assert_eq!(
            canonical_uri("/photos/Jan sample.jpg"),
            "/photos/Jan%20sample.jpg"
        );
    }

    #[test]
    fn encoded_wire_paths_are_not_double_encoded() {
        assert_eq!(
            canonical_uri_from_encoded("/photos/a%20b%2fc+%25").unwrap(),
            "/photos/a%20b%2Fc%2B%25"
        );
        assert_eq!(canonical_uri("/photos/a%20b"), "/photos/a%2520b");
        assert!(matches!(
            canonical_uri_from_encoded("/broken%2"),
            Err(SigningError::InvalidEncodedUri)
        ));
    }

    #[test]
    fn query_is_sorted_after_encoding_and_retains_duplicates() {
        let query = canonical_query(&[
            QueryParam::new("z", "last"),
            QueryParam::new("a", "2"),
            QueryParam::new("a", "1"),
            QueryParam::new(" ", "+"),
            QueryParam::new("a", "1"),
        ]);
        assert_eq!(query, "%20=%2B&a=1&a=1&a=2&z=last");
    }

    #[test]
    fn headers_are_combined_normalized_and_sorted() {
        let result = canonical_headers(&[
            Header::new("X-Amz-Meta-Z", "  one\t two "),
            Header::new("Host", "examplebucket.s3.amazonaws.com"),
            Header::new("x-amz-meta-z", "three"),
        ])
        .unwrap();
        assert_eq!(
            result.canonical(),
            "host:examplebucket.s3.amazonaws.com\nx-amz-meta-z:one two,three\n"
        );
        assert_eq!(result.signed(), "host;x-amz-meta-z");
    }

    #[test]
    fn host_must_be_present_unambiguous_and_nonempty() {
        assert!(matches!(
            canonical_headers(&[Header::new("x-test", "one")]),
            Err(SigningError::MissingHostHeader)
        ));
        assert!(matches!(
            canonical_headers(&[Header::new("host", "  ")]),
            Err(SigningError::InvalidHostHeader)
        ));
        assert!(matches!(
            canonical_headers(&[
                Header::new("host", "one.example"),
                Header::new("Host", "two.example"),
            ]),
            Err(SigningError::InvalidHostHeader)
        ));
    }

    proptest! {
        #[test]
        fn encoding_round_trips_utf8(value in any::<String>()) {
            let encoded = aws_uri_encode(&value, true);
            prop_assert_eq!(percent_decode(&encoded), value.as_bytes());
            prop_assert!(!encoded.contains('+'));
            prop_assert!(!encoded.contains('/'));
        }

        #[test]
        fn canonical_query_is_lexically_ordered(
            parameters in prop::collection::vec(("[^&=]{0,12}", "[^&=]{0,12}"), 0..30)
        ) {
            let borrowed = parameters
                .iter()
                .map(|(name, value)| QueryParam::new(name, value))
                .collect::<Vec<_>>();
            let canonical = canonical_query(&borrowed);
            let components = if canonical.is_empty() && parameters.is_empty() {
                Vec::new()
            } else {
                canonical
                    .split('&')
                    .map(|component| component.split_once('=').unwrap())
                    .collect::<Vec<_>>()
            };
            prop_assert!(components.windows(2).all(|pair| pair[0] <= pair[1]));
        }

        #[test]
        fn canonical_query_is_permutation_invariant(
            parameters in prop::collection::vec((".{0,16}", ".{0,16}"), 0..40)
        ) {
            let forward = parameters
                .iter()
                .map(|(name, value)| QueryParam::new(name, value))
                .collect::<Vec<_>>();
            let reverse = parameters
                .iter()
                .rev()
                .map(|(name, value)| QueryParam::new(name, value))
                .collect::<Vec<_>>();
            prop_assert_eq!(canonical_query(&forward), canonical_query(&reverse));
        }

        #[test]
        fn header_names_are_case_insensitive(
            suffix in "[A-Za-z0-9-]{1,24}",
            value in "[A-Za-z0-9]{1,40}",
        ) {
            let lower = format!("x-test-{}", suffix.to_ascii_lowercase());
            let upper = lower.to_ascii_uppercase();
            let first = canonical_headers(&[
                Header::new("host", "example.test"),
                Header::new(&lower, &value),
            ]).unwrap();
            let second = canonical_headers(&[
                Header::new("HOST", "example.test"),
                Header::new(&upper, &value),
            ]).unwrap();
            prop_assert_eq!(first.canonical(), second.canonical());
            prop_assert_eq!(first.signed(), second.signed());
        }

        #[test]
        fn header_whitespace_is_canonical(
            words in prop::collection::vec("[A-Za-z0-9]{1,8}", 1..10),
            separators in prop::collection::vec("[ \\t]{1,5}", 0..10),
        ) {
            let mut value = String::from("  ");
            for (index, word) in words.iter().enumerate() {
                if index != 0 {
                    value.push_str(separators.get(index - 1).map_or(" ", String::as_str));
                }
                value.push_str(word);
            }
            value.push_str("\t ");
            let result = canonical_headers(&[
                Header::new("host", "example.com"),
                Header::new("x-test", &value),
            ]).unwrap();
            prop_assert_eq!(
                result.canonical(),
                format!("host:example.com\nx-test:{}\n", words.join(" "))
            );
        }
    }

    fn percent_decode(value: &str) -> Vec<u8> {
        let mut decoded = Vec::with_capacity(value.len());
        let bytes = value.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' {
                decoded.push((hex_nibble(bytes[index + 1]) << 4) | hex_nibble(bytes[index + 2]));
                index += 3;
            } else {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
        decoded
    }

    const fn hex_nibble(value: u8) -> u8 {
        match value {
            b'0'..=b'9' => value - b'0',
            b'A'..=b'F' => value - b'A' + 10,
            _ => unreachable!(),
        }
    }
}
