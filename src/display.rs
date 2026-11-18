use crate::{bridge::ffi::Entry, state};
use std::io::{self, Write};

pub fn size(bytes: i64) -> String {
    if bytes < 0 {
        return "—".into();
    }
    let mut n = bytes as f64;
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut unit = 0;
    while n >= 1024.0 && unit < units.len() - 1 {
        n /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{n:.1} {}", units[unit])
    }
}

pub fn listing(output: &mut impl Write, entries: &[Entry], terminal: bool) -> io::Result<()> {
    for entry in entries {
        let name = state::display(&entry.name);
        if terminal {
            let (bytes, suffix) = if entry.folder {
                ("—".into(), "/")
            } else {
                (size(entry.size), "")
            };
            writeln!(output, "{bytes:>9}  {name}{suffix}")?;
        } else {
            let kind = if entry.folder { "dir" } else { "file" };
            writeln!(output, "{kind}\t{}\t{name}", entry.size)?;
        }
    }
    Ok(())
}
// serde_json escapes ASCII controls. Also escape terminal and bidi controls
// from Unicode without changing the filename a JSON reader receives.
struct ListingJson;
impl serde_json::ser::Formatter for ListingJson {
    fn write_string_fragment<W: ?Sized + Write>(
        &mut self,
        out: &mut W,
        text: &str,
    ) -> io::Result<()> {
        let mut start = 0;
        for (i, c) in text
            .char_indices()
            .filter(|(_, c)| state::unsafe_character(*c))
        {
            out.write_all(&text.as_bytes()[start..i])?;
            for unit in c.encode_utf16(&mut [0; 2]) {
                write!(out, "\\u{unit:04x}")?;
            }
            start = i + c.len_utf8();
        }
        out.write_all(&text.as_bytes()[start..])
    }
}

/// Keep one SDK page in memory, including when a pipeline requests JSON.
pub fn directory(
    out: &mut impl Write,
    terminal: bool,
    json: bool,
    mut page: impl FnMut(usize) -> anyhow::Result<Vec<Entry>>,
) -> anyhow::Result<()> {
    #[derive(serde::Serialize)]
    struct Row<'a> {
        name: &'a str,
        #[serde(rename = "type")]
        kind: &'static str,
        size: Option<i64>,
    }
    let mut offset = 0;
    loop {
        let rows = page(offset)?;
        if json {
            if offset == 0 {
                out.write_all(b"[")?;
            }
            for (i, entry) in rows.iter().enumerate() {
                if offset + i != 0 {
                    out.write_all(b",")?;
                }
                let row = Row {
                    name: &entry.name,
                    kind: if entry.folder { "dir" } else { "file" },
                    size: (!entry.folder && entry.size >= 0).then_some(entry.size),
                };
                let mut writer = serde_json::Serializer::with_formatter(&mut *out, ListingJson);
                serde::Serialize::serialize(&row, &mut writer).map_err(io::Error::from)?;
            }
        } else {
            listing(out, &rows, terminal)?;
        }
        offset += rows.len();
        if rows.len() < 256 {
            break;
        }
    }
    if json {
        out.write_all(b"]\n")?;
    }
    out.flush()?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_listing_round_trips_names_and_sizes_across_page_boundaries() {
        let name = "tab\tline\nquote\"slash\\雪\u{202e}\u{009b}";
        for count in [0, 1, 256, 257] {
            let mut output = vec![];
            directory(&mut output, true, true, |offset| {
                Ok((offset..count.min(offset + 256))
                    .map(|i| Entry {
                        id: i as u64,
                        name: name.into(),
                        size: if i == 0 { -1 } else { i64::MAX },
                        folder: i == 0,
                        shared: false,
                        exported: false,
                        parent: 0,
                        fingerprint: String::new(),
                        modified: 0,
                    })
                    .collect())
            })
            .unwrap();
            assert!(
                !std::str::from_utf8(&output)
                    .unwrap()
                    .contains(['\u{202e}', '\u{009b}'])
            );
            let rows: Vec<serde_json::Value> = serde_json::from_slice(&output).unwrap();
            assert_eq!(rows.len(), count);
            for (i, row) in rows.iter().enumerate() {
                assert_eq!(row["name"], name);
                assert_eq!(row["type"], if i == 0 { "dir" } else { "file" });
                assert_eq!(
                    row["size"].as_i64(),
                    if i == 0 { None } else { Some(i64::MAX) }
                );
            }
        }
    }
    #[test]
    fn listing_aligns_names_marks_directories_and_preserves_piped_fields() {
        let entry = |name: &str, folder, size| Entry {
            id: 0,
            name: name.into(),
            folder,
            size,
            modified: 0,
            shared: false,
            exported: false,
            parent: 0,
            fingerprint: String::new(),
        };
        let rows = [
            entry("My folder", true, -1),
            entry("Movie.mkv", false, 1_318_538_052),
            entry("line\nbreak", false, 0),
        ];
        let mut out = vec![];
        listing(&mut out, &rows, true).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "        —  My folder/\n  1.2 GiB  Movie.mkv\n      0 B  line\\nbreak\n"
        );
        let mut out = vec![];
        listing(&mut out, &rows, false).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "dir\t-1\tMy folder\nfile\t1318538052\tMovie.mkv\nfile\t0\tline\\nbreak\n"
        );
    }
}
