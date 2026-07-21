#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|body: &[u8]| {
    let _ = s3_wire::fuzzing::parse_error_xml(body, 64 * 1024);
});
