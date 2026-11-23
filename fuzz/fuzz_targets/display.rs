//! A file name or message shown in the terminal must never carry terminal
//! controls or invisible direction changes, whatever the input.
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../../src/text.rs"]
#[allow(dead_code)]
mod text;

fuzz_target!(|input: &str| {
    let shown = text::display(input);
    assert!(
        !shown.chars().any(text::unsafe_character),
        "unsafe character survived: {shown:?}"
    );
    if !input.chars().any(text::unsafe_character) {
        assert_eq!(shown, input, "safe text must pass through unchanged");
    }
});
