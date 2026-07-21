#![no_main]

use libfuzzer_sys::fuzz_target;
use s3_wire::{AddressingStyle, Endpoint};

fuzz_target!(|input: (&str, &str, &str, bool)| {
    let (endpoint_value, bucket, key, virtual_hosted) = input;
    if let Ok(endpoint) = Endpoint::new(endpoint_value) {
        let style = if virtual_hosted {
            AddressingStyle::VirtualHosted
        } else {
            AddressingStyle::Path
        };
        let _ = s3_wire::fuzzing::endpoint_object_path_size(&endpoint, bucket, key, style);
    }
});
