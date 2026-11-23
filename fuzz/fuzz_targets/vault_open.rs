//! A damaged or hostile saved-login file must fail cleanly, never panic.
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../../src/vault.rs"]
#[allow(dead_code)]
mod vault;

fuzz_target!(|input: (&[u8], &[u8])| {
    let (password, data) = input;
    let _ = vault::open(password, data);
});
