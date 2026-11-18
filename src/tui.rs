mod actions;
mod file_preview;
mod icons;
mod lifecycle;
mod model;
mod picker;
#[cfg(test)]
mod preview;
mod rate;
mod view;
mod worker;

use crate::{backend::Backend, input};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
pub use lifecycle::application;
use model::*;
use picker::Picker;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{SyncSender, sync_channel},
    },
    time::{Duration, Instant},
};

struct TransferCancellation {
    ids: Vec<u64>,
    subject: String,
    confirm: bool,
}
enum Modal {
    Preview {
        request: u64,
        scroll: u16,
        column: u16,
        image: std::cell::RefCell<Option<file_preview::PreviewImage>>,
    },
    Action(actions::Dialog),
    Upload {
        picker: Picker,
        parent: u64,
        destination: String,
    },
    Download {
        picker: Picker,
        entries: Vec<Entry>,
        overwrite: bool,
    },
    Transfers {
        selected: Option<TransferFocus>,
        cancellation: Option<TransferCancellation>,
    },
    Help,
    Quit {
        confirm: bool,
    },
    Error(String),
}
struct BrowserLocation {
    folders: Vec<(u64, String)>,
    offset: usize,
    cursor: usize,
    viewport: usize,
    page: Arc<Vec<Entry>>,
}
struct Ui {
    preview_only: bool,
    root: u64,
    serial: u64,
    nerd_icons: bool,
    viewport: std::cell::Cell<usize>,
    review_visible: std::cell::Cell<bool>,
    detail_scroll_limit: std::cell::Cell<u16>,
    preview_rows: std::cell::Cell<u16>,
    folders: Vec<(u64, String)>,
    marked: Vec<Selection>,
    other_location: Option<BrowserLocation>,
    offset: usize,
    cursor: usize,
    request: u64,
    page: Arc<Vec<Entry>>,
    loading: bool,
    more: bool,
    list_error: Option<String>,
    positions: std::collections::HashMap<u64, (usize, usize, usize)>,
    modal: Option<Modal>,
    notice: Option<(String, Instant)>,
    worker_notice: Option<u64>,
    revision: u64,
    download_dir: PathBuf,
    upload_dir: PathBuf,
    closed: bool,
}
impl Ui {
    fn new(root: u64) -> Result<Self> {
        let cwd = std::fs::canonicalize(std::env::current_dir()?)?;
        Ok(Self {
            preview_only: false,
            root,
            serial: 0,
            nerd_icons: true,
            viewport: std::cell::Cell::new(0),
            review_visible: std::cell::Cell::new(false),
            detail_scroll_limit: std::cell::Cell::new(0),
            preview_rows: std::cell::Cell::new(1),
            folders: vec![(root, String::new())],
            marked: Vec::new(),
            other_location: None,
            offset: 0,
            cursor: 0,
            request: 0,
            page: Arc::new(Vec::new()),
            loading: true,
            more: false,
            list_error: None,
            positions: Default::default(),
            modal: None,
            notice: None,
            worker_notice: None,
            revision: u64::MAX,
            download_dir: cwd.clone(),
            upload_dir: cwd,
            closed: false,
        })
    }
    fn path(&self) -> String {
        let path = self
            .folders
            .iter()
            .map(|(_, n)| n.as_str())
            .collect::<Vec<_>>()
            .join("/");
        if path.is_empty() { "/".into() } else { path }
    }
    fn parent(&self) -> u64 {
        self.folders.last().unwrap().0
    }
    fn in_bin(&self) -> bool {
        self.folders[0].0 != self.root
    }
    fn send(&mut self, commands: &SyncSender<Command>, command: Command) -> bool {
        match commands.try_send(command) {
            Ok(()) => true,
            Err(_) => {
                self.notice = Some(("Please wait for the current request".into(), Instant::now()));
                false
            }
        }
    }
    fn list(&mut self, commands: &SyncSender<Command>) {
        self.navigate(commands, self.parent(), self.offset, self.cursor);
    }
    fn navigate(
        &mut self,
        commands: &SyncSender<Command>,
        parent: u64,
        offset: usize,
        cursor: usize,
    ) -> bool {
        let request = self.request.wrapping_add(1);
        if !self.send(
            commands,
            Command::List {
                request,
                parent,
                offset,
            },
        ) {
            return false;
        }
        self.marked.retain(|s| s.parent == parent);
        self.request = request;
        self.loading = true;
        self.list_error = None;
        self.cursor = cursor;
        self.offset = offset;
        self.viewport.set(cursor);
        self.page = Arc::default();
        true
    }
    fn apply(&mut self, snapshot: &Snapshot) -> bool {
        if self.revision == snapshot.revision {
            return false;
        }
        self.revision = snapshot.revision;
        if let Some(page) = &snapshot.page
            && page.request == self.request
            && page.parent == self.parent()
            && page.offset == self.offset
        {
            self.more = page.more;
            self.list_error = page.error.clone();
            if let Some(error) = &page.error {
                self.notice = Some((error.clone(), Instant::now()));
            }
            let selected_id = self.page.get(self.cursor).map(|entry| entry.id);
            self.page = page.entries.clone();
            self.cursor = selected_id
                .and_then(|id| self.page.iter().position(|entry| entry.id == id))
                .unwrap_or(self.cursor.min(self.page.len().saturating_sub(1)));
            self.loading = false;
        }
        if let Some(Modal::Transfers { selected, .. }) = &mut self.modal
            && !selected.is_some_and(|focus| snapshot.jobs.iter().any(|job| job.id == focus.job))
        {
            *selected = snapshot.preferred_transfer();
        }
        if let Some((id, notice)) = &snapshot.notice
            && self.worker_notice != Some(*id)
        {
            self.worker_notice = Some(*id);
            self.notice = Some((notice.clone(), Instant::now()));
        }
        true
    }
    fn open_upload(&mut self) {
        if self.folders[0].0 != self.root {
            self.notice = Some((
                "Choose a folder in the cloud drive to upload".into(),
                Instant::now(),
            ));
            return;
        }
        match Picker::new(&self.upload_dir, false) {
            Ok(picker) => {
                self.modal = Some(Modal::Upload {
                    picker,
                    parent: self.parent(),
                    destination: self.path(),
                })
            }
            Err(e) => self.modal = Some(Modal::Error(e.to_string())),
        }
    }
    fn open_download(&mut self) {
        let entries: Vec<_> = self.targets().into_iter().map(|s| s.entry).collect();
        if entries.is_empty() {
            return;
        }
        match Picker::new(&self.download_dir, true) {
            Ok(mut picker) => {
                picker.check_download_names(&entries);
                self.modal = Some(Modal::Download {
                    picker,
                    entries,
                    overwrite: false,
                })
            }
            Err(e) => self.modal = Some(Modal::Error(e.to_string())),
        }
    }
    fn key(
        &mut self,
        key: KeyEvent,
        snapshot: &Snapshot,
        commands: &SyncSender<Command>,
        cancel: &AtomicBool,
    ) {
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c') {
            input::interrupt(cancel);
            self.closed = true;
            let _ = commands.try_send(Command::Quit);
            return;
        }
        let picker = matches!(
            self.modal,
            Some(Modal::Upload { .. } | Modal::Download { .. })
        );
        // Text fields own their editing chords. Modified keys must never fall through to file actions.
        if !(key.modifiers - KeyModifiers::SHIFT).is_empty() {
            let editing_key = key.modifiers == KeyModifiers::CONTROL
                && match key.code {
                    KeyCode::Char('l') => picker,
                    KeyCode::Char('u') => {
                        picker
                            || matches!(
                                self.modal,
                                Some(Modal::Action(actions::Dialog::Name { .. }))
                            )
                    }
                    _ => false,
                };
            if !editing_key {
                return;
            }
        }
        if key.code == KeyCode::F(3) && (self.modal.is_none() || picker) {
            self.nerd_icons = !self.nerd_icons;
            return;
        }
        if let Some(modal) = self.modal.take() {
            self.modal = self.modal_key(modal, key, snapshot, commands);
            return;
        }
        match key.code {
            KeyCode::Char('1') => self.switch_location(false, snapshot, commands),
            KeyCode::Char('2') => self.switch_location(true, snapshot, commands),
            KeyCode::Esc if !self.marked.is_empty() => self.marked.clear(),
            KeyCode::Esc if self.in_bin() => self.switch_location(false, snapshot, commands),
            KeyCode::Char(' ') if !self.loading => {
                self.toggle_mark();
            }
            KeyCode::Enter
                if !self.loading
                    && (!self.marked.is_empty()
                        || self.page.get(self.cursor).is_some_and(|e| !e.folder)) =>
            {
                self.open_sheet(commands)
            }
            KeyCode::Char('i') if !self.loading => self.open_sheet(commands),
            KeyCode::Char('p') if !self.loading => self.open_preview(commands),
            KeyCode::Delete if !self.in_bin() && !self.loading => self.review_trash(commands),
            KeyCode::F(2) if !self.in_bin() && !self.loading => {
                self.edit_name(actions::NameKind::Rename)
            }
            KeyCode::Char('n') if !self.in_bin() && !self.loading => {
                self.edit_name(actions::NameKind::NewFolder)
            }
            KeyCode::Char('m') | KeyCode::Char('c') if !self.in_bin() && !self.loading => {
                self.destination(key.code == KeyCode::Char('c'), commands)
            }
            KeyCode::Up | KeyCode::Char('k')
                if self.cursor == 0 && self.offset > 0 && !self.loading =>
            {
                self.navigate(
                    commands,
                    self.parent(),
                    self.offset.saturating_sub(PAGE_SIZE),
                    PAGE_SIZE - 1,
                );
            }
            KeyCode::Down | KeyCode::Char('j')
                if self.more && self.cursor + 1 == self.page.len() && !self.loading =>
            {
                self.navigate(commands, self.parent(), self.offset + PAGE_SIZE, 0);
            }
            KeyCode::Up | KeyCode::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(self.page.len().saturating_sub(1))
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') if !self.loading => {
                if let Some(entry) = self.page.get(self.cursor).cloned()
                    && entry.folder
                {
                    let position = (self.offset, self.cursor, self.viewport.get());
                    if self.navigate(commands, entry.id, 0, 0) {
                        self.positions.insert(self.parent(), position);
                        self.folders.push((entry.id, entry.name));
                    }
                }
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace if self.folders.len() > 1 => {
                let parent = self.folders[self.folders.len() - 2].0;
                let (offset, cursor, viewport) =
                    self.positions.get(&parent).copied().unwrap_or_default();
                if self.navigate(commands, parent, offset, cursor) {
                    self.folders.pop();
                    self.positions.remove(&parent);
                    self.viewport.set(viewport);
                }
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace if self.in_bin() => {
                self.switch_location(false, snapshot, commands);
            }
            KeyCode::PageDown if self.more && !self.loading => {
                self.navigate(commands, self.parent(), self.offset + PAGE_SIZE, 0);
            }
            KeyCode::PageUp if !self.loading => {
                self.navigate(
                    commands,
                    self.parent(),
                    self.offset.saturating_sub(PAGE_SIZE),
                    0,
                );
            }
            KeyCode::Char('u') => self.open_upload(),
            KeyCode::Char('d') if !self.loading => self.open_download(),
            KeyCode::Char('r') => {
                self.send(commands, Command::Refresh);
                self.list(commands);
            }
            KeyCode::Char('t') => {
                self.modal = Some(Modal::Transfers {
                    selected: snapshot.preferred_transfer(),
                    cancellation: None,
                })
            }
            KeyCode::Char('?') | KeyCode::F(1) => self.modal = Some(Modal::Help),
            KeyCode::Char('q') => {
                if snapshot.action.is_some() || snapshot.jobs.iter().any(|j| !j.state.terminal()) {
                    self.modal = Some(Modal::Quit { confirm: false });
                } else {
                    self.closed = true;
                    self.send(commands, Command::Quit);
                }
            }
            _ => (),
        }
    }
    fn modal_key(
        &mut self,
        mut modal: Modal,
        key: KeyEvent,
        snapshot: &Snapshot,
        commands: &SyncSender<Command>,
    ) -> Option<Modal> {
        if let Modal::Preview {
            request,
            scroll,
            mut column,
            image,
        } = modal
        {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('p')
            ) {
                self.send(commands, Command::CancelPreview(request));
                if self.preview_only {
                    self.closed = true;
                }
                return None;
            }
            let lines = snapshot
                .preview
                .as_ref()
                .and_then(|p| p.content.as_ref())
                .and_then(|c| match c.as_ref() {
                    crate::preview::Content::Text { lines, .. } => Some(lines.len()),
                    _ => None,
                })
                .unwrap_or(0)
                .saturating_sub(self.preview_rows.get() as usize)
                .min(u16::MAX as usize) as u16;
            let scroll = match key.code {
                KeyCode::Left | KeyCode::Char('h') => {
                    column = column.saturating_sub(8);
                    scroll
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    column = column.saturating_add(8).min(32_768);
                    scroll
                }
                KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => scroll.saturating_add(1).min(lines),
                KeyCode::PageUp => scroll.saturating_sub(15),
                KeyCode::PageDown | KeyCode::Char(' ') => scroll.saturating_add(15).min(lines),
                KeyCode::Home => 0,
                KeyCode::End => lines,
                _ => scroll,
            };
            return Some(Modal::Preview {
                request,
                scroll,
                column,
                image,
            });
        }
        if let Modal::Action(dialog) = modal {
            let next = self
                .action_key(dialog, key, snapshot, commands)
                .map(Modal::Action);
            return next.or_else(|| self.modal.take());
        }
        match &mut modal {
            Modal::Preview { .. } => unreachable!(),
            Modal::Download {
                picker,
                entries,
                overwrite,
            } if picker.location_input.is_some() => {
                download_picker_key(picker, entries, overwrite, key);
                return Some(modal);
            }
            Modal::Upload { picker, .. } if picker.location_input.is_some() => {
                picker_key(picker, key);
                return Some(modal);
            }
            _ => (),
        }
        if let Modal::Transfers { cancellation, .. } = &mut modal
            && let Some(prompt) = cancellation
        {
            match key.code {
                KeyCode::Right => prompt.confirm = true,
                KeyCode::Left => prompt.confirm = false,
                KeyCode::Tab | KeyCode::BackTab => prompt.confirm = !prompt.confirm,
                KeyCode::Esc => *cancellation = None,
                KeyCode::Enter if !prompt.confirm => *cancellation = None,
                KeyCode::Enter if self.send(commands, Command::Cancel(prompt.ids.clone())) => {
                    *cancellation = None;
                }
                _ => (),
            }
            return Some(modal);
        }
        if key.code == KeyCode::Esc
            || (matches!(modal, Modal::Help)
                && matches!(key.code, KeyCode::Char('?') | KeyCode::F(1)))
            || (matches!(modal, Modal::Transfers { .. }) && key.code == KeyCode::Char('t'))
        {
            return None;
        }
        match &mut modal {
            Modal::Action(_) => unreachable!(),
            Modal::Preview { .. } => unreachable!(),
            Modal::Upload {
                picker,
                parent,
                destination,
            } => {
                if (key.modifiers.is_empty() && key.code == KeyCode::Char('u'))
                    || (key.code == KeyCode::Enter && picker.button_focus)
                {
                    match picker.sources() {
                        Ok(sources) => {
                            self.upload_dir = picker.directory.clone();
                            if self.send(
                                commands,
                                Command::Upload {
                                    sources,
                                    parent: *parent,
                                    destination: destination.clone(),
                                },
                            ) {
                                self.notice = Some(("Upload queued".into(), Instant::now()));
                                return None;
                            }
                        }
                        Err(e) => picker.error = Some(e.to_string()),
                    }
                } else {
                    picker_key(picker, key);
                }
            }
            Modal::Download {
                picker,
                entries,
                overwrite,
            } => {
                if key.code == KeyCode::Char('o') && picker.conflict {
                    *overwrite = !*overwrite;
                    picker.error = None;
                } else if (key.modifiers.is_empty() && key.code == KeyCode::Char('d'))
                    || (key.code == KeyCode::Enter && picker.button_focus)
                {
                    if entries
                        .iter()
                        .any(|entry| picker.directory.join(&entry.name).exists() && !entry.folder)
                        && !*overwrite
                    {
                        picker.check_download_names(entries);
                        picker.error = Some(
                            "File exists. Choose another folder, or press o to allow replacement."
                                .into(),
                        );
                    } else {
                        self.download_dir = picker.directory.clone();
                        if self.send(
                            commands,
                            if entries.len() == 1 {
                                Command::Download {
                                    entry: entries[0].clone(),
                                    destination: picker.directory.clone(),
                                    overwrite: *overwrite,
                                }
                            } else {
                                Command::DownloadMany {
                                    entries: entries.clone(),
                                    destination: picker.directory.clone(),
                                    overwrite: *overwrite,
                                }
                            },
                        ) {
                            self.notice = Some(("Download queued".into(), Instant::now()));
                            self.marked.clear();
                            return None;
                        }
                    }
                } else {
                    download_picker_key(picker, entries, overwrite, key);
                }
            }
            Modal::Transfers {
                selected,
                cancellation,
            } => {
                let job =
                    selected.and_then(|focus| snapshot.jobs.iter().find(|j| j.id == focus.job));
                match key.code {
                    KeyCode::Up
                    | KeyCode::Char('k')
                    | KeyCode::Down
                    | KeyCode::Char('j')
                    | KeyCode::PageUp
                    | KeyCode::PageDown
                    | KeyCode::Home
                    | KeyCode::End => {
                        let mut rows = snapshot.transfer_rows();
                        let current = rows
                            .position(|(focus, _, _)| Some(focus) == *selected)
                            .unwrap_or(0);
                        let len: usize = snapshot.jobs.iter().map(|j| j.items.len()).sum();
                        let next = match key.code {
                            KeyCode::Up | KeyCode::Char('k') => current.saturating_sub(1),
                            KeyCode::PageUp => current.saturating_sub(8),
                            KeyCode::PageDown => current + 8,
                            KeyCode::Home => 0,
                            KeyCode::End => len,
                            _ => current + 1,
                        };
                        *selected = snapshot
                            .transfer_rows()
                            .nth(next.min(len.saturating_sub(1)))
                            .map(|(focus, _, _)| focus);
                    }
                    KeyCode::Char('x' | 'X') => {
                        let all = key.code == KeyCode::Char('X');
                        let jobs: Vec<_> = snapshot
                            .jobs
                            .iter()
                            .filter(|j| {
                                j.state.cancellable()
                                    && (all || job.is_some_and(|selected| selected.id == j.id))
                            })
                            .collect();
                        let subject = match jobs.as_slice() {
                            [] => return Some(modal),
                            [j] if j.items.len() > 1 => {
                                format!("{} items · {}", j.items.len(), j.name)
                            }
                            [j] => j.name.clone(),
                            many => format!("{} transfers", many.len()),
                        };
                        *cancellation = Some(TransferCancellation {
                            subject,
                            ids: jobs.iter().map(|j| j.id).collect(),
                            confirm: false,
                        });
                    }
                    _ => (),
                }
            }
            Modal::Quit { confirm } => match key.code {
                KeyCode::Left => *confirm = false,
                KeyCode::Right => *confirm = true,
                KeyCode::Tab | KeyCode::BackTab => *confirm = !*confirm,
                KeyCode::Enter if *confirm => {
                    self.closed = true;
                    self.send(commands, Command::Quit);
                    return None;
                }
                KeyCode::Enter => return None,
                _ => (),
            },
            Modal::Help => (),
            Modal::Error(_) => {
                if key.code == KeyCode::Enter {
                    return None;
                }
            }
        }
        Some(modal)
    }
}
fn download_picker_key(
    picker: &mut Picker,
    entries: &[Entry],
    overwrite: &mut bool,
    key: KeyEvent,
) {
    let before = picker.directory.clone();
    picker_key(picker, key);
    if before != picker.directory {
        *overwrite = false;
        picker.check_download_names(entries);
    }
}
fn picker_key(picker: &mut Picker, key: KeyEvent) {
    picker.error = None;
    if picker.location_input.is_some() {
        match key.code {
            KeyCode::Esc => picker.location_input = None,
            KeyCode::Enter => {
                if let Err(e) = picker.enter_location() {
                    picker.error = Some(e.to_string());
                }
            }
            KeyCode::Backspace => {
                picker.location_input.as_mut().unwrap().pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                picker.location_input.as_mut().unwrap().clear()
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                picker.paste_location(&c.to_string())
            }
            _ => (),
        }
        return;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('l') {
        picker.edit_location();
        return;
    }
    let result = match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            picker.move_by(-1);
            Ok(())
        }
        KeyCode::Down | KeyCode::Char('j') => {
            picker.move_by(1);
            Ok(())
        }
        KeyCode::PageUp => picker.page(false),
        KeyCode::PageDown => picker.page(true),
        KeyCode::Enter => picker.open(),
        KeyCode::Right | KeyCode::Char('l') => {
            picker.button_focus = false;
            if picker.entries.get(picker.cursor).is_some_and(|e| e.folder) {
                picker.open()
            } else {
                Ok(())
            }
        }
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => picker.parent(),
        KeyCode::Char(' ') => picker.toggle(),
        KeyCode::Char('.') => picker.toggle_hidden(),
        KeyCode::Tab | KeyCode::BackTab => {
            picker.button_focus = !picker.button_focus;
            Ok(())
        }
        _ => Ok(()),
    };
    if let Err(e) = result {
        picker.error = Some(e.to_string());
    }
}
struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    }
}
pub fn download(
    backend: &mut Backend,
    entries: Vec<crate::bridge::ffi::Entry>,
    destination: PathBuf,
    overwrite: bool,
) -> Result<()> {
    worker::cli(
        backend,
        Command::DownloadMany {
            entries: entries.into_iter().map(Into::into).collect(),
            destination,
            overwrite,
        },
    )
}
pub fn upload(
    backend: &mut Backend,
    sources: Vec<PathBuf>,
    parent: u64,
    destination: String,
) -> Result<()> {
    // Each queued upload pins its source with an open file descriptor.
    // Bound that working set even when the shell expands a very large glob.
    for (i, chunk) in sources.chunks(MAX_SELECTION).enumerate() {
        if let Err(e) = worker::cli(
            backend,
            Command::Upload {
                sources: chunk.to_vec(),
                parent,
                destination: destination.clone(),
            },
        ) {
            if i != 0 && !e.is::<input::Cancelled>() {
                anyhow::bail!("{} earlier sources completed. {e}", i * MAX_SELECTION);
            }
            return Err(e);
        }
    }
    Ok(())
}
pub fn run(backend: &mut Backend) -> Result<()> {
    crate::feedback::browse(backend)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    #[test]
    fn pickers_start_in_launch_directory_and_support_arrows_and_direct_locations() {
        let mut ui = Ui::new(1).unwrap();
        let cwd = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
        assert_eq!(ui.download_dir, cwd);
        assert_eq!(ui.upload_dir, cwd);
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let nested = root.join("d u café");
        std::fs::create_dir(&nested).unwrap();
        ui.download_dir = root.clone();
        ui.loading = false;
        ui.page = Arc::new(vec![Entry {
            id: 2,
            name: "archive.tar.zst".into(),
            size: 42,
            folder: false,
            modified: 0,
            shared: false,
            exported: false,
            parent: 0,
            fingerprint: String::new(),
        }]);
        let state = Snapshot::default();
        let (send, receive) = sync_channel(8);
        let cancel = AtomicBool::new(false);
        ui.key(key(KeyCode::Char('d')), &state, &send, &cancel);
        ui.key(key(KeyCode::Right), &state, &send, &cancel);
        let Some(Modal::Download { picker, .. }) = &ui.modal else {
            panic!("Expected picker");
        };
        assert_eq!(picker.directory, nested);
        ui.key(key(KeyCode::Left), &state, &send, &cancel);
        let Some(Modal::Download { picker, .. }) = &ui.modal else {
            panic!("Expected picker");
        };
        assert_eq!(picker.directory, root);
        assert_eq!(picker.entries[picker.cursor].path, nested);
        for c in ['l', 'u'] {
            ui.key(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
                &state,
                &send,
                &cancel,
            );
        }
        for c in nested.to_str().unwrap().chars() {
            ui.key(key(KeyCode::Char(c)), &state, &send, &cancel);
        }
        ui.key(key(KeyCode::Enter), &state, &send, &cancel);
        let Some(Modal::Download { picker, .. }) = &ui.modal else {
            panic!("Expected picker");
        };
        assert_eq!(picker.directory, nested);
        assert!(picker.location_input.is_none());
        assert!(
            receive.try_recv().is_err(),
            "typing d/u in a path must not start a transfer"
        );
        ui.key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &state,
            &send,
            &cancel,
        );
        ui.key(key(KeyCode::Esc), &state, &send, &cancel);
        assert!(
            ui.modal.is_some(),
            "Esc leaves path editing before closing the dialog"
        );
        ui.key(key(KeyCode::Esc), &state, &send, &cancel);
        assert!(ui.modal.is_none());
    }
    #[test]
    fn upload_picker_queues_multiple_selections_without_leaving_the_tui() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"a").unwrap();
        std::fs::write(tmp.path().join("b.txt"), b"b").unwrap();
        let mut ui = Ui::new(1).unwrap();
        ui.upload_dir = std::fs::canonicalize(tmp.path()).unwrap();
        let state = Snapshot::default();
        let (send, recv) = sync_channel(4);
        let cancel = AtomicBool::new(false);
        ui.key(key(KeyCode::Char('u')), &state, &send, &cancel);
        for code in [
            KeyCode::Char(' '),
            KeyCode::Down,
            KeyCode::Char(' '),
            KeyCode::Tab,
            KeyCode::Enter,
        ] {
            ui.key(key(code), &state, &send, &cancel);
        }
        assert!(ui.modal.is_none());
        assert!(!ui.closed);
        match recv.try_recv().unwrap() {
            Command::Upload {
                sources, parent, ..
            } => {
                assert_eq!(sources.len(), 2);
                assert_eq!(parent, 1);
            }
            _ => panic!("Expected upload"),
        }
    }
    #[test]
    fn download_dialog_requires_explicit_replacement_and_stays_in_app() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("exists.txt"), b"old").unwrap();
        let mut ui = Ui::new(1).unwrap();
        ui.loading = false;
        ui.download_dir = std::fs::canonicalize(tmp.path()).unwrap();
        ui.page = Arc::new(vec![Entry {
            id: 2,
            name: "exists.txt".into(),
            size: 3,
            folder: false,
            modified: 0,
            shared: false,
            exported: false,
            parent: 0,
            fingerprint: String::new(),
        }]);
        let state = Snapshot::default();
        let (send, recv) = sync_channel(4);
        let cancel = AtomicBool::new(false);
        ui.key(key(KeyCode::Char('d')), &state, &send, &cancel);
        ui.key(key(KeyCode::Char('d')), &state, &send, &cancel);
        assert!(recv.try_recv().is_err());
        assert!(ui.modal.is_some());
        ui.key(key(KeyCode::Char('o')), &state, &send, &cancel);
        ui.key(key(KeyCode::Char('d')), &state, &send, &cancel);
        assert!(matches!(
            recv.try_recv().unwrap(),
            Command::Download {
                overwrite: true,
                ..
            }
        ));
        assert!(ui.modal.is_none());
        assert_eq!(
            std::fs::read(tmp.path().join("exists.txt")).unwrap(),
            b"old"
        );
    }
}
