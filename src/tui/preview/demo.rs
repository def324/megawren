//! Scripted README tour. Only the account, drive contents and SDK events are simulated.
use crate::tui::*;
use ratatui::{
    Frame,
    backend::{Backend as _, TestBackend},
    buffer::Buffer,
    style::Color,
};
use serde_json::json;
use std::{
    fs,
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

const COLS: u16 = 112;
const ROWS: u16 = 30;
const EMAIL: &str = "robin@example.com";
const PASSWORD: &str = "demo-password";
const OTP: &str = "314159";
const MIB: i64 = 1024 * 1024;

// Encode only Ratatui's output. No PTY, environment metadata, or input events enter the cast.
struct Cast {
    file: fs::File,
    screen: Terminal<TestBackend>,
    previous: Buffer,
    caption: Option<&'static str>,
    time_ms: u32,
    home: String,
}
impl Cast {
    fn new(path: &Path) -> Result<Self> {
        crossterm::style::force_color_output(true);
        let screen = Terminal::new(TestBackend::new(COLS, ROWS))?;
        let mut file = fs::File::create(path)?;
        writeln!(
            file,
            "{}",
            json!({"version":2, "width":COLS, "height":ROWS})
        )?;
        let previous = screen.backend().buffer().clone();
        Ok(Self {
            file,
            screen,
            previous,
            caption: None,
            time_ms: 0,
            home: std::env::var("HOME")?,
        })
    }
    fn frame(
        &mut self,
        hold_ms: u32,
        caption: &'static str,
        draw: impl FnOnce(&mut Frame),
    ) -> Result<()> {
        self.screen.draw(draw)?;
        let mut buffer = self.screen.backend().buffer().clone();
        // A full block covers the whole terminal cell. Use its background fill in the cast
        // so the font's block glyph and the transfer label share the same top and bottom.
        for cell in &mut buffer.content {
            if cell.symbol() == "█" && cell.fg != Color::Reset && cell.modifier.is_empty() {
                cell.set_bg(cell.fg).set_symbol(" ");
            }
        }
        // Check full rendered cells before ANSI diffing can split paths or hidden fields.
        let visible: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        anyhow::ensure!(
            [
                self.home.as_str(),
                env!("CARGO_MANIFEST_DIR"),
                PASSWORD,
                OTP
            ]
            .iter()
            .all(|value| !visible.contains(value)),
            "A host path or an unmasked login field reached the demo"
        );
        let mut bytes = Vec::new();
        let mut output = CrosstermBackend::new(&mut bytes);
        output.draw(self.previous.diff_iter(&buffer))?;
        // A solid cursor beside a masked field looks like another character in a GIF.
        output.hide_cursor()?;
        self.previous = buffer;
        let seconds = f64::from(self.time_ms) / 1000.;
        if self.caption != Some(caption) {
            writeln!(self.file, "{}", json!([seconds, "m", caption]))?;
            self.caption = Some(caption);
        }
        writeln!(
            self.file,
            "{}",
            json!([seconds, "o", String::from_utf8(bytes)?])
        )?;
        self.time_ms += hold_ms;
        Ok(())
    }
}

fn sign_in(cast: &mut Cast) -> Result<()> {
    use lifecycle::{AppState, InputMode, Phase, preview_prompt};
    let app = AppState {
        initial_preview: None,
        phase: Phase::Input,
        closing: false,
        notes: Default::default(),
        revision: 0,
        state_path: "/demo/.megawren".into(),
    };
    for (label, value, hidden, stride, pause_ms) in [
        ("Email: ", EMAIL, false, 4, 650),
        ("Password: ", PASSWORD, true, 4, 650),
        ("Authenticator code: ", OTP, true, 1, 1600),
    ] {
        for end in (0..value.len()).step_by(stride).chain([value.len()]) {
            let prompt = preview_prompt(label, hidden, &value[..end], InputMode::Text);
            let hold = if end == value.len() {
                pause_ms
            } else if end == 0 {
                250
            } else {
                100
            };
            cast.frame(
                hold,
                "This account uses a six-digit code from an authenticator app.",
                |f| view::draw_lifecycle(f, &app, Some(&prompt), Duration::ZERO),
            )?;
        }
    }
    let masked_digits = cast
        .screen
        .backend()
        .buffer()
        .content
        .iter()
        .filter(|cell| cell.symbol() == "•")
        .count();
    assert_eq!(
        masked_digits, 6,
        "Completed authenticator field in the tour"
    );
    Ok(())
}

fn entry(id: u64, name: &str, size: i64, days_ago: i64) -> Entry {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    Entry {
        id,
        name: name.into(),
        folder: size < 0,
        size,
        modified: now - days_ago * 86400 - 3600,
        shared: false,
        exported: false,
        parent: 0,
        fingerprint: String::new(),
    }
}

fn transfer(id: u64, direction: Direction, file: &Entry, destination: &str) -> JobView {
    JobView {
        id,
        direction,
        items: vec![TransferItem::from(file)].into(),
        current_item: Some(0),
        name: file.name.clone(),
        file: file.name.clone(),
        total: file.size,
        bytes: 0,
        speed: 0.,
        state: JobState::Queued,
        files: 0,
        folders: 0,
        error: None,
        destination: destination.into(),
    }
}

fn advance(job: &mut JobView, elapsed_ms: u32, duration_ms: u32) {
    job.bytes = job.total * i64::from(elapsed_ms) / i64::from(duration_ms);
    if elapsed_ms == duration_ms {
        job.state = JobState::Complete;
        job.files = 1;
        job.speed = 0.;
    } else {
        job.state = JobState::Transferring;
        job.speed = job.total as f64 * 1000. / f64::from(duration_ms);
    }
}

fn picker(ui: &mut Ui) -> Option<&mut Picker> {
    match ui.modal.as_mut()? {
        Modal::Upload { picker, .. } | Modal::Download { picker, .. } => Some(picker),
        _ => None,
    }
}

fn browser(
    cast: &mut Cast,
    ui: &mut Ui,
    state: &Snapshot,
    local: &Path,
    ms: u32,
    caption: &'static str,
) -> Result<()> {
    // A real picker reads only the disposable fixture. Show its logical path during drawing,
    // then restore the real path for the next keypress and its filesystem checks.
    let real = if let Some(p) = picker(ui) {
        let logical = Path::new("/demo").join(p.directory.strip_prefix(local)?);
        Some(std::mem::replace(&mut p.directory, logical))
    } else {
        None
    };
    let result = cast.frame(ms, caption, |f| view::draw(f, ui, state));
    if let Some(real) = real {
        picker(ui).unwrap().directory = real;
    }
    result
}

#[test]
#[ignore = "renders the synthetic README tour; run scripts/demo.py"]
fn record() -> Result<()> {
    let mut cast = Cast::new(&PathBuf::from(std::env::var("MEGAWREN_DEMO_CAST")?))?;
    let scratch = tempfile::tempdir()?;
    let local = scratch.path().canonicalize()?;
    let mut ui = Ui::new(1)?;
    ui.upload_dir = local.join("Documents");
    ui.download_dir = local.join("Downloads");
    fs::create_dir(&ui.upload_dir)?;
    fs::create_dir(&ui.download_dir)?;
    let clip = entry(10, "Coast walk.mp4", 160 * MIB, 0);
    fs::File::create(ui.upload_dir.join(&clip.name))?.set_len(clip.size as u64)?;
    fs::write(ui.upload_dir.join("Packing list.md"), "# Day pack\nWater\n")?;
    fs::write(ui.upload_dir.join("Waypoints.csv"), "name,km\nHarbor,0\n")?;
    ui.loading = false;
    ui.page = Arc::new(vec![
        entry(2, "Photos", -1, 4),
        entry(3, "Dawn chorus.flac", 28 * MIB, 3),
        entry(4, "Harbor light.jpg", 6 * MIB, 1),
        entry(5, "Route maps.tar.zst", 48 * MIB, 7),
        entry(6, "Tide tables.pdf", 3 * MIB, 5),
        entry(7, "Trip notes.md", 2400, 2),
        entry(8, "Walking distances.csv", 780, 6),
    ]);
    let mut state = Snapshot {
        account: Some(crate::preview::redact_account(EMAIL)),
        connection: Connection::Ready,
        storage_pending: false,
        storage: Some(Storage {
            used: 7 * 1024 * MIB,
            total: 20 * 1024 * MIB,
        }),
        ..Default::default()
    };
    let (send, commands) = sync_channel(16);
    let press = |ui: &mut Ui, state: &Snapshot, code| {
        ui.key(
            KeyEvent::new(code, KeyModifiers::NONE),
            state,
            &send,
            &AtomicBool::new(false),
        );
    };

    sign_in(&mut cast)?;
    let mut show = |ui: &mut Ui, state: &Snapshot, ms, caption: &'static str| {
        browser(&mut cast, ui, state, &local, ms, caption)
    };
    use KeyCode::{Char, Down};
    show(
        &mut ui,
        &state,
        3300,
        "Press u to pick a file from your computer.",
    )?;
    press(&mut ui, &state, Char('u'));
    let select = "Mark the video with Space, then press u to upload it.";
    show(&mut ui, &state, 1800, select)?;
    press(&mut ui, &state, Char(' '));
    show(&mut ui, &state, 2600, select)?;
    press(&mut ui, &state, Char('u'));
    let Command::Upload {
        sources,
        parent: 1,
        destination,
    } = commands.try_recv()?
    else {
        panic!("Expected upload")
    };
    assert_eq!(sources, [ui.upload_dir.join(&clip.name)]);
    state
        .jobs
        .push_back(transfer(1, Direction::Upload, &clip, &destination));
    ui.notice = None;
    const FRAME_MS: u32 = 400;
    const UPLOAD_MS: u32 = 8000;
    const DOWNLOAD_MS: u32 = 2800;
    for elapsed in (0..UPLOAD_MS).step_by(FRAME_MS as usize) {
        advance(&mut state.jobs[0], elapsed, UPLOAD_MS);
        show(
            &mut ui,
            &state,
            FRAME_MS,
            "You can keep browsing while the video uploads.",
        )?;
    }
    advance(&mut state.jobs[0], UPLOAD_MS, UPLOAD_MS);
    state.storage.as_mut().unwrap().used += clip.size;
    let mut entries = ui.page.as_ref().clone();
    entries.insert(1, clip.clone());
    state.page = Some(Page {
        parent: ui.parent(),
        request: ui.request,
        offset: 0,
        more: false,
        error: None,
        entries: Arc::new(entries),
    });
    state.revision += 1;
    ui.apply(&state);
    press(&mut ui, &state, Down);
    show(
        &mut ui,
        &state,
        4000,
        "The video is on MEGA now. Press d to download a copy.",
    )?;
    press(&mut ui, &state, Char('d'));
    press(&mut ui, &state, KeyCode::Tab);
    show(
        &mut ui,
        &state,
        4500,
        "Pick the folder where you want the copy, then press d again.",
    )?;
    press(&mut ui, &state, Char('d'));
    let Command::Download {
        entry,
        destination,
        overwrite: false,
    } = commands.try_recv()?
    else {
        panic!("Expected download")
    };
    assert_eq!((entry.id, destination), (clip.id, local.join("Downloads")));
    state
        .jobs
        .push_back(transfer(2, Direction::Download, &clip, "/demo/Downloads"));
    ui.notice = None;
    let downloaded = "You’ll find Coast walk.mp4 in Downloads.";
    for elapsed in (0..DOWNLOAD_MS).step_by(FRAME_MS as usize) {
        advance(&mut state.jobs[1], elapsed, DOWNLOAD_MS);
        show(&mut ui, &state, FRAME_MS, downloaded)?;
    }
    advance(&mut state.jobs[1], DOWNLOAD_MS, DOWNLOAD_MS);
    show(&mut ui, &state, 4000, downloaded)
}
