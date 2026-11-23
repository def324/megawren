use crate::{bridge::ffi, input};
use anyhow::{Result, bail};
use std::{
    cell::Cell,
    io::{self, IsTerminal, Write},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
    },
    time::{Duration, Instant},
};

pub trait Pending {
    fn poll(&mut self) -> Result<ffi::Event>;
    fn cancel(&mut self) -> Result<()>;
}
pub struct Native<'a> {
    pub client: Pin<&'a mut ffi::Client>,
    pub revoke_session: bool,
}
impl Pending for Native<'_> {
    fn poll(&mut self) -> Result<ffi::Event> {
        Ok(self.client.as_mut().event()?)
    }
    fn cancel(&mut self) -> Result<()> {
        Ok(self.client.as_mut().cancel(self.revoke_session)?)
    }
}

#[derive(Clone, Copy)]
pub enum Stage {
    Working,
    SigningIn,
    RestoringLogin,
    LoadingCloud,
    LoadingShared,
    Downloading,
    Uploading,
    SigningOut,
    ReadingIndex,
    SavingIndex,
    UnlockingLogin,
    SavingLogin,
    CreatingFolder,
    Moving,
    Copying,
    Trashing,
}
impl Stage {
    fn label(self) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::SigningIn => "Signing in",
            Self::RestoringLogin => "Restoring saved login",
            Self::LoadingCloud => "Loading cloud drive",
            Self::LoadingShared => "Loading shared folder",
            Self::Downloading => "Downloading",
            Self::Uploading => "Uploading",
            Self::SigningOut => "Signing out",
            Self::ReadingIndex => "Reading local index",
            Self::SavingIndex => "Saving local index",
            Self::UnlockingLogin => "Unlocking saved login",
            Self::SavingLogin => "Saving login",
            Self::CreatingFolder => "Creating folder",
            Self::Moving => "Moving",
            Self::Copying => "Copying",
            Self::Trashing => "Moving to Rubbish Bin",
        }
    }
    fn preparing_label(self) -> Option<&'static str> {
        match self {
            Self::LoadingCloud => Some("Opening cloud drive…"),
            Self::LoadingShared => Some("Opening shared folder…"),
            Self::Downloading => Some("Finishing download…"),
            Self::Uploading => Some("Finishing upload…"),
            _ => None,
        }
    }
    pub(crate) fn progress(self, state: &ffi::Event) -> String {
        if state.total > 0
            && state.bytes >= state.total
            && let Some(message) = self.preparing_label()
        {
            return message.to_owned();
        }
        progress_label(self.label(), state.bytes, state.total)
    }
}
impl std::fmt::Display for Stage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

