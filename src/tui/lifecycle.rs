use super::*;
use crate::{
    bridge::ffi,
    feedback::{self, Confirmation, Surface},
    operation::Stage,
};
use anyhow::{Context, ensure};
use std::{
    cell::RefCell,
    collections::VecDeque,
    io::IsTerminal,
    rc::Rc,
    sync::mpsc::{Receiver, RecvTimeoutError},
};
use zeroize::Zeroizing;
#[cfg(all(test, unix))]
mod tests;

#[derive(Clone, PartialEq, Eq)]
pub(super) enum Phase {
    Busy {
        message: String,
        percent: Option<u16>,
    },
    Input,
    Browser(u64),
    Done,
}
pub(super) struct AppState {
    pub initial_preview: Option<Entry>,
    pub phase: Phase,
    pub closing: bool,
    pub notes: VecDeque<String>,
    pub revision: u64,
    pub state_path: String,
}
type App = Arc<Mutex<AppState>>;
fn change(app: &App, action: impl FnOnce(&mut AppState)) {
    let mut state = app.lock().unwrap_or_else(|e| e.into_inner());
    action(&mut state);
    state.revision = state.revision.wrapping_add(1);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum InputMode {
    Text,
    Choice(Confirmation),
    Error,
}
struct Request {
    label: String,
    hidden: bool,
    max: usize,
    mode: InputMode,
    error: Option<&'static str>,
    reply: SyncSender<Result<Zeroizing<String>>>,
}
pub(super) struct Prompt {
    request: Request,
    value: Zeroizing<String>,
    pub choice: bool,
    pub error: Option<&'static str>,
}
#[cfg(test)]
pub(super) fn preview_prompt(label: &str, hidden: bool, value: &str, mode: InputMode) -> Prompt {
    let (reply, _) = sync_channel(1);
    Prompt {
        request: Request {
            label: label.into(),
            hidden,
            max: 4096,
            mode,
            error: None,
            reply,
        },
        value: Zeroizing::new(value.into()),
        choice: false,
        error: None,
    }
}
enum Action {
    None,
    Submit,
    Exit,
    SignInAgain,
}
impl Prompt {
    pub fn label(&self) -> &str {
        &self.request.label
    }
    pub fn hidden(&self) -> bool {
        self.request.hidden
    }
    pub fn mode(&self) -> InputMode {
        self.request.mode
    }
    pub fn display_value(&self) -> String {
        if self.hidden() {
            "•".repeat(self.value.chars().count().min(80))
        } else {
            safe(&self.value)
        }
    }
    fn paste(&mut self, text: &str) {
        if self.mode() != InputMode::Text {
            return;
        }
        if text.chars().any(char::is_control) {
            self.error = Some("Paste a single line without control characters.");
            return;
        }
        if self.value.len() + text.len() > self.request.max {
            self.error = Some("Input is too long.");
            return;
        }
        self.value.push_str(text);
        self.error = None;
    }
    fn key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::F(2) && self.label().starts_with("Unlock") {
            return Action::SignInAgain;
        }
        if self.mode() == InputMode::Error {
            return if matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q')) {
                Action::Submit
            } else {
                Action::None
            };
        }
        if key.code == KeyCode::Esc {
            return Action::Exit;
        }
        if matches!(self.mode(), InputMode::Choice(_)) {
            match key.code {
                KeyCode::Left => self.choice = false,
                KeyCode::Right => self.choice = true,
                KeyCode::Tab | KeyCode::BackTab => self.choice = !self.choice,
                KeyCode::Char('y' | 'Y') => self.choice = true,
                KeyCode::Char('n' | 'N') => self.choice = false,
                KeyCode::Enter => return Action::Submit,
                _ => (),
            }
        } else {
            match key.code {
                KeyCode::Enter => return Action::Submit,
                KeyCode::Backspace => {
                    self.value.pop();
                    self.error = None;
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.value.clear();
                    self.error = None;
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.paste(&c.to_string())
                }
                _ => (),
            }
        }
        Action::None
    }
    fn submit(self) {
        let answer = if matches!(self.mode(), InputMode::Choice(_)) {
            Zeroizing::new(if self.choice { "y" } else { "n" }.into())
        } else {
            self.value
        };
        let _ = self.request.reply.send(Ok(answer));
    }
}

