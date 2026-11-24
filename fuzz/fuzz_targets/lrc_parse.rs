#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| timbre_fuzz::lrc_parse(data));
