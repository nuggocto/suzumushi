// SPDX-License-Identifier: Apache-2.0

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    for bound in [0, 1, 4, 64, 4096] {
        let output = suzumushi::display::terminal_safe(data, bound);
        assert!(output.len() <= bound);
        assert!(!output.chars().any(char::is_control));
    }
});