struct Frontend {
    app: App,
    shared: Shared,
    prompts: SyncSender<Request>,
    commands: RefCell<Option<Receiver<Command>>>,
    alive: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}
impl Frontend {
    fn ask(
        &self,
        label: &str,
        hidden: bool,
        max: usize,
        mode: InputMode,
        error: Option<&'static str>,
        cancel: &AtomicBool,
    ) -> Result<Zeroizing<String>> {
        if mode != InputMode::Error {
            input::check(cancel)?;
        }
        let (send, receive) = sync_channel(1);
        self.prompts
            .send(Request {
                label: label.into(),
                hidden,
                max,
                mode,
                error,
                reply: send,
            })
            .context("Terminal interface closed")?;
        loop {
            if !self.alive.load(Ordering::Relaxed) {
                anyhow::bail!("Terminal interface closed");
            }
            if mode != InputMode::Error {
                input::check(cancel)?;
            }
            match receive.recv_timeout(Duration::from_millis(50)) {
                Ok(result) => return result,
                Err(RecvTimeoutError::Timeout) => (),
                Err(RecvTimeoutError::Disconnected) => return Err(input::Cancelled.into()),
            }
        }
    }
    fn done(&self) {
        change(&self.app, |s| s.phase = Phase::Done);
    }
}
impl Surface for Frontend {
    fn read(
        &self,
        label: &str,
        hidden: bool,
        max: usize,
        error: Option<&'static str>,
        cancel: &AtomicBool,
    ) -> Result<Zeroizing<String>> {
        self.ask(label, hidden, max, InputMode::Text, error, cancel)
    }
    fn confirm(&self, choice: Confirmation, cancel: &AtomicBool) -> Result<bool> {
        Ok(self
            .ask(
                choice.question(),
                false,
                1,
                InputMode::Choice(choice),
                None,
                cancel,
            )?
            .as_str()
            == "y")
    }
    fn progress(&self, stage: Stage, bytes: i64, total: i64, retry: i32, cancelling: bool) {
        let event = ffi::Event {
            done: false,
            code: 0,
            bytes,
            total,
            retry,
        };
        let message = if cancelling {
            "Stopping active work…".into()
        } else if retry != 0 {
            format!("{stage}… retrying")
        } else {
            stage.progress(&event)
        };
        let preparing = total > 0
            && bytes >= total
            && matches!(
                stage,
                Stage::LoadingCloud | Stage::LoadingShared | Stage::Downloading | Stage::Uploading
            );
        let progress = if total > 0 && !preparing {
            Some(percent(bytes, total))
        } else {
            None
        };
        let phase = Phase::Busy {
            message,
            percent: progress,
        };
        let mut state = self.app.lock().unwrap_or_else(|e| e.into_inner());
        if state.phase != phase {
            state.phase = phase;
            state.revision = state.revision.wrapping_add(1);
        }
    }
    fn note(&self, message: &str) {
        change(&self.app, |s| {
            s.notes.push_back(safe(message));
            while s.notes.len() > 2 {
                s.notes.pop_front();
            }
        });
    }
    fn browse(
        &self,
        backend: &mut Backend,
        preview: Option<crate::bridge::ffi::Entry>,
    ) -> Result<()> {
        input::check(&backend.cancel)?;
        let root = backend.api().root()?;
        let commands = self
            .commands
            .borrow_mut()
            .take()
            .context("Browser already opened")?;
        change(&self.app, |s| {
            s.initial_preview = preview.map(Into::into);
            s.phase = Phase::Browser(root);
            s.notes.clear();
        });
        backend.interactive_ui = true;
        let cancel = backend.cancel.clone();
        let result = worker::run(
            backend,
            self.shared.clone(),
            commands,
            &cancel,
            !backend.remembered,
        );
        backend.interactive_ui = false;
        self.closing();
        if snapshot(&self.shared).preview_failed {
            return Err(feedback::DisplayedError.into());
        }
        result
    }
    fn closing(&self) {
        input::arm_exit(&self.cancel);
        change(&self.app, |s| {
            s.closing = true;
            s.phase = Phase::Busy {
                message: "Closing SDK…".into(),
                percent: None,
            };
        });
    }
}

