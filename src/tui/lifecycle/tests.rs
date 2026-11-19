use super::*;
use std::{
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn terminal_flags(fd: i32) -> libc::tcflag_t {
    let mut attrs = std::mem::MaybeUninit::<libc::termios>::uninit();
    assert_eq!(unsafe { libc::tcgetattr(fd, attrs.as_mut_ptr()) }, 0);
    unsafe { attrs.assume_init() }.c_lflag & (libc::ECHO | libc::ICANON | libc::ISIG)
}
struct Pty {
    master: File,
    observer: File,
    child: Child,
    output: Vec<u8>,
}
impl Pty {
    fn new(mode: &str, fixture: &std::path::Path) -> Self {
        let (mut master, mut slave) = (-1, -1);
        let mut size = libc::winsize {
            ws_row: 28,
            ws_col: 110,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::addr_of_mut!(size),
                )
            },
            0
        );
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        assert_ne!(terminal_flags(slave.as_raw_fd()) & libc::ECHO, 0);
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--ignored",
            "--exact",
            "tui::lifecycle::tests::application_child",
            "--nocapture",
        ])
        .env("MEGAWREN_UI_TEST_CASE", mode)
        .env("MEGAWREN_UI_TEST_DIR", fixture)
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave.try_clone().unwrap()));
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().unwrap();
        Self {
            master,
            observer: slave,
            child,
            output: Vec::new(),
        }
    }
    fn until(&mut self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !String::from_utf8_lossy(&self.output).contains(needle) {
            assert!(
                Instant::now() < deadline,
                "Timed out waiting for {needle}; output: {}",
                String::from_utf8_lossy(&self.output)
            );
            let mut poll = libc::pollfd {
                fd: self.master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut poll, 1, 100) } > 0 {
                let mut bytes = [0u8; 4096];
                let n = self.master.read(&mut bytes).unwrap();
                assert!(n > 0, "PTY closed before prompt");
                self.output.extend_from_slice(&bytes[..n]);
            }
        }
    }
    fn type_bytes(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
    }
    fn signal(&self) {
        assert_eq!(unsafe { libc::kill(self.child.id() as _, libc::SIGINT) }, 0);
    }
    fn finish(&mut self, code: i32) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mut poll = libc::pollfd {
                fd: self.master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut poll, 1, 10) } > 0 && poll.revents & libc::POLLIN != 0 {
                let mut bytes = [0u8; 4096];
                if let Ok(n) = self.master.read(&mut bytes) {
                    self.output.extend_from_slice(&bytes[..n]);
                }
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                assert_eq!(status.code(), Some(code));
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Prompt did not exit after Ctrl-C; output: {}",
                String::from_utf8_lossy(&self.output)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        // The child asserts restoration before exit. macOS may invalidate the
        // slave's termios as the controlling session exits, so checking it here
        // can return EIO despite correct restoration.
        let output = String::from_utf8_lossy(&self.output);
        assert!(
            output.contains("Terminal restored")
                || output.contains("Interrupted; private working files"),
            "No restoration confirmation: {output}"
        );
    }
}
impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        // Close the PTY before reaping: a macOS session leader can be waiting
        // for terminal teardown while the parent still holds the master open.
        if let Ok(null) = File::open("/dev/null") {
            drop(std::mem::replace(&mut self.master, null));
        }
        if let Ok(null) = File::open("/dev/null") {
            drop(std::mem::replace(&mut self.observer, null));
        }
        for _ in 0..20 {
            if self.child.try_wait().ok().flatten().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

const FIXTURE: &str = "  synthetic pässphrase  ";
const SYNTHETIC_SESSION: &[u8] = b"offline lifecycle test session, no real account";

#[test]
fn empty_passphrase_needs_explicit_consent_and_then_opens_automatically() {
    let fixture = tempfile::tempdir().unwrap();
    let mut p = Pty::new("choose-passphrase", fixture.path());
    p.until("(optional)");
    p.type_bytes(b"\r");
    p.until("anyway");
    assert!(!fixture.path().join("state/session.enc").exists());
    p.type_bytes(b"\r"); // The default returns to the field.
    p.output.clear();
    p.until("(optional)");
    p.type_bytes(b"\ry\r");
    p.finish(0);
    let stored = std::fs::read(fixture.path().join("state/session.enc")).unwrap();
    assert!(crate::vault::opens_automatically(&stored));
    assert_eq!(
        &*crate::vault::open(b"", &stored).unwrap(),
        SYNTHETIC_SESSION
    );
}

#[test]
fn confirmation_mismatch_can_be_corrected_with_a_one_character_passphrase() {
    let fixture = tempfile::tempdir().unwrap();
    let mut p = Pty::new("choose-passphrase", fixture.path());
    p.until("(optional)");
    p.type_bytes(b"x\ry\r");
    p.until("didn");
    p.type_bytes(b"x\rx\r");
    p.finish(0);
    let stored = std::fs::read(fixture.path().join("state/session.enc")).unwrap();
    assert!(!crate::vault::opens_automatically(&stored));
    assert_eq!(
        &*crate::vault::open(b"x", &stored).unwrap(),
        SYNTHETIC_SESSION
    );
}

fn count(output: &str, needle: &str) -> usize {
    output.matches(needle).count()
}

#[test]
fn quit_and_ctrl_c_keep_one_terminal_session_and_save_the_same_index() {
    for mode in ["q", "ctrlc", "preview"] {
        let fixture = tempfile::tempdir().unwrap();
        let mut p = Pty::new(mode, fixture.path());
        p.until("F2 sign");
        p.type_bytes(format!("{FIXTURE}\r").as_bytes());
        p.until(if mode == "preview" {
            "preview-fixture"
        } else {
            "fixture.txt"
        });
        p.type_bytes(match mode {
            "q" => b"q",
            "preview" => &[27],
            _ => &[3],
        });
        p.finish(0);
        let output = String::from_utf8_lossy(&p.output);
        assert!(!output.contains(FIXTURE));
        assert!(!output.contains("Cancelled"));
        assert_eq!(count(&output, "\x1b[?1049h"), 1);
        assert_eq!(count(&output, "\x1b[?1049l"), 1);
        assert!(
            count(&output, "\x1b[2J") <= 1,
            "stage changes must not clear the whole screen"
        );
        assert!(
            !output.contains("\r\x1b[2K"),
            "CLI progress must not leak into the TUI"
        );
        let key = crate::cache::key(SYNTHETIC_SESSION).unwrap();
        let (restored, format) = crate::cache::restore(
            &fixture.path().join("state/cache.enc"),
            &key,
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        let expected = match std::env::var("MEGAWREN_COMPRESS_CACHE").as_deref() {
            Ok("0") => crate::cache::Format::Original,
            _ => crate::cache::Format::Zstd,
        };
        assert_eq!(format, expected);
        assert_eq!(
            std::fs::read(restored.path().join("megaclient_statecache20_synthetic.db")).unwrap(),
            b"completed index"
        );
    }
}

#[test]
fn a_late_exit_request_preserves_the_finished_phase() {
    let app = Arc::new(Mutex::new(AppState {
        initial_preview: None,
        phase: Phase::Done,
        closing: false,
        notes: VecDeque::new(),
        revision: 0,
        state_path: String::new(),
    }));
    let shared = Arc::new(Mutex::new(Snapshot::default()));
    begin_exit(&app, &shared, &AtomicBool::new(false));
    assert!(matches!(app.lock().unwrap().phase, Phase::Done));
}

#[test]
fn hidden_input_cancellation_is_a_clean_exit_without_a_cache() {
    let fixture = tempfile::tempdir().unwrap();
    let mut p = Pty::new("cancel-prompt", fixture.path());
    p.until("F2 sign");
    p.type_bytes(FIXTURE.as_bytes());
    p.type_bytes(&[3]);
    p.finish(0);
    let output = String::from_utf8_lossy(&p.output);
    assert!(!output.contains(FIXTURE) && !output.contains("Cancelled"));
    assert!(!fixture.path().join("state/cache.enc").exists());
}

#[test]
fn consecutive_fields_preserve_buffered_input_without_exposing_the_secret() {
    let fixture = tempfile::tempdir().unwrap();
    let mut p = Pty::new("sequence", fixture.path());
    p.until("Email");
    p.type_bytes(format!("test@example.invalid\r{FIXTURE}\r").as_bytes());
    p.until("fixture.txt");
    p.type_bytes(b"q");
    p.finish(0);
    assert!(!String::from_utf8_lossy(&p.output).contains(FIXTURE));
}

#[test]
fn errors_are_shown_inside_the_same_terminal_and_not_repeated_after_exit() {
    let fixture = tempfile::tempdir().unwrap();
    let mut p = Pty::new("error", fixture.path());
    p.until("F2 sign");
    p.type_bytes(format!("{FIXTURE}\r").as_bytes());
    p.until("Could not continue");
    p.type_bytes(b"\r");
    p.finish(1);
    let output = String::from_utf8_lossy(&p.output);
    assert_eq!(count(&output, "\x1b[?1049h"), 1);
    assert_eq!(count(&output, "\x1b[?1049l"), 1);
    assert!(
        !output
            .rsplit("\x1b[?1049l")
            .next()
            .unwrap()
            .contains("Synthetic")
    );
    assert!(!output.contains(FIXTURE));
}

#[test]
fn forced_exit_during_cleanup_keeps_the_existing_snapshot() {
    let fixture = tempfile::tempdir().unwrap();
    let state = crate::state::State::at(fixture.path().join("state")).unwrap();
    let run = state.run().unwrap();
    std::fs::write(
        run.path().join("megaclient_statecache20_synthetic.db"),
        b"previous index",
    )
    .unwrap();
    let key = crate::cache::key(SYNTHETIC_SESSION).unwrap();
    crate::cache::save(
        &state.path.join("cache.enc"),
        run.path(),
        &key,
        &AtomicBool::new(false),
    )
    .unwrap();
    let previous = std::fs::read(state.path.join("cache.enc")).unwrap();
    let mut p = Pty::new("force", fixture.path());
    p.until("F2 sign");
    p.type_bytes(format!("{FIXTURE}\r").as_bytes());
    p.until("fixture.txt");
    p.type_bytes(b"q");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !fixture.path().join("closing").exists() {
        assert!(Instant::now() < deadline);
        let mut poll = libc::pollfd {
            fd: p.master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, 10) } > 0 && poll.revents & libc::POLLIN != 0 {
            let mut bytes = [0u8; 4096];
            if let Ok(n) = p.master.read(&mut bytes) {
                p.output.extend_from_slice(&bytes[..n]);
            }
        }
    }
    p.signal();
    p.finish(130);
    assert_eq!(
        std::fs::read(state.path.join("cache.enc")).unwrap(),
        previous
    );
}

#[test]
fn prompt_masks_secrets_bounds_paste_and_exposes_recovery_as_an_action() {
    let (reply, receive) = sync_channel(1);
    let mut prompt = Prompt {
        request: Request {
            label: "Unlock saved login: ".into(),
            hidden: true,
            max: 32,
            mode: InputMode::Text,
            error: None,
            reply,
        },
        value: Zeroizing::new(String::new()),
        choice: false,
        error: None,
    };
    prompt.paste(FIXTURE);
    assert!(!prompt.display_value().contains(FIXTURE));
    prompt.paste("\x1b[2J");
    assert_eq!(prompt.value.as_str(), FIXTURE);
    prompt.paste(&"x".repeat(100));
    assert_eq!(prompt.value.as_str(), FIXTURE);
    assert!(matches!(
        prompt.key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE)),
        Action::SignInAgain
    ));
    prompt.submit();
    assert_eq!(receive.recv().unwrap().unwrap().as_str(), FIXTURE);
}

