//! Small nvim-web-devicons subset, also used by Yazi. MIT attribution is in
//! THIRD_PARTY_NOTICES.md. No font probing, subprocess, or icon runtime is used.
use super::model::kind;
use ratatui::style::Color;

pub fn lookup(name: &str, folder: bool, nerd: bool) -> (&'static str, Color) {
    let (plain, category) = kind(name, folder);
    let (icon, color) = match category {
        "Folder" => ("", 75),
        "Video" => ("", 208),
        "Archive" => ("", 214),
        "Image" => ("", 140),
        "Audio" => ("", 39),
        "Document" => {
            match name
                .rsplit_once('.')
                .map(|(_, ext)| ext.to_ascii_lowercase())
                .as_deref()
            {
                Some("pdf") => ("", 203),
                Some("md") => ("", 253),
                _ => ("󰈙", 113),
            }
        }
        _ => ("", 252),
    };
    (if nerd { icon } else { plain }, Color::Indexed(color))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_aliases_share_an_icon_and_plain_mode_has_no_private_use_glyphs() {
        assert_eq!(
            lookup("a.tar.zst", false, true),
            lookup("a.zip", false, true)
        );
        assert_ne!(
            lookup("Movies", true, true).0,
            lookup("a.mkv", false, true).0
        );
        for (name, folder) in [("a.mkv", false), ("a.tar.zst", false), ("Folder", true)] {
            assert!(
                !lookup(name, folder, false)
                    .0
                    .chars()
                    .any(|c| (0xE000..=0xF8FF).contains(&(c as u32))
                        || (0xF0000..=0xFFFFD).contains(&(c as u32)))
            );
        }
    }
}
