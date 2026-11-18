//! Offline visual fixtures, rendered by the actual widgets with synthetic data.
mod demo;
use super::*;
use ratatui::{backend::TestBackend, style::Color};
use std::{fmt::Write, path::Path};

fn color(value: Color, default: &str) -> String {
    match value {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(n) if n >= 232 => {
            let v = 8 + (n - 232) as u16 * 10;
            format!("#{v:02x}{v:02x}{v:02x}")
        }
        Color::Indexed(n) if n >= 16 => {
            let n = n - 16;
            let component = |v| if v == 0 { 0 } else { 55 + v as u16 * 40 };
            format!(
                "#{:02x}{:02x}{:02x}",
                component(n / 36),
                component(n / 6 % 6),
                component(n % 6)
            )
        }
        Color::White => "#ffffff".into(),
        _ => default.into(),
    }
}
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn export(path: &Path, ui: &Ui, state: &Snapshot, width: u16, height: u16) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| view::draw(f, ui, state)).unwrap();
    let buffer = terminal.backend().buffer();
    write_svg(path, buffer, width, height);
}
fn write_svg(path: &Path, buffer: &ratatui::buffer::Buffer, width: u16, height: u16) {
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\"><rect width=\"100%\" height=\"100%\" fill=\"#272b33\"/><g font-family=\"Menlo, monospace\" font-size=\"14\">",
        width as usize * 9,
        height as usize * 21
    );
    for y in 0..height {
        for x in 0..width {
            let cell = &buffer[(x, y)];
            if cell.bg != Color::Reset {
                write!(
                    svg,
                    "<rect x=\"{}\" y=\"{}\" width=\"9\" height=\"21\" fill=\"{}\"/>",
                    x as usize * 9,
                    y as usize * 21,
                    color(cell.bg, "#272b33")
                )
                .unwrap();
            }
            if !cell.symbol().trim().is_empty() {
                write!(
                    svg,
                    "<text x=\"{}\" y=\"{}\" fill=\"{}\" font-weight=\"{}\">{}</text>",
                    x as usize * 9,
                    y as usize * 21 + 16,
                    color(cell.fg, "#e6edf3"),
                    if cell.modifier.contains(ratatui::style::Modifier::BOLD) {
                        "bold"
                    } else {
                        "normal"
                    },
                    escape(cell.symbol())
                )
                .unwrap();
            }
        }
    }
    svg.push_str("</g></svg>");
    std::fs::write(path, svg).unwrap();
}

