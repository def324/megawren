//! Turns untrusted text, such as file names from a MEGA account, into text
//! that is safe to show in a terminal or emit as JSON. It has no dependency
//! on the rest of the crate, so the fuzz targets in fuzz/ compile it directly.
use std::io::{self, Write};

/// Escapes terminal controls and invisible or direction-changing characters.
pub fn display(s: &str) -> String {
    let mut output = String::with_capacity(s.len());
    for c in s.chars() {
        if unsafe_character(c) {
            output.extend(c.escape_default());
        } else {
            output.push(c);
        }
    }
    output
}
pub fn unsafe_character(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
}

// serde_json escapes ASCII controls. Also escape terminal and bidi controls
// from Unicode without changing the filename a JSON reader receives.
pub struct ListingJson;
impl serde_json::ser::Formatter for ListingJson {
    fn write_string_fragment<W: ?Sized + Write>(
        &mut self,
        out: &mut W,
        text: &str,
    ) -> io::Result<()> {
        let mut start = 0;
        for (i, c) in text.char_indices().filter(|(_, c)| unsafe_character(*c)) {
            out.write_all(&text.as_bytes()[start..i])?;
            for unit in c.encode_utf16(&mut [0; 2]) {
                write!(out, "\\u{unit:04x}")?;
            }
            start = i + c.len_utf8();
        }
        out.write_all(&text.as_bytes()[start..])
    }
}
