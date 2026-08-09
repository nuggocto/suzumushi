// SPDX-License-Identifier: Apache-2.0

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    suzumushi::metadata::fuzz_parse(data);
    suzumushi::metadata::fuzz_reply(data);
});