struct End(App);
impl Drop for End {
    fn drop(&mut self) {
        change(&self.0, |s| s.phase = Phase::Done);
    }
}

pub fn application(cancel: Arc<AtomicBool>, work: impl FnOnce() -> Result<()>) -> Result<()> {
    application_inner(cancel, |_| work())
}
fn application_inner(
    cancel: Arc<AtomicBool>,
    work: impl FnOnce(&Frontend) -> Result<()>,
) -> Result<()> {
    ensure!(
        io::stdout().is_terminal(),
        "The browser requires a terminal. Use ls, get, or put for redirected output."
    );
    let app = Arc::new(Mutex::new(AppState {
        initial_preview: None,
        phase: Phase::Busy {
            message: "Starting MEGA…".into(),
            percent: None,
        },
        closing: false,
        notes: VecDeque::new(),
        revision: 0,
        state_path: crate::state::home()?.display().to_string(),
    }));
    let shared = Arc::new(Mutex::new(Snapshot::default()));
    let (prompts, inputs) = sync_channel(1);
    let (commands, receive) = sync_channel(32);
    let (ready, started) = sync_channel(1);
    let alive = Arc::new(AtomicBool::new(true));
    std::thread::scope(|scope| -> Result<()> {
        let thread_app = app.clone();
        let thread_shared = shared.clone();
        let thread_cancel = cancel.clone();
        let thread_alive = alive.clone();
        let display = scope.spawn(move || {
            let result = terminal_app(
                thread_app,
                thread_shared.clone(),
                inputs,
                commands,
                thread_cancel.clone(),
                ready,
            );
            thread_alive.store(false, Ordering::Relaxed);
            update(&thread_shared, |s| s.closed = true);
            if result.is_err() {
                thread_cancel.store(true, Ordering::Relaxed);
            }
            result
        });
        if started.recv().is_err() {
            return display
                .join()
                .map_err(|_| anyhow::anyhow!("Terminal interface failed"))?;
        }
        let end = End(app.clone());
        let frontend = Rc::new(Frontend {
            app,
            shared,
            prompts,
            commands: RefCell::new(Some(receive)),
            alive,
            cancel: cancel.clone(),
        });
        let _presentation = feedback::install(frontend.clone());
        let result = work(&frontend);
        let failed = result
            .as_ref()
            .err()
            .filter(|e| !e.is::<input::Cancelled>());
        if let Some(error) = failed
            && !error.is::<feedback::DisplayedError>()
            && frontend.alive.load(Ordering::Relaxed)
        {
            let _ = frontend.ask(
                &crate::error::message(error),
                false,
                0,
                InputMode::Error,
                None,
                &cancel,
            );
        }
        frontend.done();
        drop(end);
        display
            .join()
            .map_err(|_| anyhow::anyhow!("Terminal interface failed"))??;
        if failed.is_some() {
            Err(feedback::DisplayedError.into())
        } else {
            Ok(())
        }
    })
}

fn begin_exit(app: &App, shared: &Shared, cancel: &AtomicBool) {
    input::arm_exit(cancel);
    update(shared, |s| s.closed = true);
    change(app, |s| {
        // Dropping the prompt or setting cancel can let the worker finish first.
        if matches!(s.phase, Phase::Done) {
            return;
        }
        s.closing = true;
        s.phase = Phase::Busy {
            message: "Stopping active work…".into(),
            percent: None,
        };
    });
}

