//! Explicit, bounded file previews. No selection-change fetching or persistent cache.
use anyhow::{Context, Result, bail, ensure};
use image::{DynamicImage, ImageDecoder, ImageReader, Limits};
use std::{
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub const IMAGE_LIMIT: i64 = 100 * 1024 * 1024;
pub const TEXT_LIMIT: i64 = 1024 * 1024;
pub const TEXT_READ: i64 = 128 * 1024;
const PIXELS: u64 = 32_000_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Image,
}
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub kind: Kind,
    pub bytes: i64,
    pub truncated: bool,
}
pub enum Content {
    Text { lines: Vec<String>, truncated: bool },
    Image(DynamicImage),
}
pub fn plan(name: &str, size: i64, folder: bool) -> Result<Plan> {
    ensure!(!folder, "Choose a file to preview");
    ensure!(size >= 0, "File size is unavailable");
    let lower = name.to_lowercase();
    let extension = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    let kind = if matches!(extension, "jpg" | "jpeg" | "png" | "webp" | "gif") {
        Kind::Image
    } else if matches!(
        extension,
        "txt"
            | "md"
            | "markdown"
            | "json"
            | "jsonl"
            | "yaml"
            | "yml"
            | "toml"
            | "ini"
            | "cfg"
            | "conf"
            | "csv"
            | "tsv"
            | "log"
            | "xml"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "rs"
            | "py"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "go"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "sql"
            | "lua"
            | "rb"
            | "java"
            | "kt"
            | "swift"
            | "nix"
            | "env"
    ) || matches!(
        lower.as_str(),
        "readme" | "license" | "licence" | "makefile" | "dockerfile" | ".gitignore" | ".env"
    ) {
        Kind::Text
    } else {
        bail!("Preview supports text, JPEG, PNG, WebP, and GIF files");
    };
    let limit = if kind == Kind::Image {
        IMAGE_LIMIT
    } else {
        TEXT_LIMIT
    };
    ensure!(
        size <= limit,
        "{} previews are limited to {}",
        if kind == Kind::Image { "Image" } else { "Text" },
        if kind == Kind::Image {
            "100 MiB"
        } else {
            "1 MiB"
        }
    );
    let bytes = if kind == Kind::Text {
        size.min(TEXT_READ)
    } else {
        size
    };
    Ok(Plan {
        kind,
        bytes,
        truncated: bytes < size,
    })
}
struct Cancellable {
    file: File,
    cancel: Arc<AtomicBool>,
}
impl Read for Cancellable {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("Preview closed"));
        }
        self.file.read(buffer)
    }
}
impl Seek for Cancellable {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}
pub fn decode(mut file: File, plan: Plan, cancel: Arc<AtomicBool>) -> Result<Content> {
    ensure!(
        file.metadata()?.len() == plan.bytes as u64,
        "Preview data has an unexpected length"
    );
    ensure!(!cancel.load(Ordering::Relaxed), "Preview closed");
    file.rewind()?;
    if plan.kind == Kind::Text {
        let mut bytes = Vec::new();
        file.take(TEXT_READ as u64).read_to_end(&mut bytes)?;
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(error) if plan.truncated && error.error_len().is_none() => {
                std::str::from_utf8(&bytes[..error.valid_up_to()])?
            }
            Err(_) => bail!("This file is not UTF-8 text"),
        };
        ensure!(!text.contains('\0'), "This file contains binary data");
        let mut lines = Vec::new();
        let mut truncated = plan.truncated;
        for line in text.trim_start_matches('\u{feff}').lines().take(4001) {
            if lines.len() == 4000 {
                truncated = true;
                break;
            }
            let mut output = String::new();
            for (index, c) in line.trim_end_matches('\r').chars().enumerate() {
                if index == 4096 {
                    output.push('…');
                    truncated = true;
                    break;
                }
                if c == '\t' {
                    output.push_str("    ");
                } else if c.is_control()
                    || matches!(c,'\u{061c}'|'\u{200b}'..='\u{200f}'|'\u{2028}'..='\u{202e}'|'\u{2060}'..='\u{206f}'|'\u{feff}')
                {
                    output.extend(c.escape_default());
                } else {
                    output.push(c);
                }
            }
            lines.push(output);
        }
        return Ok(Content::Text { lines, truncated });
    }
    let mut reader = ImageReader::new(BufReader::new(Cancellable {
        file,
        cancel: cancel.clone(),
    }))
    .with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().context("Cannot decode this image")?;
    let (width, height) = decoder.dimensions();
    ensure!(
        u64::from(width) * u64::from(height) <= PIXELS
            && decoder.total_bytes() <= 128 * 1024 * 1024,
        "Image dimensions are too large to preview safely"
    );
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder).context("Cannot decode this image")?;
    ensure!(!cancel.load(Ordering::Relaxed), "Preview closed");
    image.apply_orientation(orientation);
    Ok(Content::Image(
        if image.width() > 1024 || image.height() > 1024 {
            image.thumbnail(1024, 1024)
        } else {
            image
        },
    ))
}