const STATUS_DELAY: Duration = Duration::from_secs(3);
const STARTUP_DELAY: Duration = Duration::from_secs(2);
thread_local! {
    static STARTUP_STARTED: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// All saved-login startup stages share a clock; short stages cannot repeatedly
/// reset the feedback delay. This is scoped to the controller thread and ends
/// before the TUI starts, so transfer feedback keeps its usual timing.
pub struct StartupFeedback {
    previous: Option<Instant>,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl StartupFeedback {
    pub fn begin() -> Self {
        Self::at(Instant::now())
    }
    fn at(started: Instant) -> Self {
        Self {
            previous: STARTUP_STARTED.replace(Some(started)),
            _thread: std::marker::PhantomData,
        }
    }
}
impl Drop for StartupFeedback {
    fn drop(&mut self) {
        STARTUP_STARTED.set(self.previous);
    }
}

pub fn progress_label(label: &str, bytes: i64, total: i64) -> String {
    if total <= 0 {
        return format!("{label}…");
    }
    let ceiling = if bytes < total { 99 } else { 100 };
    let percent = ((bytes.max(0) as u128 * 100 + total as u128 / 2) / total as u128).min(ceiling);
    format!("{label}… {percent}%")
}

static CLI_PROGRESS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

pub fn configure_cli_progress(json: bool) {
    CLI_PROGRESS.get_or_init(|| !json && input::can_prompt() && io::stdout().is_terminal());
}

pub(crate) struct Status<W: Write> {
    stage: Stage,
    visible: bool,
    started: Instant,
    delay: Duration,
    painted: Option<String>,
    output: W,
}
impl Status<io::Stderr> {
    pub(crate) fn new(stage: Stage, visible: bool) -> Self {
        Self::with_clock(
            stage,
            visible,
            CLI_PROGRESS.get().copied().unwrap_or(false),
            Instant::now(),
            io::stderr(),
        )
    }
}
impl<W: Write> Status<W> {
    fn with_clock(stage: Stage, visible: bool, tty: bool, now: Instant, output: W) -> Self {
        let startup = STARTUP_STARTED.get();
        let mut status = Self::with_output(stage, visible, tty, startup.unwrap_or(now), output);
        if startup.is_some() {
            status.delay = STARTUP_DELAY;
        }
        status
    }
    pub(crate) fn immediate(mut self) -> Self {
        self.delay = Duration::ZERO;
        self
    }
    fn with_output(stage: Stage, visible: bool, tty: bool, started: Instant, output: W) -> Self {
        Self {
            stage,
            visible: visible && tty,
            started,
            delay: STATUS_DELAY,
            painted: None,
            output,
        }
    }
    pub(crate) fn show(&mut self, state: &ffi::Event, cancelling: bool, now: Instant) {
        if crate::feedback::progress(
            self.stage,
            state.bytes,
            state.total,
            state.retry,
            cancelling,
        ) {
            return;
        }
        if !self.visible || (!cancelling && now.duration_since(self.started) < self.delay) {
            return;
        }
        let mut message = if cancelling {
            "Cancelling…".to_owned()
        } else if state.retry != 0 {
            format!("{}… retrying", self.stage)
        } else {
            self.stage.progress(state)
        };
        if !cancelling
            && state.retry == 0
            && (state.total <= 0
                || (state.bytes >= state.total && self.stage.preparing_label().is_some()))
        {
            let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let frame = (now.duration_since(self.started).as_millis() / 100) % frames.len() as u128;
            message = format!("{} {message}", frames[frame as usize]);
        }
        if self.painted.as_ref() == Some(&message) {
            return;
        }
        let _ = write!(self.output, "\r\x1b[2K{message}");
        let _ = self.output.flush();
        self.painted = Some(message);
    }
}
impl<W: Write> Drop for Status<W> {
    fn drop(&mut self) {
        if self.painted.is_some() {
            let _ = write!(self.output, "\r\x1b[2K");
            let _ = self.output.flush();
        }
    }
}

pub fn wait(
    pending: &mut impl Pending,
    cancel: &AtomicBool,
    progress: Option<&SyncSender<(i64, i64)>>,
    stage: Stage,
    ui: bool,
    idle_limit: Duration,
) -> Result<i32> {
    let mut status = Status::new(stage, !ui);
    let mut activity = Instant::now();
    let mut bytes = 0;
    let mut cancelling = None;
    let mut timed_out = false;
    loop {
        let event = pending.poll()?;
        if event.bytes != bytes {
            activity = Instant::now();
            bytes = event.bytes;
            if let Some(sender) = progress {
                let _ = sender.try_send((event.bytes, event.total));
            }
        }
        let interrupted = cancel.load(Ordering::Relaxed);
        if event.done {
            if timed_out {
                bail!("MEGA stopped responding while {stage}");
            }
            if cancelling.is_some() || interrupted {
                return Err(input::Cancelled.into());
            }
            return Ok(event.code);
        }
        if cancelling.is_none() && (interrupted || activity.elapsed() > idle_limit) {
            timed_out = !interrupted;
            cancelling = Some(Instant::now());
            status.show(&event, true, Instant::now());
            pending.cancel()?;
        }
        if cancelling.is_some_and(|start: Instant| start.elapsed() > Duration::from_secs(2)) {
            if interrupted {
                return Err(input::Cancelled.into());
            }
            bail!("MEGA stopped responding while {stage}");
        }
        status.show(&event, cancelling.is_some(), Instant::now());
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_startup_stages_share_the_feedback_delay_then_restore_transfer_timing() {
        let start = Instant::now();
        let event = ffi::Event {
            done: false,
            code: 0,
            bytes: 0,
            total: 0,
            retry: 0,
        };
        let mut output = Vec::new();
        {
            let _startup = StartupFeedback::at(start);
            {
                let mut cache =
                    Status::with_clock(Stage::ReadingIndex, true, true, start, &mut output);
                cache.show(&event, false, start + Duration::from_millis(1500));
            }
            assert!(output.is_empty());
            let mut login = Status::with_clock(
                Stage::RestoringLogin,
                true,
                true,
                start + Duration::from_millis(1500),
                &mut output,
            );
            login.show(&event, false, start + Duration::from_millis(2100));
        }
        assert!(String::from_utf8_lossy(&output).contains("Restoring saved login…"));
        output.clear();
        let mut transfer = Status::with_clock(
            Stage::Downloading,
            true,
            true,
            start + Duration::from_secs(3),
            &mut output,
        );
        transfer.show(&event, false, start + Duration::from_secs(4));
        assert!(transfer.output.is_empty());
    }
    #[test]
    fn unattended_progress_writes_nothing_even_on_retry_or_cancellation() {
        let start = Instant::now();
        let mut output = Vec::new();
        {
            let mut status =
                Status::with_output(Stage::Downloading, true, false, start, &mut output);
            let event = ffi::Event {
                done: false,
                bytes: 256,
                total: 512,
                code: 0,
                retry: 1,
            };
            status.show(&event, false, start + Duration::from_secs(4));
            status.show(&event, true, start + Duration::from_secs(5));
        }
        assert!(output.is_empty());
    }
    #[test]
    fn interrupted_shutdown_can_show_cache_saving_without_waiting() {
        let start = Instant::now();
        let mut output = Vec::new();
        let mut status =
            Status::with_output(Stage::SavingIndex, true, true, start, &mut output).immediate();
        status.show(
            &ffi::Event {
                done: false,
                code: 0,
                bytes: 0,
                total: 100,
                retry: 0,
            },
            false,
            start,
        );
        assert!(String::from_utf8_lossy(status.output).contains("Saving local index… 0%"));
    }
    #[test]
    fn completed_metadata_switches_to_opening_instead_of_staying_at_99_percent() {
        let mut output = Vec::new();
        let start = Instant::now();
        let mut event = ffi::Event {
            done: false,
            code: 0,
            bytes: 999,
            total: 1000,
            retry: 0,
        };
        let mut status = Status::with_output(Stage::LoadingCloud, true, true, start, &mut output);
        status.show(&event, false, start + Duration::from_secs(3));
        assert!(String::from_utf8_lossy(status.output).ends_with("Loading cloud drive… 99%"));
        event.bytes = 1000;
        status.show(&event, false, start + Duration::from_secs(4));
        assert!(String::from_utf8_lossy(status.output).ends_with("Opening cloud drive…"));
        let opening = status.painted.clone().unwrap();
        assert!(!opening.contains('%'));
        status.show(&event, false, start + Duration::from_millis(4100));
        assert_ne!(status.painted.as_ref().unwrap(), &opening);
    }
    #[test]
    fn fast_operations_leave_no_status_or_blank_line() {
        let mut output = Vec::new();
        let start = Instant::now();
        {
            let mut status = Status::with_output(Stage::SigningIn, true, true, start, &mut output);
            status.show(
                &ffi::Event {
                    done: false,
                    code: 0,
                    bytes: 0,
                    total: 0,
                    retry: 0,
                },
                false,
                start + Duration::from_secs(1),
            );
        }
        assert!(output.is_empty());
    }
    #[test]
    fn loading_is_delayed_and_displays_only_percentage_without_duplicate_updates() {
        let mut output = Vec::new();
        let start = Instant::now();
        let event = ffi::Event {
            done: false,
            code: 0,
            bytes: 675,
            total: 4894,
            retry: 0,
        };
        {
            let mut status =
                Status::with_output(Stage::LoadingCloud, true, true, start, &mut output);
            status.show(&event, false, start + Duration::from_secs(2));
            assert!(status.output.is_empty());
            status.show(&event, false, start + Duration::from_secs(3));
            assert_eq!(
                String::from_utf8_lossy(status.output),
                "\r\x1b[2KLoading cloud drive… 14%"
            );
            let length = status.output.len();
            status.show(&event, false, start + Duration::from_secs(44));
            assert_eq!(status.output.len(), length);
        }
        assert!(output.ends_with(b"\r\x1b[2K"));
        assert!(!output.contains(&b'\n'));
    }
    #[test]
    fn cancellation_is_immediate_and_unknown_progress_is_not_fabricated() {
        let mut output = Vec::new();
        let start = Instant::now();
        let event = ffi::Event {
            done: false,
            code: 0,
            bytes: 0,
            total: 0,
            retry: 0,
        };
        let mut status = Status::with_output(Stage::LoadingCloud, true, true, start, &mut output);
        status.show(&event, true, start);
        assert_eq!(
            String::from_utf8_lossy(status.output),
            "\r\x1b[2KCancelling…"
        );
        assert_eq!(
            progress_label("Loading cloud drive", 0, 0),
            "Loading cloud drive…"
        );
        assert_eq!(
            progress_label("Loading cloud drive", 100, 100),
            "Loading cloud drive… 100%"
        );
    }
    struct Fake<'a> {
        cancel: &'a AtomicBool,
        requested: bool,
        progress: bool,
    }
    impl Pending for Fake<'_> {
        fn poll(&mut self) -> Result<ffi::Event> {
            if !self.progress && !self.requested {
                self.cancel.store(true, Ordering::Relaxed);
            }
            Ok(ffi::Event {
                done: self.requested || self.progress,
                code: 0,
                bytes: if self.progress { 42 } else { 0 },
                total: 42,
                retry: 0,
            })
        }
        fn cancel(&mut self) -> Result<()> {
            self.requested = true;
            Ok(())
        }
    }
    #[test]
    fn interrupt_cancels_pending_work_and_returns_typed_error() {
        let cancel = AtomicBool::new(false);
        let mut pending = Fake {
            cancel: &cancel,
            requested: false,
            progress: false,
        };
        let result = wait(
            &mut pending,
            &cancel,
            None,
            Stage::LoadingCloud,
            true,
            Duration::from_secs(1),
        );
        assert!(pending.requested);
        assert!(result.unwrap_err().is::<input::Cancelled>());
    }
    #[test]
    fn completion_does_not_depend_on_a_free_progress_slot() {
        let cancel = AtomicBool::new(false);
        let mut pending = Fake {
            cancel: &cancel,
            requested: false,
            progress: true,
        };
        let (send, _recv) = std::sync::mpsc::sync_channel(1);
        send.send((0, 42)).unwrap();
        assert_eq!(
            wait(
                &mut pending,
                &cancel,
                Some(&send),
                Stage::Downloading,
                true,
                Duration::from_secs(1)
            )
            .unwrap(),
            0
        );
        assert!(!pending.requested);
    }
}
