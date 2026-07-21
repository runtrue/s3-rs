#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: Vec<(String, String)>| {
    let canonical = s3_wire::fuzzing::canonical_query_pairs(&input);
    assert!(!canonical.contains(' '));
});
