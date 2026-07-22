//! Cross-module properties for public validation and redaction boundaries.

use std::sync::Arc;

use proptest::prelude::*;
use s3_wire::{
    Credentials, ObjectKey, PageSize, S3Config, S3Error, StaticCredentialsProvider, UploadId,
};

const MAX_OBJECT_KEY_BYTES: usize = 1_024;

proptest! {
    #[test]
    fn object_keys_round_trip_exact_utf8(value in ".{1,1400}") {
        match ObjectKey::new(value.clone()) {
            Ok(key) => {
                prop_assert!(!value.is_empty());
                prop_assert!(value.len() <= MAX_OBJECT_KEY_BYTES);
                prop_assert_eq!(key.into_string(), value);
            }
            Err(_) => prop_assert!(value.len() > MAX_OBJECT_KEY_BYTES),
        }
    }

    #[test]
    fn page_size_acceptance_matches_s3_bounds(value in any::<u16>()) {
        let page_size = PageSize::new(value);
        prop_assert_eq!(page_size.is_some(), (1..=PageSize::MAX).contains(&value));
        if let Some(page_size) = page_size {
            prop_assert_eq!(page_size.get(), value);
        }
    }

    #[test]
    fn credential_and_upload_identifiers_are_redacted(
        access in "[A-Za-z0-9]{12,40}",
        secret in "[A-Za-z0-9+/=]{12,80}",
        token in "[A-Za-z0-9+/=]{12,80}",
        upload_value in "[A-Za-z0-9+/=._~-]{12,100}",
    ) {
        let credentials = Credentials::new(access.clone(), secret.clone(), Some(token.clone())).unwrap();
        let rendered = format!("{credentials:?}");
        prop_assert!(!rendered.contains(&access));
        prop_assert!(!rendered.contains(&secret));
        prop_assert!(!rendered.contains(&token));

        let upload_id = UploadId::new(upload_value.clone()).unwrap();
        let upload_debug = format!("{:?}", upload_id);
        let upload_display = upload_id.to_string();
        prop_assert!(!upload_debug.contains(&upload_value));
        prop_assert!(!upload_display.contains(&upload_value));
    }

    #[test]
    fn client_debug_does_not_expose_credentials(
        access in "[A-Za-z0-9]{8,40}",
        secret in "[A-Za-z0-9+/=]{12,80}",
    ) {
        let credentials = Credentials::new(access.clone(), secret.clone(), None).unwrap();
        let config = S3Config::builder()
            .bucket("bucket")
            .credentials_provider(Arc::new(StaticCredentialsProvider::new(credentials)))
            .build()
            .unwrap();
        let rendered = format!("{config:?}");
        prop_assert!(!rendered.contains(&access));
        prop_assert!(!rendered.contains(&secret));
    }

    #[test]
    fn error_source_text_is_never_formatted(secret in "[A-Za-z0-9+/=]{12,80}") {
        let source = std::io::Error::other(secret.clone());
        let error = S3Error::transport(source);
        let display = error.to_string();
        let debug = format!("{:?}", error);
        prop_assert!(!display.contains(&secret));
        prop_assert!(!debug.contains(&secret));
    }
}
