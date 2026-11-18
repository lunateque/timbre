//! The `config` libFuzzer entry point; the checks live in the target function.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    timbre_fuzz::config(data);
});