pub fn redact_account(email: &str) -> String {
    fn part(text: &str) -> String {
        let count = text.chars().count();
        let keep = if count > 5 { 2 } else { 1 };
        let start: String = text.chars().take(keep).collect();
        let end: String = if count > 2 {
            text.chars().skip(count - keep).collect()
        } else {
            String::new()
        };
        format!("{start}…{end}")
    }
    let Some((local, host)) = email.split_once('@') else {
        return "Account".into();
    };
    if local.is_empty()
        || host.is_empty()
        || email.len() > 512
        || email.chars().any(|c| c.is_control() || matches!(c,'\u{061c}'|'\u{200b}'..='\u{200f}'|'\u{2028}'..='\u{202e}'|'\u{2060}'..='\u{206f}'|'\u{feff}'))
    {
        return "Account".into();
    }
    format!("{}@{}", part(local), part(host))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn file(bytes: &[u8]) -> File {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(bytes).unwrap();
        file
    }
    #[test]
    fn limits_and_type_policy_are_checked_before_reading() {
        assert_eq!(
            plan("image.png", IMAGE_LIMIT, false).unwrap().bytes,
            IMAGE_LIMIT
        );
        assert!(plan("image.jpg", IMAGE_LIMIT + 1, false).is_err());
        assert!(plan("notes.txt", TEXT_LIMIT + 1, false).is_err());
        let text = plan("script.rs", TEXT_LIMIT, false).unwrap();
        assert_eq!(text.bytes, TEXT_READ);
        assert!(text.truncated);
        for name in [
            "movie.mkv",
            "archive.zip",
            "page.pdf",
            "vector.svg",
            "program.exe",
        ] {
            assert!(plan(name, 10, false).is_err());
        }
        assert!(plan("notes.txt", -1, false).is_err());
        assert!(plan("folder", 0, true).is_err());
    }
    #[test]
    fn text_is_sanitized_bounded_and_tolerates_a_cut_utf8_suffix() {
        let text = "hello\x1b]52;secret\x07\ncolumn\tvalue\nrtl\u{202e}";
        let Content::Text { lines, .. } = decode(
            file(text.as_bytes()),
            plan("a.txt", text.len() as i64, false).unwrap(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap() else {
            panic!()
        };
        assert!(!lines.join("\n").contains('\x1b'));
        assert!(!lines.join("\n").contains('\u{202e}'));
        assert!(lines[1].contains("    "));
        let mut bytes = vec![b'x'; TEXT_READ as usize];
        bytes[TEXT_READ as usize - 1] = 0xc3;
        let Content::Text { truncated, .. } = decode(
            file(&bytes),
            plan("a.txt", TEXT_READ + 3, false).unwrap(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap() else {
            panic!()
        };
        assert!(truncated);
        assert!(
            decode(
                file(b"a\0b"),
                plan("a.txt", 3, false).unwrap(),
                Arc::new(AtomicBool::new(false))
            )
            .is_err()
        );
        let long = "x\n".repeat(5000);
        let Content::Text { lines, truncated } = decode(
            file(long.as_bytes()),
            plan("a.txt", long.len() as i64, false).unwrap(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(lines.len(), 4000);
        assert!(truncated);
    }
    #[test]
    fn image_decoding_checks_dimensions_and_cancellation_before_allocating_pixels() {
        use image::{ImageBuffer, ImageFormat, Rgb};
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(8, 4, Rgb([40, 90, 180])));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        let bytes = bytes.into_inner();
        let Content::Image(decoded) = decode(
            file(&bytes),
            plan("a.png", bytes.len() as i64, false).unwrap(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!((decoded.width(), decoded.height()), (8, 4));
        assert!(
            decode(
                file(&bytes),
                plan("a.png", bytes.len() as i64, false).unwrap(),
                Arc::new(AtomicBool::new(true))
            )
            .is_err()
        );
        assert!(
            decode(
                file(b"not an image"),
                plan("a.png", 12, false).unwrap(),
                Arc::new(AtomicBool::new(false))
            )
            .is_err()
        );
        // Valid PNG headers claim excessive dimensions/pixel counts; reject before pixel allocation.
        for (width, height) in [(100_000u32, 100_000u32), (10_000, 4_000)] {
            let mut bomb = bytes.clone();
            bomb[16..20].copy_from_slice(&width.to_be_bytes());
            bomb[20..24].copy_from_slice(&height.to_be_bytes());
            let mut crc = !0u32;
            for byte in &bomb[12..29] {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = (crc >> 1) ^ 0xedb88320u32.wrapping_mul(crc & 1);
                }
            }
            bomb[29..33].copy_from_slice(&(!crc).to_be_bytes());
            let error = decode(
                file(&bomb),
                plan("a.png", bomb.len() as i64, false).unwrap(),
                Arc::new(AtomicBool::new(false)),
            )
            .err()
            .expect("Huge image must fail");
            if width == 10_000 {
                assert!(error.to_string().contains("dimensions"));
            }
        }
    }
    #[test]
    fn both_account_components_are_redacted_without_invalid_utf8_or_controls() {
        assert_eq!(redact_account("robin42@example.com"), "ro…42@ex…om");
        assert_eq!(redact_account("a@xy"), "a…@x…");
        let masked = redact_account("写真名@example.org");
        assert!(masked.contains('…'));
        assert!(!masked.contains("example.org"));
        for invalid in [
            "",
            "missing-at",
            "foo@",
            "@example.org",
            "bad\x1b@example.org",
            "bad\u{202e}@example.org",
        ] {
            assert_eq!(redact_account(invalid), "Account");
        }
    }
}
