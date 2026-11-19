//! Foreground presentation boundary. CLI code keeps its existing output when no
//! surface is installed. The browser installs one surface for its whole lifetime.
use crate::{backend::Backend, operation::Stage};
use anyhow::Result;
use std::{cell::RefCell, rc::Rc, sync::atomic::AtomicBool};
use zeroize::Zeroizing;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    Remember,
    WithoutPassphrase,
    Replace,
    Expired,
}
impl Confirmation {
    pub fn question(self) -> &'static str {
        match self {
            Self::Remember => "Remember login on this device?",
            Self::WithoutPassphrase => {
                "Anyone who can read your saved login can access your MEGA account."
            }
            Self::Replace => "Replace saved login? The old server session stays active.",
            Self::Expired => "Saved login expired. Sign in again?",
        }
    }
}

pub trait Surface {
    fn read(
        &self,
        label: &str,
        hidden: bool,
        max: usize,
        error: Option<&'static str>,
        cancel: &AtomicBool,
    ) -> Result<Zeroizing<String>>;
    fn confirm(&self, choice: Confirmation, cancel: &AtomicBool) -> Result<bool>;
    fn progress(&self, stage: Stage, bytes: i64, total: i64, retry: i32, cancelling: bool);
    fn note(&self, message: &str);
    fn browse(
        &self,
        backend: &mut Backend,
        preview: Option<crate::bridge::ffi::Entry>,
    ) -> Result<()>;
    fn closing(&self);
}
#[derive(Debug)]
pub struct DisplayedError;
impl std::fmt::Display for DisplayedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Operation failed")
    }
}
impl std::error::Error for DisplayedError {}
#[derive(Debug)]
pub struct SignInAgain;
impl std::fmt::Display for SignInAgain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sign in again")
    }
}
impl std::error::Error for SignInAgain {}
thread_local! { static SURFACE: RefCell<Option<Rc<dyn Surface>>> = const { RefCell::new(None) }; }
pub struct Guard(Option<Rc<dyn Surface>>);
pub fn install(surface: Rc<dyn Surface>) -> Guard {
    Guard(SURFACE.replace(Some(surface)))
}
impl Drop for Guard {
    fn drop(&mut self) {
        SURFACE.set(self.0.take());
    }
}
fn current() -> Option<Rc<dyn Surface>> {
    SURFACE.with_borrow(Clone::clone)
}
pub fn active() -> bool {
    SURFACE.with_borrow(Option::is_some)
}
pub fn read(
    label: &str,
    hidden: bool,
    max: usize,
    error: Option<&'static str>,
    cancel: &AtomicBool,
) -> Option<Result<Zeroizing<String>>> {
    current().map(|s| s.read(label, hidden, max, error, cancel))
}
pub fn confirm(choice: Confirmation, cancel: &AtomicBool) -> Option<Result<bool>> {
    current().map(|s| s.confirm(choice, cancel))
}
pub fn progress(stage: Stage, bytes: i64, total: i64, retry: i32, cancelling: bool) -> bool {
    if let Some(s) = current() {
        s.progress(stage, bytes, total, retry, cancelling);
        true
    } else {
        false
    }
}
pub fn note(message: impl AsRef<str>) {
    if let Some(s) = current() {
        s.note(message.as_ref());
    } else {
        eprintln!("{}", crate::state::display(message.as_ref()));
    }
}
pub fn browse(backend: &mut Backend) -> Result<()> {
    current()
        .ok_or_else(|| anyhow::anyhow!("Browser presentation is unavailable"))?
        .browse(backend, None)
}
pub fn preview(backend: &mut Backend, entry: crate::bridge::ffi::Entry) -> Result<()> {
    current()
        .ok_or_else(|| anyhow::anyhow!("Preview requires a terminal"))?
        .browse(backend, Some(entry))
}
pub fn closing() {
    if let Some(s) = current() {
        s.closing();
    }
}