#[test]
#[ignore = "spawned by offline lifecycle PTY tests"]
fn application_child() {
    let mode = std::env::var("MEGAWREN_UI_TEST_CASE").unwrap();
    let fixture = PathBuf::from(std::env::var_os("MEGAWREN_UI_TEST_DIR").unwrap());
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    ctrlc::set_handler(move || input::interrupt(&signal)).unwrap();
    let before = terminal_flags(0);
    let result = application_inner(cancel.clone(), |frontend| {
        let state = crate::state::State::at(fixture.join("state"))?;
        if mode == "choose-passphrase" {
            let pass = crate::choose_passphrase(&cancel)?;
            state.save(&crate::vault::seal(pass.as_bytes(), SYNTHETIC_SESSION)?)?;
            return Ok(());
        }
        if mode == "sequence" {
            let email = crate::line("Email: ", &cancel)?;
            assert_eq!(email, "test@example.invalid");
        }
        let pass = crate::prompt("Unlock saved login: ", &cancel)?;
        assert_eq!(pass.as_str(), FIXTURE);
        drop(pass);
        if mode == "error" {
            anyhow::bail!("Synthetic connection failure");
        }
        feedback::progress(Stage::LoadingCloud, 50, 100, 0, false);
        std::thread::sleep(Duration::from_millis(100));
        update(&frontend.shared, |s| {
            if mode == "preview" {
                s.preview = Some(Arc::new(PreviewState {
                    request: 1,
                    name: "preview-fixture".into(),
                    bytes: 1,
                    total: 1,
                    decoding: false,
                    error: None,
                    content: Some(Arc::new(crate::preview::Content::Text {
                        lines: vec!["preview-fixture".into()],
                        truncated: false,
                    })),
                }));
            }
            s.page = Some(Page {
                more: false,
                request: 1,
                parent: 1,
                offset: 0,
                error: None,
                entries: Arc::new(vec![Entry {
                    id: 2,
                    name: "fixture.txt".into(),
                    size: 42,
                    folder: false,
                    modified: 0,
                    shared: false,
                    exported: false,
                    parent: 0,
                    fingerprint: String::new(),
                }]),
            })
        });
        change(&frontend.app, |s| {
            if mode == "preview" {
                s.initial_preview = Some(Entry {
                    id: 2,
                    name: "preview-fixture.txt".into(),
                    size: 1,
                    folder: false,
                    modified: 0,
                    shared: false,
                    exported: false,
                    parent: 0,
                    fingerprint: String::new(),
                });
            }
            s.phase = Phase::Browser(1);
        });
        while !cancel.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(10));
        }
        feedback::closing();
        feedback::progress(Stage::SavingIndex, 0, 100, 0, false);
        std::fs::write(fixture.join("closing"), b"ready")?;
        if mode == "force" {
            loop {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        std::thread::sleep(Duration::from_millis(150));
        let run = state.run()?;
        std::fs::write(
            run.path().join("megaclient_statecache20_synthetic.db"),
            b"completed index",
        )?;
        let key = crate::cache::key(SYNTHETIC_SESSION)?;
        crate::cache::save_after_interrupt(&state.path.join("cache.enc"), run.path(), &key)?;
        Err(input::Cancelled.into())
    });
    if mode == "error" {
        assert!(result.unwrap_err().is::<feedback::DisplayedError>());
    } else {
        result.unwrap();
    }
    assert_eq!(terminal_flags(0), before);
    eprintln!("Terminal restored");
    if mode == "error" {
        std::process::exit(1);
    }
}
