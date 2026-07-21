#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|body: &[u8]| {
    let _ = s3_wire::fuzzing::parse_listing_xml(body, 256 * 1024);
});
