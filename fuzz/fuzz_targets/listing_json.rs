//! `ls --json` escapes terminal and direction controls, yet a JSON reader must
//! still receive exactly the original file name.
#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../../src/text.rs"]
#[allow(dead_code)]
mod text;

fuzz_target!(|name: &str| {
    let mut json = Vec::new();
    let mut writer = serde_json::Serializer::with_formatter(&mut json, text::ListingJson);
    serde::Serialize::serialize(&name, &mut writer).unwrap();
    let json = String::from_utf8(json).expect("JSON output is UTF-8");
    assert!(
        !json.chars().any(text::unsafe_character),
        "unsafe character in JSON: {json:?}"
    );
    let decoded: String = serde_json::from_str(&json).expect("JSON output parses");
    assert_eq!(decoded, name, "JSON must round-trip the file name");
});