fn terminal_app(
    app: App,
    shared: Shared,
    inputs: Receiver<Request>,
    commands: SyncSender<Command>,
    cancel: Arc<AtomicBool>,
    ready: SyncSender<()>,
) -> Result<()> {
    enable_raw_mode()?;
    let _guard = Guard;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let _ = ready.send(());
    let mut browser: Option<Ui> = None;
    let mut prompt: Option<Prompt> = None;
    let mut previous = u64::MAX;
    let mut dirty = true;
    let mut animated = Instant::now();
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) && !app.lock().unwrap_or_else(|e| e.into_inner()).closing
        {
            prompt = None;
            begin_exit(&app, &shared, &cancel);
        }
        if let Ok(request) = inputs.try_recv() {
            let mode = request.mode;
            if !cancel.load(Ordering::Relaxed) || mode == InputMode::Error {
                prompt = Some(Prompt {
                    error: request.error,
                    request,
                    value: Zeroizing::new(String::new()),
                    choice: false,
                });
                change(&app, |s| s.phase = Phase::Input);
            }
        }
        let state = snapshot(&shared);
        let life = app.lock().unwrap_or_else(|e| e.into_inner());
        if life.phase == Phase::Done {
            break;
        }
        if life.phase != Phase::Input {
            prompt = None;
        }
        if life.phase == Phase::Input && prompt.is_none() {
            // Keep the previous frame and queued keys until the controller
            // supplies the next field or work stage. No empty in-between frame.
            drop(life);
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        if previous != life.revision {
            previous = life.revision;
            dirty = true;
        }
        if let Phase::Browser(root) = life.phase {
            if browser.is_none() {
                let mut ui = Ui::new(root)?;
                if let Some(entry) = life.initial_preview.clone() {
                    ui.preview_only = true;
                    ui.preview_entry(entry, &commands);
                } else {
                    ui.list(&commands);
                }
                browser = Some(ui);
            }
            let ui = browser.as_mut().unwrap();
            dirty |= ui.apply(&state);
            if ui
                .notice
                .as_ref()
                .is_some_and(|(_, time)| time.elapsed() > Duration::from_secs(5))
            {
                ui.notice = None;
                dirty = true;
            }
        } else if matches!(life.phase, Phase::Busy { .. })
            && animated.elapsed() >= Duration::from_millis(100)
        {
            animated = Instant::now();
            dirty = true;
        }
        if dirty {
            terminal.draw(|frame| {
                if matches!(life.phase, Phase::Browser(_))
                    && let Some(ui) = browser.as_ref()
                {
                    view::draw(frame, ui, &state);
                } else {
                    view::draw_lifecycle(frame, &life, prompt.as_ref(), started.elapsed());
                }
            })?;
            dirty = false;
        }
        let closing = life.closing;
        let phase = life.phase.clone();
        drop(life);
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        match event::read()? {
            Event::Resize(..) => dirty = true,
            Event::Paste(text) => {
                let text = Zeroizing::new(text);
                if let Some(p) = prompt.as_mut() {
                    p.paste(&text);
                    dirty = true;
                } else if matches!(phase, Phase::Browser(_))
                    && let Some(ui) = browser.as_mut()
                {
                    ui.paste(&text);
                    dirty = true;
                }
            }
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let interrupt =
                    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
                if interrupt {
                    if closing {
                        input::force_exit();
                    }
                    input::interrupt(&cancel);
                    prompt = None;
                    begin_exit(&app, &shared, &cancel);
                } else if let Some(p) = prompt.as_mut() {
                    match p.key(key) {
                        Action::Submit => {
                            let p = prompt.take().unwrap();
                            p.submit();
                        }
                        Action::Exit => {
                            prompt = None;
                            begin_exit(&app, &shared, &cancel);
                        }
                        Action::SignInAgain => {
                            let p = prompt.take().unwrap();
                            change(&app, |s| {
                                s.phase = Phase::Busy {
                                    message: "Signing in again…".into(),
                                    percent: None,
                                }
                            });
                            let _ = p.request.reply.send(Err(feedback::SignInAgain.into()));
                        }
                        Action::None => (),
                    }
                } else if matches!(phase, Phase::Browser(_))
                    && let Some(ui) = browser.as_mut()
                {
                    ui.key(key, &state, &commands, &cancel);
                    if ui.closed {
                        if ui.preview_only
                            && state.preview.as_ref().is_some_and(|p| p.error.is_some())
                        {
                            update(&shared, |s| s.preview_failed = true);
                        }
                        begin_exit(&app, &shared, &cancel);
                    }
                } else if !closing && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    begin_exit(&app, &shared, &cancel);
                }
                dirty = true;
            }
            _ => (),
        }
    }
    Ok(())
}
