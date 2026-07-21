#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &str| {
    let canonical = s3_wire::fuzzing::canonical_uri_path(input);
    assert!(canonical.starts_with('/'));
});
