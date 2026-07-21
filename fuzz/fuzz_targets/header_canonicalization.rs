#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: Vec<(String, String)>| {
    let _ = s3_wire::fuzzing::canonical_header_pairs(&input);
});