#[test]
#[ignore = "writes offline visual fixtures to target/tui-preview"]
fn export_visual_fixtures() {
    let root = std::env::current_dir().unwrap().join("target/tui-preview");
    let local = root.join("local");
    for folder in ["Documents", "Pictures", "Projects"] {
        std::fs::create_dir_all(local.join(folder)).unwrap();
    }
    let mut ui = Ui::new(1).unwrap();
    ui.loading = false;
    ui.download_dir = local.clone();
    ui.upload_dir = local;
    ui.page = Arc::new(
        [
            ("Documents", true, 0),
            ("Movies", true, 0),
            ("Pictures", true, 0),
            ("backup.tar.zst", false, 8_375_186_227),
            ("documentary.mkv", false, 1_283_457_023),
            ("cover.jpg", false, 4_194_304),
            ("notes.md", false, 4096),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (name, folder, size))| Entry {
            id: n as u64 + 2,
            name: name.into(),
            folder,
            size,
            modified: 1_794_924_000,
            shared: false,
            exported: false,
            parent: 0,
            fingerprint: String::new(),
        })
        .collect(),
    );
    let state = Snapshot {
        account: Some(crate::preview::redact_account("robin42@example.com")),
        connection: Connection::Ready,
        storage: Some(Storage {
            used: 169 * 1024_i64.pow(4) / 10,
            total: 20 * 1024_i64.pow(4),
        }),
        ..Snapshot::default()
    };
    export(&root.join("browser.svg"), &ui, &state, 120, 30);
    export(&root.join("compact.svg"), &ui, &state, 70, 24);
    ui.modal = Some(Modal::Help);
    export(&root.join("help.svg"), &ui, &state, 100, 28);
    ui.modal = None;
    ui.marked = ui
        .page
        .iter()
        .take(2)
        .cloned()
        .map(|entry| Selection { entry, parent: 1 })
        .collect();
    export(&root.join("selected.svg"), &ui, &state, 120, 30);
    ui.marked.clear();
    ui.cursor = 3;
    ui.open_download();
    export(&root.join("download.svg"), &ui, &state, 120, 30);
    ui.open_upload();
    export(&root.join("upload.svg"), &ui, &state, 120, 30);
    ui.modal = Some(Modal::Action(actions::Dialog::Menu {
        cursor: 0,
        request: None,
        targets: ui.targets(),
    }));
    export(&root.join("actions.svg"), &ui, &state, 120, 30);
    ui.cursor = 0;
    let (send, _recv) = sync_channel(16);
    ui.review_trash(&send);
    let Some(Modal::Action(actions::Dialog::TrashReview { request, .. })) = &ui.modal else {
        panic!()
    };
    let mut management = state.clone();
    management.details = Some(Inspection {
        request: *request,
        details: Some(Arc::new(Details {
            members: Vec::new(),
            selection: Selection {
                entry: ui.page[0].clone(),
                parent: 1,
            },
            path: "/Documents".into(),
            created: 1_794_924_000,
            access: 3,
            in_rubbish: false,
            restore: None,
            generation: 1,
            restricted: false,
            favourite: false,
            direct_link: crate::bridge::ffi::LinkState::None,
            stats: Some(FolderStats {
                files: 12,
                folders: 3,
                bytes: 4_194_304,
                versions: 2,
                version_bytes: 8192,
            }),
            review_ready: true,
            checked: 15,
        })),
        error: None,
    });
    let review = ui.modal.take();
    ui.modal = Some(Modal::Action(actions::Dialog::Menu {
        cursor: 0,
        request: management.details.as_ref().map(|s| s.request),
        targets: ui.targets(),
    }));
    export(&root.join("item-panel.svg"), &ui, &management, 100, 28);
    ui.modal = review;
    export(&root.join("rubbish-review.svg"), &ui, &management, 100, 28);
    export(&root.join("rubbish-confirm.svg"), &ui, &management, 80, 24);
    ui.destination(true, &send);
    let Some(Modal::Action(actions::Dialog::Destination { request, .. })) = &ui.modal else {
        panic!()
    };
    management.picker_page = Some(Page {
        more: false,
        request: *request,
        parent: 1,
        offset: 0,
        entries: Arc::new(ui.page.iter().filter(|e| e.folder).cloned().collect()),
        error: None,
    });
    export(&root.join("copy.svg"), &ui, &management, 100, 28);
    ui.modal = None;
    management.rubbish = Some(9);
    ui.switch_location(true, &management, &send);
    ui.loading = false;
    export(&root.join("bin-empty.svg"), &ui, &management, 70, 24);
    export(&root.join("bin-narrow.svg"), &ui, &management, 38, 14);
    ui.page = Arc::new(vec![
        management
            .details
            .as_ref()
            .unwrap()
            .details
            .as_ref()
            .unwrap()
            .selection
            .entry
            .clone(),
    ]);
    export(&root.join("bin.svg"), &ui, &management, 110, 28);
    ui.modal = None;
    let text_entry = Entry {
        id: 70,
        name: "example.rs".into(),
        size: 42,
        folder: false,
        modified: 0,
        shared: false,
        exported: false,
        parent: 0,
        fingerprint: String::new(),
    };
    ui.preview_entry(text_entry, &send);
    let Some(Modal::Preview { request, .. }) = &ui.modal else {
        panic!()
    };
    management.preview = Some(Arc::new(PreviewState {
        request: *request,
        name: "example.rs".into(),
        bytes: 42,
        total: 42,
        decoding: false,
        error: None,
        content: Some(Arc::new(crate::preview::Content::Text {
            lines: vec![
                "fn main() {".into(),
                "    println!(\"Hello, world!\");".into(),
                "}".into(),
            ],
            truncated: false,
        })),
    }));
    export(&root.join("text-preview.svg"), &ui, &management, 100, 28);
    let pixels = image::RgbImage::from_fn(320, 160, |x, y| {
        image::Rgb([(x * 255 / 320) as u8, (y * 255 / 160) as u8, 170])
    });
    ui.preview_entry(
        Entry {
            id: 71,
            name: "image.png".into(),
            size: 1000,
            folder: false,
            modified: 0,
            shared: false,
            exported: false,
            parent: 0,
            fingerprint: String::new(),
        },
        &send,
    );
    let Some(Modal::Preview { request, .. }) = &ui.modal else {
        panic!()
    };
    management.preview = Some(Arc::new(PreviewState {
        request: *request,
        name: "image.png".into(),
        bytes: 1000,
        total: 1000,
        decoding: false,
        error: None,
        content: Some(Arc::new(crate::preview::Content::Image(
            image::DynamicImage::ImageRgb8(pixels),
        ))),
    }));
    export(&root.join("image-preview.svg"), &ui, &management, 100, 28);
    let mut app = lifecycle::AppState {
        initial_preview: None,
        phase: lifecycle::Phase::Input,
        closing: false,
        notes: std::collections::VecDeque::new(),
        revision: 0,
        state_path: "/home/alex/.megawren".into(),
    };
    let prompt = lifecycle::preview_prompt(
        "Unlock saved login: ",
        true,
        "synthetic preview value",
        lifecycle::InputMode::Text,
    );
    let mut terminal = Terminal::new(TestBackend::new(110, 28)).unwrap();
    terminal
        .draw(|f| view::draw_lifecycle(f, &app, Some(&prompt), Duration::from_secs(1)))
        .unwrap();
    write_svg(
        &root.join("unlock.svg"),
        terminal.backend().buffer(),
        110,
        28,
    );
    app.closing = true;
    app.phase = lifecycle::Phase::Busy {
        message: "Saving local index… 63%".into(),
        percent: Some(63),
    };
    terminal
        .draw(|f| view::draw_lifecycle(f, &app, None, Duration::from_secs(2)))
        .unwrap();
    write_svg(
        &root.join("saving.svg"),
        terminal.backend().buffer(),
        110,
        28,
    );
}
