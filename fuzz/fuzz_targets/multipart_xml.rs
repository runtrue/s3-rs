#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|body: &[u8]| {
    let _ = s3_wire::fuzzing::parse_multipart_xml(body, 256 * 1024);
});
