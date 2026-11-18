use super::model::*;
use crate::{backend::Backend, bridge::ffi, input, state, transfer};
use anyhow::{Result, bail, ensure};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError},
    },
    time::{Duration, Instant},
};
#[cfg(test)]
use std::{fs, path::PathBuf};
#[cfg(test)]
mod live;
mod manage;
mod previews;
#[cfg(test)]
mod throughput;

pub trait Remote {
    fn stop_sdk(&mut self);
    fn scratch(&self) -> Result<tempfile::TempDir>;
    fn state_path(&self) -> Option<&Path>;
    fn errors(&mut self) -> String;

    fn account_label(&mut self) -> Result<String>;
    fn preview_file(&mut self) -> Result<std::fs::File>;
    fn preview(&mut self, id: u64, file: &std::fs::File, bytes: i64) -> Result<()>;
    fn preview_event(&mut self) -> Result<ffi::Event>;
    fn cancel_preview(&mut self) -> Result<()>;
    fn change_many(
        &mut self,
        kind: u8,
        sources: &[u64],
        parents: &[u64],
        names: &[String],
        generation: u64,
    ) -> Result<()>;
    fn metadata(&mut self, id: u64) -> Result<ffi::Metadata>;
    fn upload_location(&mut self, id: u64) -> Result<String>;
    fn rubbish(&mut self) -> Result<u64>;
    fn generation(&mut self) -> u64;
    fn start_stats(&mut self, id: u64) -> Result<()>;
    fn stats(&mut self) -> Result<ffi::FolderStats>;
    fn folders(&mut self, id: u64, offset: usize) -> Result<Vec<Entry>>;
    fn change(
        &mut self,
        kind: u8,
        source: u64,
        parent: u64,
        name: &str,
        generation: u64,
    ) -> Result<()>;
    fn list(&mut self, id: u64, offset: usize, limit: usize) -> Result<Vec<Entry>>;
    fn info(&mut self, id: u64) -> Result<Entry>;
    fn find(&mut self, parent: u64, name: &str) -> Result<Option<Entry>>;
    fn mkdir(&mut self, parent: u64, name: &str) -> Result<()>;
    fn result_node(&mut self) -> Result<u64>;
    fn download(&mut self, id: u64, path: &Path) -> Result<()>;
    fn upload(&mut self, path: &Path, parent: u64, name: &str, expected: &str) -> Result<()>;
    fn event(&mut self) -> Result<ffi::Event>;
    fn cancel(&mut self, revoke: bool) -> Result<()>;
    fn refresh_account(&mut self) -> Result<()>;
    fn account(&mut self) -> Result<ffi::Account>;
    fn connection(&mut self) -> Result<i32>;
}
impl Remote for Backend {
    fn stop_sdk(&mut self) {
        Backend::stop_sdk(self)
    }
    fn scratch(&self) -> Result<tempfile::TempDir> {
        Backend::scratch(self)
    }
    fn state_path(&self) -> Option<&Path> {
        Some(self.state_root())
    }
    fn errors(&mut self) -> String {
        self.api().errors().unwrap_or_default()
    }

    fn account_label(&mut self) -> Result<String> {
        let email = zeroize::Zeroizing::new(self.api().email()?);
        Ok(crate::preview::redact_account(&email))
    }
    fn preview_file(&mut self) -> Result<std::fs::File> {
        Backend::preview_file(self)
    }
    fn preview(&mut self, id: u64, file: &std::fs::File, bytes: i64) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            Ok(self.api().preview(id, file.as_raw_fd(), bytes)?)
        }
        #[cfg(not(unix))]
        {
            let _ = (id, file, bytes);
            bail!("Secure previews are unavailable on this platform")
        }
    }
    fn preview_event(&mut self) -> Result<ffi::Event> {
        Ok(self.api().preview_event()?)
    }
    fn cancel_preview(&mut self) -> Result<()> {
        Ok(self.api().cancel_preview()?)
    }
    fn change_many(
        &mut self,
        kind: u8,
        sources: &[u64],
        parents: &[u64],
        names: &[String],
        generation: u64,
    ) -> Result<()> {
        Ok(self
            .api()
            .change_many(kind, sources, parents, names, generation)?)
    }
    fn upload_location(&mut self, id: u64) -> Result<String> {
        Ok(self.api().upload_location(id)?)
    }
    fn metadata(&mut self, id: u64) -> Result<ffi::Metadata> {
        Ok(self.api().metadata(id)?)
    }
    fn rubbish(&mut self) -> Result<u64> {
        Ok(self.api().rubbish()?)
    }
    fn generation(&mut self) -> u64 {
        self.api().generation()
    }
    fn start_stats(&mut self, id: u64) -> Result<()> {
        Ok(self.api().start_stats(id)?)
    }
    fn stats(&mut self) -> Result<ffi::FolderStats> {
        Ok(self.api().folder_stats()?)
    }
    fn folders(&mut self, id: u64, offset: usize) -> Result<Vec<Entry>> {
        Ok(self
            .api()
            .folders(id, offset, PAGE_SIZE + 1)?
            .into_iter()
            .map(Into::into)
            .collect())
    }
    fn change(
        &mut self,
        kind: u8,
        source: u64,
        parent: u64,
        name: &str,
        generation: u64,
    ) -> Result<()> {
        Ok(self.api().change(kind, source, parent, name, generation)?)
    }
    fn list(&mut self, id: u64, offset: usize, limit: usize) -> Result<Vec<Entry>> {
        Ok(self
            .api()
            .children(id, offset, limit)?
            .into_iter()
            .map(Into::into)
            .collect())
    }
    fn info(&mut self, id: u64) -> Result<Entry> {
        Ok(self.api().info(id)?.into())
    }
    fn find(&mut self, parent: u64, name: &str) -> Result<Option<Entry>> {
        Ok(Backend::find(self, parent, name)?.map(Into::into))
    }
    fn mkdir(&mut self, parent: u64, name: &str) -> Result<()> {
        Ok(self.api().mkdir(parent, name)?)
    }
    fn result_node(&mut self) -> Result<u64> {
        Ok(self.api().result_node()?)
    }
    fn download(&mut self, id: u64, path: &Path) -> Result<()> {
        Ok(self.api().download(id, state::native(path)?)?)
    }
    fn upload(&mut self, path: &Path, parent: u64, name: &str, expected: &str) -> Result<()> {
        Ok(self
            .api()
            .upload(state::native(path)?, parent, name, expected)?)
    }
    fn event(&mut self) -> Result<ffi::Event> {
        Ok(self.api().event()?)
    }
    fn cancel(&mut self, revoke: bool) -> Result<()> {
        Ok(self.api().cancel(revoke)?)
    }
    fn refresh_account(&mut self) -> Result<()> {
        Ok(self.api().refresh_account()?)
    }
    fn account(&mut self) -> Result<ffi::Account> {
        Ok(self.api().account()?)
    }
    fn connection(&mut self) -> Result<i32> {
        Ok(self.api().connection()?)
    }
}

use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use transfer::{Boundary, Source};

fn check_download_source(selected: &Entry, current: &Entry) -> Result<()> {
    ensure!(
        selected.id == current.id
            && selected.parent == current.parent
            && selected.name == current.name
            && selected.folder == current.folder
            && selected.size == current.size
            && selected.modified == current.modified
            && selected.fingerprint == current.fingerprint,
        "Cloud item changed. Select it again."
    );
    Ok(())
}

// Offset pagination needs a stable prefix. Replay it only after a node update,
// one page at a time, so a change elsewhere cannot discard this download.
#[derive(Default)]
struct FolderCursor {
    offset: usize,
    prefix: DefaultHasher,
    generation: u64,
    replay: Option<(usize, DefaultHasher)>,
}
const CHILD_PAGE: usize = 32;
fn hash_children(hash: &mut DefaultHasher, rows: &[Entry]) {
    for row in rows {
        (row.id, &row.name, row.folder).hash(hash);
    }
}

enum Work {
    Download {
        entry: Entry,
        destination: Arc<File>,
        overwrite: bool,
        depth: usize,
    },
    DownloadChildren {
        id: u64,
        cursor: FolderCursor,
        destination: Arc<File>,
        overwrite: bool,
        depth: usize,
    },
    Upload {
        source: Source,
        parent: u64,
        depth: usize,
    },
    UploadChildren {
        source: Source,
        entries: rustix::fs::Dir,
        parent: u64,
        depth: usize,
    },
}
enum Active {
    Download {
        scratch: tempfile::TempDir,
        destination: Arc<File>,
        entry: Entry,
        overwrite: bool,
    },
    Upload {
        _snapshot: tempfile::TempDir,
    },
    Mkdir {
        expected: String,
        source: Source,
        depth: usize,
    },
}
enum Prepared {
    Upload {
        scratch: tempfile::TempDir,
        parent: u64,
        name: String,
        target: String,
    },
    Download {
        file: transfer::PreparedDownload,
        entry: Entry,
        overwrite: bool,
    },
}
struct LocalTask {
    cancel: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Result<Prepared>>>,
}
impl LocalTask {
    fn start(f: impl FnOnce(&AtomicBool) -> Result<Prepared> + Send + 'static) -> Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("transfer-io".into())
            .spawn(move || f(&flag))?;
        Ok(Self {
            cancel,
            thread: Some(thread),
        })
    }
    fn finished(&self) -> bool {
        self.thread.as_ref().unwrap().is_finished()
    }
    fn join(mut self) -> Result<Prepared> {
        self.thread
            .take()
            .unwrap()
            .join()
            .map_err(|_| anyhow::anyhow!("Transfer I/O worker stopped"))?
    }
}
impl Drop for LocalTask {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.cancel.store(true, Ordering::Relaxed);
            let _ = thread.join();
        }
    }
}
struct Job {
    boundary: Boundary,
    published: std::collections::HashSet<(u64, u64)>,
    upload_targets: std::collections::HashMap<u64, String>,
    completed_bytes: i64,
    view: JobView,
    work: Vec<Work>,
    active: Option<Active>,
    local: Option<LocalTask>,
    cancelled: bool,
    abort_sent: bool,
    activity: Instant,
    last_bytes: i64,
    rate: super::rate::Rate,
    last_published: Instant,
}
impl Job {
    fn new(
        boundary: Boundary,
        id: u64,
        direction: Direction,
        items: Vec<TransferItem>,
        destination: String,
        work: Vec<Work>,
    ) -> Self {
        let now = Instant::now();
        let first = &items[0].name;
        let name = if items.len() == 1 {
            first.clone()
        } else {
            format!("{first} (+{})", items.len() - 1)
        };
        Self {
            boundary,
            published: Default::default(),
            upload_targets: Default::default(),
            completed_bytes: 0,
            view: JobView {
                id,
                direction,
                items: items.into(),
                current_item: None,
                name,
                file: String::new(),
                destination,
                state: JobState::Queued,
                bytes: 0,
                total: 0,
                speed: 0.0,
                files: 0,
                folders: 0,
                error: None,
            },
            work,
            active: None,
            local: None,
            cancelled: false,
            abort_sent: false,
            activity: now,
            last_bytes: 0,
            rate: super::rate::Rate::new(now),
            last_published: now,
        }
    }
    fn file(&mut self, name: String) {
        self.view.file = name;
        self.view.bytes = 0;
        self.view.total = 0;
        self.view.speed = 0.0;
        self.activity = Instant::now();

        self.last_bytes = 0;
        self.abort_sent = false;
    }
}
pub struct Engine<'a, R: Remote> {
    remote: &'a mut R,
    interrupt: Option<&'a AtomicBool>,
    shared: Shared,
    queued: VecDeque<Job>,
    active: Option<Job>,
    next_id: u64,
    current: Option<(u64, u64, usize)>,
    page_generation: u64,
    health: Instant,
    quota_refresh: Instant,
    closing: bool,
    fatal: Option<String>,
    interactive: bool,
    inspection: Option<manage::Review>,
    reviewed: Option<std::sync::Arc<Details>>,
    review_queue: VecDeque<Selection>,
    review_members: Vec<std::sync::Arc<Details>>,
    review_many: bool,
    preview: Option<previews::Pending>,
    changing: bool,
}
impl<'a, R: Remote> Engine<'a, R> {
    pub fn new(remote: &'a mut R, shared: Shared) -> Self {
        Self::build(remote, shared, true)
    }
    fn build(remote: &'a mut R, shared: Shared, interactive: bool) -> Self {
        let now = Instant::now();
        if interactive {
            let _ = remote.refresh_account();
            let rubbish = remote.rubbish().ok();
            let account = remote.account_label().ok();
            update(&shared, |s| {
                s.rubbish = rubbish;
                s.account = account;
            });
        }
        Self {
            remote,
            interrupt: None,
            shared,
            queued: VecDeque::new(),
            active: None,
            next_id: 1,
            current: None,
            page_generation: 0,
            health: now - Duration::from_secs(5),
            quota_refresh: now,
            closing: false,
            fatal: None,
            interactive,
            inspection: None,
            reviewed: None,
            review_queue: VecDeque::new(),
            review_members: Vec::new(),
            review_many: false,
            preview: None,
            changing: false,
        }
    }
    fn notice(&self, text: String) {
        update(&self.shared, |s| {
            s.notice = Some((s.revision.wrapping_add(1), safe(&text)))
        });
    }
    fn publish(&self, view: &JobView) {
        update(&self.shared, |s| {
            if let Some(existing) = s.jobs.iter_mut().find(|j| j.id == view.id) {
                *existing = view.clone();
            } else {
                s.jobs.push_back(view.clone());
            }
            while s.jobs.iter().filter(|j| j.state.terminal()).count() > HISTORY {
                if let Some(index) = s.jobs.iter().position(|j| j.state.terminal()) {
                    s.jobs.remove(index);
                } else {
                    break;
                }
            }
        });
    }
    fn page(&mut self, request: u64, parent: u64, offset: usize) {
        self.page_generation = self.remote.generation();
        let (mut entries, error) = match self.remote.list(parent, offset, PAGE_SIZE + 1) {
            Ok(rows) => (rows, None),
            Err(e) => (Vec::new(), Some(safe(&e.to_string()))),
        };
        let more = entries.len() > PAGE_SIZE;
        entries.truncate(PAGE_SIZE);
        update(&self.shared, |s| {
            s.page = Some(Page {
                more,
                request,
                parent,
                offset,
                entries: std::sync::Arc::new(entries),
                error,
            })
        });
    }
    fn enqueue(&mut self, command: Command) -> Result<()> {
        ensure!(
            !self.changing
                && self.reviewed.is_none()
                && !self.inspection.as_ref().is_some_and(|r| r.destructive),
            "Finish or cancel the file action first"
        );
        ensure!(
            self.queued.len() + usize::from(self.active.is_some()) < MAX_JOBS,
            "Transfer queue is full"
        );
        let boundary = Boundary::new(self.remote.state_path())?;
        let job = match command {
            Command::Download {
                entry,
                destination,
                overwrite,
            } => {
                state::component(&entry.name)?;
                let directory = boundary.directory(&destination)?;
                Job::new(
                    boundary,
                    self.next_id,
                    Direction::Download,
                    vec![TransferItem::from(&entry)],
                    destination.display().to_string(),
                    vec![Work::Download {
                        entry,
                        destination: directory,
                        overwrite,
                        depth: 0,
                    }],
                )
            }
            Command::DownloadMany {
                entries,
                destination,
                overwrite,
            } => {
                ensure!(
                    !entries.is_empty() && (!self.interactive || entries.len() <= MAX_SELECTION),
                    "Choose 1–{MAX_SELECTION} items"
                );
                let directory = boundary.directory(&destination)?;
                let mut names = std::collections::HashSet::new();
                let mut work = Vec::new();
                for entry in &entries {
                    state::component(&entry.name)?;
                    ensure!(
                        names.insert(entry.name.clone()),
                        "Selected items have conflicting names"
                    );
                    let current = self.remote.info(entry.id)?;
                    ensure!(current.name == entry.name, "Item changed; select it again");
                }
                for entry in entries.iter().rev() {
                    work.push(Work::Download {
                        entry: entry.clone(),
                        destination: directory.clone(),
                        overwrite,
                        depth: 0,
                    });
                }
                Job::new(
                    boundary,
                    self.next_id,
                    Direction::Download,
                    entries.iter().map(TransferItem::from).collect(),
                    destination.display().to_string(),
                    work,
                )
            }
            Command::Upload {
                sources,
                parent,
                destination,
            } => {
                ensure!(
                    Some(parent) != snapshot(&self.shared).rubbish
                        && !self.remote.metadata(parent)?.in_rubbish,
                    "Choose a folder in the cloud drive to upload"
                );
                ensure!(
                    !sources.is_empty() && sources.len() <= MAX_SELECTION,
                    "Choose 1–{MAX_SELECTION} items"
                );
                let target = self.remote.upload_location(parent)?;
                let mut items = Vec::with_capacity(sources.len());
                let mut work = Vec::with_capacity(sources.len());
                for path in sources {
                    let source = boundary.source(&path)?;
                    items.push(TransferItem {
                        name: source.name.clone(),
                        size: if source.meta.is_file() {
                            Some(i64::try_from(source.meta.len())?)
                        } else {
                            None
                        },
                        folder: source.meta.is_dir(),
                    });
                    work.push(Work::Upload {
                        source,
                        parent,
                        depth: 0,
                    });
                }
                work.reverse();
                let mut job = Job::new(
                    boundary,
                    self.next_id,
                    Direction::Upload,
                    items,
                    destination,
                    work,
                );
                job.upload_targets.insert(parent, target);
                job
            }
            _ => bail!("Invalid transfer request"),
        };
        self.next_id = self.next_id.wrapping_add(1);
        self.publish(&job.view);
        self.queued.push_back(job);
        Ok(())
    }
    pub fn command(&mut self, command: Command) {
        match command {
            Command::Preview { request, entry } => {
                if let Err(error) = self.start_preview(request, entry) {
                    self.preview_error(request, error.to_string());
                }
            }
            Command::CancelPreview(request) => self.close_preview(request),
            Command::List {
                request,
                parent,
                offset,
            } => {
                self.current = Some((request, parent, offset));
                self.page(request, parent, offset);
            }
            Command::Download { .. } | Command::DownloadMany { .. } | Command::Upload { .. } => {
                if let Err(e) = self.enqueue(command) {
                    self.notice(e.to_string());
                }
            }
            Command::Cancel(ids) => {
                for id in ids {
                    if let Some(job) = self.active.as_mut().filter(|j| j.view.id == id) {
                        job.cancelled = true;
                        if let Some(task) = &job.local {
                            task.cancel.store(true, Ordering::Relaxed);
                        }
                        job.view.state = JobState::Cancelling;
                        let view = job.view.clone();
                        self.publish(&view);
                    } else if let Some(index) = self.queued.iter().position(|j| j.view.id == id) {
                        self.queued.remove(index);
                        update(&self.shared, |s| s.jobs.retain(|job| job.id != id));
                    }
                }
            }
            Command::Refresh => {
                let _ = self.remote.refresh_account();
                self.quota_refresh = Instant::now();
                if let Some((r, p, o)) = self.current {
                    self.page(r, p, o);
                }
            }
            Command::Quit => self.closing = true,
            Command::Inspect {
                request,
                selection,
                review,
            } => {
                self.clear_review_many();
                if let Err(e) = self.inspect(request, selection, review) {
                    self.inspection_error(request, e);
                }
            }
            Command::InspectMany {
                request,
                selections,
            } => {
                if let Err(e) = self.inspect_many(request, selections) {
                    self.inspection_error(request, e);
                }
            }
            Command::CancelInspect(request) => {
                if snapshot(&self.shared)
                    .details
                    .as_ref()
                    .is_some_and(|d| d.request == request)
                {
                    self.inspection = None;
                    self.reviewed = None;
                    self.clear_review_many();
                    update(&self.shared, |s| s.details = None);
                }
            }
            Command::PickFolder {
                request,
                parent,
                offset,
            } => {
                let result = self.remote.folders(parent, offset);
                let (mut rows, error) = match result {
                    Ok(rows) => (rows, None),
                    Err(e) => (vec![], Some(safe(&e.to_string()))),
                };
                let more = rows.len() > PAGE_SIZE;
                rows.truncate(PAGE_SIZE);
                update(&self.shared, |s| {
                    s.picker_page = Some(Page {
                        more,
                        request,
                        parent,
                        offset,
                        entries: std::sync::Arc::new(rows),
                        error,
                    })
                });
            }
            Command::Change(change) => {
                if let Err(e) = self.change(change) {
                    self.inspection = None;
                    self.reviewed = None;
                    self.clear_review_many();
                    self.notice(e.to_string());
                }
            }
        }
    }
    fn health(&mut self) {
        if self.health.elapsed() >= Duration::from_secs(2) {
            self.health = Instant::now();
            // Opening from a cached tree can precede the first server updates.
            if let Some((request, parent, offset)) = self.current
                && self.remote.generation() != self.page_generation
            {
                self.page(request, parent, offset);
            }
            let connection = match self.remote.connection() {
                Ok(0) => Connection::Ready,
                Ok(2) => Connection::Expired,
                _ => Connection::Waiting,
            };
            if snapshot(&self.shared).connection != connection {
                update(&self.shared, |s| s.connection = connection);
            }
        }
        if let Ok(account) = self.remote.account() {
            let storage = if account.ready && account.total > 0 {
                Some(Storage {
                    used: account.used,
                    total: account.total,
                })
            } else {
                None
            };
            let before = snapshot(&self.shared);
            if before.storage != storage || before.storage_pending != account.pending {
                update(&self.shared, |s| {
                    s.storage = storage;
                    s.storage_pending = account.pending;
                });
            }
        }
        if self.quota_refresh.elapsed() >= Duration::from_secs(60) {
            let _ = self.remote.refresh_account();
            self.quota_refresh = Instant::now();
        }
    }
    fn prepare(&mut self, job: &mut Job, work: Work) -> Result<()> {
        if matches!(
            &work,
            Work::Download { depth: 0, .. } | Work::Upload { depth: 0, .. }
        ) {
            job.view.current_item = Some(job.view.current_item.map_or(0, |index| index + 1));
        }
        match work {
            Work::Download {
                entry,
                destination,
                overwrite,
                depth,
            } => {
                ensure!(depth <= 256, "Folder nesting is too deep");
                let current = self.remote.info(entry.id)?;
                check_download_source(&entry, &current)?;
                job.file(entry.name.clone());
                if let Some(identity) = transfer::existing(&destination, &entry.name)? {
                    ensure!(
                        !job.published.contains(&identity),
                        "Two cloud items map to the same local name"
                    );
                    ensure!(entry.folder || overwrite, "File already exists");
                }
                if entry.folder {
                    let (folder, created) = job.boundary.folder(&destination, &entry.name)?;
                    if created {
                        job.view.folders += 1;
                    }
                    use std::os::unix::fs::MetadataExt;
                    let m = folder.metadata()?;
                    job.published.insert((m.dev(), m.ino()));
                    job.work.push(Work::DownloadChildren {
                        id: entry.id,
                        cursor: FolderCursor::default(),
                        destination: folder,
                        overwrite,
                        depth: depth + 1,
                    });
                } else {
                    let scratch = self.remote.scratch()?;
                    self.remote
                        .download(entry.id, &scratch.path().join("data"))?;
                    job.active = Some(Active::Download {
                        scratch,
                        destination,
                        entry,
                        overwrite,
                    });
                    job.view.state = JobState::Transferring;
                }
            }
            Work::DownloadChildren {
                id,
                mut cursor,
                destination,
                overwrite,
                depth,
            } => {
                let generation = self.remote.generation();
                if cursor.offset != 0 && cursor.generation != generation {
                    cursor.replay = Some((0, DefaultHasher::new()));
                    cursor.generation = generation;
                }
                if let Some((offset, hash)) = cursor.replay.as_mut() {
                    let rows = self.remote.list(id, *offset, CHILD_PAGE)?;
                    ensure!(
                        rows.len() == CHILD_PAGE,
                        "Folder contents changed. Select the folder again."
                    );
                    hash_children(hash, &rows);
                    *offset += rows.len();
                    if *offset == cursor.offset {
                        ensure!(
                            hash.finish() == cursor.prefix.finish(),
                            "Folder contents changed. Select the folder again."
                        );
                        cursor.replay = None;
                        cursor.generation = generation;
                    }
                    job.work.push(Work::DownloadChildren {
                        id,
                        cursor,
                        destination,
                        overwrite,
                        depth,
                    });
                    return Ok(());
                }
                let rows = self.remote.list(id, cursor.offset, CHILD_PAGE)?;
                if self.remote.generation() != generation {
                    job.work.push(Work::DownloadChildren {
                        id,
                        cursor,
                        destination,
                        overwrite,
                        depth,
                    });
                    return Ok(());
                }
                if rows.len() == CHILD_PAGE {
                    hash_children(&mut cursor.prefix, &rows);
                    cursor.offset += rows.len();
                    cursor.generation = generation;
                    job.work.push(Work::DownloadChildren {
                        id,
                        cursor,
                        destination: destination.clone(),
                        overwrite,
                        depth,
                    });
                }
                for entry in rows.into_iter().rev() {
                    job.work.push(Work::Download {
                        entry,
                        destination: destination.clone(),
                        overwrite,
                        depth,
                    });
                }
            }
            Work::Upload {
                source,
                parent,
                depth,
            } => {
                ensure!(depth <= 256, "Folder nesting is too deep");
                let target = self.remote.upload_location(parent)?;
                let path = job
                    .upload_targets
                    .get(&parent)
                    .ok_or_else(|| anyhow::anyhow!("Upload destination unavailable"))?;
                ensure!(
                    target == *path,
                    "Upload destination changed. Choose it again."
                );
                let name = source.name.clone();
                job.file(name.clone());
                let existing = self.remote.find(parent, &name)?;
                if source.meta.is_dir() {
                    if let Some(entry) = existing {
                        ensure!(entry.folder, "Destination conflicts with a file");
                        let child = self.remote.upload_location(entry.id)?;
                        let expected = format!("{}/{}", target.trim_end_matches('/'), name);
                        ensure!(child == expected, "Choose a private destination");
                        job.upload_targets.insert(entry.id, expected);
                        job.work.push(Work::UploadChildren {
                            entries: rustix::fs::Dir::read_from(&source.file)?,
                            source,
                            parent: entry.id,
                            depth: depth + 1,
                        });
                    } else {
                        self.remote.mkdir(parent, &name)?;
                        let expected = format!("{}/{}", target.trim_end_matches('/'), name);
                        job.active = Some(Active::Mkdir {
                            source,
                            depth,
                            expected,
                        });
                    }
                } else {
                    ensure!(existing.is_none(), "Remote file already exists");
                    let scratch = self.remote.scratch()?;
                    job.view.state = JobState::Preparing;
                    job.local = Some(LocalTask::start(move |cancel| {
                        source.snapshot(&scratch, cancel)?;
                        Ok(Prepared::Upload {
                            scratch,
                            parent,
                            name,
                            target,
                        })
                    })?);
                }
            }
            Work::UploadChildren {
                source,
                mut entries,
                parent,
                depth,
            } => {
                for next in entries.by_ref() {
                    let next = next?;
                    let name = next.file_name().to_str()?;
                    if name == "." || name == ".." {
                        continue;
                    }
                    let child = job.boundary.child(&source.file, name)?;
                    job.work.push(Work::UploadChildren {
                        source,
                        entries,
                        parent,
                        depth,
                    });
                    job.work.push(Work::Upload {
                        source: child,
                        parent,
                        depth,
                    });
                    break;
                }
            }
        }
        Ok(())
    }
    fn advance(&mut self, job: &mut Job) -> Result<bool> {
        if let Some(task) = &job.local {
            if !task.finished() {
                return Ok(false);
            }
            let result = job.local.take().unwrap().join();
            if job.cancelled {
                return Ok(true);
            }
            match result? {
                Prepared::Upload {
                    scratch,
                    parent,
                    name,
                    target,
                } => {
                    if self.interrupt.is_some_and(|c| c.load(Ordering::Relaxed)) {
                        return Ok(false);
                    }
                    self.remote
                        .upload(&scratch.path().join("data"), parent, &name, &target)?;
                    job.active = Some(Active::Upload { _snapshot: scratch });
                    job.activity = Instant::now();
                    job.view.state = JobState::Transferring;
                    return Ok(false);
                }
                Prepared::Download {
                    mut file,
                    entry,
                    overwrite,
                } => {
                    check_download_source(&entry, &self.remote.info(entry.id)?)?;
                    if self.interrupt.is_some_and(|c| c.load(Ordering::Relaxed)) {
                        return Ok(false);
                    }
                    match file.publish(&entry.name, overwrite) {
                        Ok(id) => {
                            job.published.insert(id);
                            job.view.files += 1;
                        }
                        Err(e)
                            if e.downcast_ref::<rustix::io::Errno>()
                                == Some(&rustix::io::Errno::XDEV) =>
                        {
                            job.local = Some(LocalTask::start(move |cancel| {
                                Ok(Prepared::Download {
                                    file: file.copy_to_destination(entry.size, cancel)?,
                                    entry,
                                    overwrite,
                                })
                            })?);
                            return Ok(false);
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        if job.active.is_some() {
            let event = self.remote.event()?;
            if event.bytes != job.last_bytes {
                job.activity = Instant::now();
                job.last_bytes = event.bytes;
            }
            job.view.speed = job.rate.observe(
                job.completed_bytes.saturating_add(event.bytes),
                Instant::now(),
            );
            job.view.bytes = event.bytes;
            job.view.total = event.total;
            if !event.done
                && job.cancelled
                && !job.abort_sent
                && !matches!(job.active, Some(Active::Mkdir { .. }))
            {
                self.remote.cancel(false)?;
                job.abort_sent = true;
            }
            if !event.done {
                if !job.cancelled {
                    job.view.state = if matches!(job.active, Some(Active::Mkdir { .. })) {
                        JobState::Preparing
                    } else if event.retry != 0 {
                        JobState::Waiting
                    } else if event.total > 0 && event.bytes >= event.total {
                        JobState::Finishing
                    } else {
                        JobState::Transferring
                    };
                }
                ensure!(
                    job.activity.elapsed() < Duration::from_secs(300),
                    "Transfer stopped responding; try again"
                );
                return Ok(false);
            }
            let active = job.active.take().unwrap();
            if job.cancelled
                && (matches!(active, Active::Download { .. })
                    || (event.code == -13 && self.remote.errors().is_empty()))
            {
                return Ok(true);
            }
            if event.code != 0 {
                let detail = self.remote.errors();
                bail!(
                    "{}",
                    if detail.is_empty() {
                        transfer_error(event.code)
                    } else {
                        detail
                    }
                );
            }
            match active {
                Active::Download {
                    scratch,
                    destination,
                    entry,
                    overwrite,
                } => {
                    check_download_source(&entry, &self.remote.info(entry.id)?)?;
                    job.view.state = JobState::Finishing;
                    job.local = Some(LocalTask::start(move |cancel| {
                        let file = transfer::PreparedDownload::prepare(
                            scratch,
                            destination,
                            entry.size,
                            cancel,
                        )?;
                        Ok(Prepared::Download {
                            file,
                            entry,
                            overwrite,
                        })
                    })?);
                }
                Active::Upload { .. } => {
                    job.view.files += 1;
                    if job.work.is_empty() {
                        job.cancelled = false;
                    }
                }
                Active::Mkdir {
                    source,
                    depth,
                    expected,
                } => {
                    job.view.folders += 1;
                    let parent = self.remote.result_node()?;
                    let location = self.remote.upload_location(parent)?;
                    ensure!(
                        location == expected,
                        "New folder changed. Review its location."
                    );
                    job.upload_targets.insert(parent, expected);
                    job.work.push(Work::UploadChildren {
                        entries: rustix::fs::Dir::read_from(&source.file)?,
                        source,
                        parent,
                        depth: depth + 1,
                    });
                }
            }
            job.completed_bytes = job.completed_bytes.saturating_add(event.bytes);
            if job.local.is_some() {
                return Ok(false);
            }
            job.view.state = JobState::Preparing;
            job.view.bytes = 0;
            job.view.total = 0;
            job.view.speed = 0.0;
        }
        if job.cancelled {
            return Ok(true);
        }
        // Limit preparation work per tick so large trees cannot starve browsing.
        if job.local.is_some() {
            return Ok(false);
        }
        for _ in 0..8 {
            let Some(work) = job.work.pop() else {
                return Ok(true);
            };
            self.prepare(job, work)?;
            if job.active.is_some() || job.local.is_some() {
                break;
            }
        }
        Ok(false)
    }
    pub fn tick(&mut self) {
        if self.interactive {
            self.health();
        }
        self.preview_tick();
        self.management_tick();
        if self.changing || self.closing {
            return;
        }
        if self.active.is_none() {
            self.active = self.queued.pop_front();
            if let Some(job) = self.active.as_mut() {
                job.view.state = JobState::Preparing;
            }
        }
        let Some(mut job) = self.active.take() else {
            return;
        };
        let previous = job.view.state;
        let result = self.advance(&mut job);
        match result {
            Ok(true) => {
                if job.cancelled {
                    update(&self.shared, |s| s.jobs.retain(|j| j.id != job.view.id));
                } else {
                    job.view.state = JobState::Complete;
                    self.publish(&job.view);
                }
                if job.view.direction == Direction::Upload {
                    let _ = self.remote.refresh_account();
                    if let Some((r, p, o)) = self.current {
                        self.page(r, p, o);
                    }
                }
            }
            Err(error) => {
                if job.active.is_some() {
                    if let Err(e) = self.quiesce(false) {
                        self.closing = true;
                        self.fatal = Some(e.to_string());
                    }
                    job.active = None;
                }
                job.view.state = JobState::Failed;
                job.view.error = Some(safe(&error.to_string()));
                self.publish(&job.view);
            }
            Ok(false) => {
                if previous != job.view.state
                    || job.last_published.elapsed() >= Duration::from_millis(200)
                {
                    self.publish(&job.view);
                    job.last_published = Instant::now();
                }
                self.active = Some(job);
            }
        }
    }
    fn quiesce(&mut self, revoke: bool) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let cancelled = self.remote.cancel(revoke);
        if cancelled.is_ok() {
            while Instant::now() < deadline {
                match self.remote.event() {
                    Ok(e) if e.done => return Ok(()),
                    Ok(_) => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => break,
                }
            }
        }
        // The SDK must release its files before Job drops private staging.
        self.remote.stop_sdk();
        bail!(
            "MEGA did not confirm cancellation. Check the destination before retrying. A temporary login may still be active."
        )
    }
    fn shutdown(&mut self, revoke: bool) -> Result<()> {
        if let Some(pending) = self.preview.take() {
            pending.cancel.store(true, Ordering::Relaxed);
            let _ = self.remote.cancel_preview();
        }
        if self.changing || self.active.as_ref().is_some_and(|j| j.active.is_some()) {
            self.quiesce(revoke)?;
        }
        self.active = None;
        self.queued.clear();
        if let Some(message) = self.fatal.take() {
            bail!(message);
        }
        Ok(())
    }
}
pub(super) fn cli(remote: &mut Backend, command: Command) -> Result<()> {
    use crate::operation::{Stage, Status};
    let cancel = remote.cancel.clone();
    let remembered = remote.remembered;
    let stage = if matches!(command, Command::Upload { .. }) {
        Stage::Uploading
    } else {
        Stage::Downloading
    };
    let shared = Arc::new(std::sync::Mutex::new(Snapshot::default()));
    let mut engine = Engine::build(remote, shared.clone(), false);
    engine.interrupt = Some(&cancel);
    engine.enqueue(command)?;
    let mut status = Status::new(stage, true);
    loop {
        if cancel.load(Ordering::Relaxed) {
            engine.shutdown(!remembered)?;
            return Err(input::Cancelled.into());
        }
        engine.tick();
        if let Some(message) = engine.fatal.take() {
            bail!(message);
        }
        let jobs = snapshot(&shared).jobs;
        let job = &jobs[0];
        status.show(
            &ffi::Event {
                done: job.state.terminal(),
                code: 0,
                bytes: job.bytes,
                total: job.total,
                retry: 0,
            },
            false,
            Instant::now(),
        );
        match job.state {
            JobState::Complete => return Ok(()),
            JobState::Failed => bail!(
                "{} · {} · {}",
                safe(&job.file),
                job.completed(),
                job.error.as_deref().unwrap_or("Transfer failed")
            ),
            _ => (),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
pub fn run(
    remote: &mut impl Remote,
    shared: Shared,
    commands: Receiver<Command>,
    cancel: &AtomicBool,
    revoke: bool,
) -> Result<()> {
    let mut engine = Engine::new(remote, shared);
    engine.interrupt = Some(cancel);
    while !engine.closing && !cancel.load(Ordering::Relaxed) && !snapshot(&engine.shared).closed {
        match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(command) => engine.command(command),
            Err(RecvTimeoutError::Timeout) => (),
            Err(RecvTimeoutError::Disconnected) => break,
        }
        for _ in 0..7 {
            match commands.try_recv() {
                Ok(command) => engine.command(command),
                Err(_) => break,
            }
        }
        if !engine.closing && !cancel.load(Ordering::Relaxed) && !snapshot(&engine.shared).closed {
            engine.tick();
        }
    }
    engine.shutdown(revoke)
}
fn transfer_error(code: i32) -> String {
    match code {
        -9 => "Remote item no longer exists".into(),
        -11 => "Permission denied".into(),
        -15 => "Session expired; sign in again".into(),
        -17 => "MEGA transfer allowance exhausted".into(),
        -24 => "MEGA storage is full".into(),
        -20 => "Cannot write the download; check free disk space".into(),
        -21 => "Cannot read the upload source".into(),
        _ => format!("Transfer failed ({code})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct Fake {
        preview_data: Vec<u8>,
        preview_pending: bool,
        preview_started: usize,
        preview_cancelled: bool,
        complete: bool,
        cancelled: bool,
        uploads: Vec<String>,
        auto_upload: bool,
        quota_requests: usize,
        epoch: u64,
        restricted: bool,
        shared_child: bool,
        linked_child: bool,
        collision: bool,
        changed_name: bool,
        in_bin: bool,
        changes: Vec<(u8, u64, u64, String)>,
        stats_override: Option<(i64, i64)>,
        tree: Option<std::collections::BTreeMap<u64, Vec<Entry>>>,
        connection_code: i32,
    }
    fn entry(id: u64) -> Entry {
        Entry {
            id,
            name: if id == 3 {
                "folder".into()
            } else {
                "test.txt".into()
            },
            size: 7,
            folder: id == 3,
            modified: 0,
            shared: false,
            exported: false,
            parent: 0,
            fingerprint: String::new(),
        }
    }
    impl Remote for Fake {
        fn stop_sdk(&mut self) {}
        fn scratch(&self) -> Result<tempfile::TempDir> {
            Ok(tempfile::tempdir()?)
        }
        fn state_path(&self) -> Option<&Path> {
            None
        }
        fn errors(&mut self) -> String {
            String::new()
        }

        fn account_label(&mut self) -> Result<String> {
            Ok(crate::preview::redact_account("tester@example.invalid"))
        }
        fn preview_file(&mut self) -> Result<std::fs::File> {
            Ok(tempfile::tempfile()?)
        }
        fn preview(&mut self, _id: u64, file: &std::fs::File, bytes: i64) -> Result<()> {
            use std::io::Write;
            self.preview_started += 1;
            file.try_clone()?
                .write_all(&self.preview_data[..bytes as usize])?;
            Ok(())
        }
        fn preview_event(&mut self) -> Result<ffi::Event> {
            Ok(ffi::Event {
                done: !self.preview_pending,
                code: 0,
                bytes: self.preview_data.len() as i64,
                total: self.preview_data.len() as i64,
                retry: 0,
            })
        }
        fn cancel_preview(&mut self) -> Result<()> {
            self.preview_cancelled = true;
            self.preview_pending = false;
            Ok(())
        }
        fn change_many(
            &mut self,
            kind: u8,
            sources: &[u64],
            parents: &[u64],
            names: &[String],
            epoch: u64,
        ) -> Result<()> {
            for i in 0..sources.len() {
                self.change(kind, sources[i], parents[i], &names[i], epoch)?;
            }
            Ok(())
        }
        fn upload_location(&mut self, id: u64) -> Result<String> {
            let m = self.metadata(id)?;
            ensure!(
                !m.restricted && !m.in_rubbish,
                "Choose a private destination"
            );
            Ok(m.path)
        }
        fn metadata(&mut self, id: u64) -> Result<ffi::Metadata> {
            Ok(ffi::Metadata {
                id,
                parent: if self.in_bin { 9 } else { 1 },
                path: format!("/{}", entry(id).name),
                created: 1,
                access: 3,
                in_rubbish: self.in_bin,
                restore: 1,
                generation: self.epoch,
                restricted: self.restricted,
                favourite: false,
                direct_link: crate::bridge::ffi::LinkState::None,
            })
        }
        fn rubbish(&mut self) -> Result<u64> {
            Ok(9)
        }
        fn generation(&mut self) -> u64 {
            self.epoch
        }
        fn start_stats(&mut self, _id: u64) -> Result<()> {
            Ok(())
        }
        fn stats(&mut self) -> Result<ffi::FolderStats> {
            let (files, folders) = self.stats_override.unwrap_or((1, 0));
            Ok(ffi::FolderStats {
                pending: false,
                ready: true,
                code: 0,
                files,
                folders,
                bytes: 7,
                versions: 0,
                version_bytes: 0,
            })
        }
        fn folders(&mut self, _id: u64, _offset: usize) -> Result<Vec<Entry>> {
            Ok(vec![])
        }
        fn change(
            &mut self,
            kind: u8,
            source: u64,
            parent: u64,
            name: &str,
            epoch: u64,
        ) -> Result<()> {
            ensure!(epoch == self.epoch, "Stale epoch");
            self.changes.push((kind, source, parent, name.into()));
            self.complete = true;
            Ok(())
        }
        fn list(&mut self, id: u64, offset: usize, limit: usize) -> Result<Vec<Entry>> {
            if let Some(tree) = &self.tree {
                return Ok(tree
                    .get(&id)
                    .map(|rows| rows.iter().skip(offset).take(limit).cloned().collect())
                    .unwrap_or_default());
            }
            Ok(if offset == 0 && limit > 0 {
                let mut row = entry(if id == 1 { 2 } else { 4 });
                row.shared = self.shared_child;
                row.exported = self.linked_child;
                vec![row]
            } else {
                vec![]
            })
        }
        fn info(&mut self, id: u64) -> Result<Entry> {
            let mut row = entry(id);
            if self.changed_name {
                row.name = "changed".into();
            }
            Ok(row)
        }
        fn find(&mut self, _parent: u64, _name: &str) -> Result<Option<Entry>> {
            Ok(self.collision.then(|| entry(99)))
        }
        fn mkdir(&mut self, _parent: u64, _name: &str) -> Result<()> {
            self.complete = true;
            Ok(())
        }
        fn result_node(&mut self) -> Result<u64> {
            Ok(3)
        }
        fn download(&mut self, _id: u64, path: &Path) -> Result<()> {
            fs::write(path, b"payload")?;
            self.complete = false;
            self.cancelled = false;
            Ok(())
        }
        fn upload(
            &mut self,
            _path: &Path,
            _parent: u64,
            name: &str,
            _expected: &str,
        ) -> Result<()> {
            self.uploads.push(name.into());
            self.complete = self.auto_upload;
            self.cancelled = false;
            Ok(())
        }
        fn event(&mut self) -> Result<ffi::Event> {
            Ok(ffi::Event {
                done: self.complete || self.cancelled,
                code: if self.cancelled { -13 } else { 0 },
                bytes: if self.complete { 7 } else { 3 },
                total: 7,
                retry: 0,
            })
        }
        fn cancel(&mut self, _revoke: bool) -> Result<()> {
            self.cancelled = true;
            Ok(())
        }
        fn refresh_account(&mut self) -> Result<()> {
            self.quota_requests += 1;
            Ok(())
        }
        fn account(&mut self) -> Result<ffi::Account> {
            Ok(ffi::Account {
                ready: true,
                pending: false,
                code: 0,
                used: 1024,
                total: 4096,
            })
        }
        fn connection(&mut self) -> Result<i32> {
            Ok(self.connection_code)
        }
    }

    #[test]
    fn open_page_picks_up_remote_changes_once_without_navigation() {
        let shared = shared();
        let mut remote = Fake::default();
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine.command(Command::List {
            request: 11,
            parent: 1,
            offset: 0,
        });
        let first = snapshot(&shared).page.unwrap();
        engine.remote.tree = Some([(1, vec![entry(9)])].into());
        engine.remote.epoch += 1;
        engine.health = Instant::now() - Duration::from_secs(3);
        engine.tick();
        let next = snapshot(&shared).page.unwrap();
        assert_eq!((next.request, next.parent, next.offset), (11, 1, 0));
        assert_eq!(next.entries[0].id, 9);
        assert!(!Arc::ptr_eq(&first.entries, &next.entries));
        engine.health = Instant::now() - Duration::from_secs(3);
        engine.tick();
        assert!(Arc::ptr_eq(
            &next.entries,
            &snapshot(&shared).page.unwrap().entries
        ));
    }
    #[test]
    fn changed_tree_reports_partial_and_keeps_only_completed_files() {
        let shared = shared();
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let mut remote = Fake::default();
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine
            .enqueue(Command::DownloadMany {
                entries: vec![entry(2), entry(3)],
                destination: base.clone(),
                overwrite: false,
            })
            .unwrap();
        tick_with_disk(&mut engine);
        engine.remote.complete = true;
        tick_with_disk(&mut engine);
        engine.remote.epoch += 1;
        engine.remote.changed_name = true;
        engine.remote.complete = true;
        tick_with_disk(&mut engine);
        let state = snapshot(&shared);
        assert_eq!(state.jobs[0].state, JobState::Failed);
        assert_eq!(state.jobs[0].files, 1);
        assert_eq!(fs::read(base.join("test.txt")).unwrap(), b"payload");
        assert!(!base.join("folder/test.txt").exists());
    }
    #[test]
    fn release_queued_download_refuses_renamed_source() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = shared();
        let mut remote = Fake::default();
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine
            .enqueue(Command::Download {
                entry: entry(2),
                destination: fs::canonicalize(tmp.path()).unwrap(),
                overwrite: true,
            })
            .unwrap();
        engine.remote.changed_name = true;
        engine.tick();
        assert_eq!(snapshot(&shared).jobs[0].state, JobState::Failed);
    }
    #[test]
    fn release_completed_upload_counts_even_when_cancel_arrived() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("ready.txt");
        fs::write(&src, b"payload").unwrap();
        let shared = shared();
        let mut remote = Fake {
            auto_upload: true,
            ..Fake::default()
        };
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine
            .enqueue(Command::Upload {
                sources: vec![fs::canonicalize(src).unwrap()],
                parent: 1,
                destination: "/".into(),
            })
            .unwrap();
        tick_with_disk(&mut engine);
        engine.command(Command::Cancel(vec![1]));
        tick_with_disk(&mut engine);
        assert_eq!(snapshot(&shared).jobs[0].files, 1);
        assert_eq!(snapshot(&shared).jobs[0].state, JobState::Complete);
    }
    fn tick_with_disk(engine: &mut Engine<'_, Fake>) {
        engine.tick();
        let deadline = Instant::now() + Duration::from_secs(5);
        while engine.active.as_ref().is_some_and(|j| j.local.is_some()) {
            assert!(Instant::now() < deadline, "local transfer stalled");
            std::thread::sleep(Duration::from_millis(1));
            engine.tick();
        }
    }
    #[test]
    fn unrelated_updates_leave_queued_and_finishing_downloads_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = shared();
        let mut remote = Fake::default();
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine
            .enqueue(Command::Download {
                entry: entry(2),
                destination: tmp.path().canonicalize().unwrap(),
                overwrite: false,
            })
            .unwrap();
        engine.remote.epoch += 1;
        engine.tick();
        engine.remote.epoch += 1;
        engine.remote.complete = true;
        tick_with_disk(&mut engine);
        assert_eq!(snapshot(&shared).jobs[0].state, JobState::Complete);
        assert_eq!(fs::read(tmp.path().join("test.txt")).unwrap(), b"payload");
    }

    #[test]
    fn folder_pagination_checks_only_the_listed_prefix_after_a_change() {
        let tmp = tempfile::tempdir().unwrap();
        let mut remote = Fake::default();
        let rows = (10..10 + CHILD_PAGE as u64 + 1)
            .map(|id| {
                let mut e = entry(id);
                e.name = format!("file-{id}");
                e
            })
            .collect::<Vec<_>>();
        remote.tree = Some([(3, rows.clone())].into());
        let mut engine = Engine::new(&mut remote, shared());
        engine
            .enqueue(Command::Download {
                entry: entry(3),
                destination: tmp.path().canonicalize().unwrap(),
                overwrite: false,
            })
            .unwrap();
        let mut job = engine.queued.pop_front().unwrap();
        for changed_folder in [false, true] {
            let dir = Arc::new(File::open(tmp.path()).unwrap());
            engine
                .prepare(
                    &mut job,
                    Work::DownloadChildren {
                        id: 3,
                        cursor: FolderCursor::default(),
                        destination: dir,
                        overwrite: false,
                        depth: 1,
                    },
                )
                .unwrap();
            let next = job
                .work
                .iter()
                .position(|work| matches!(work, Work::DownloadChildren { .. }))
                .unwrap();
            let next = job.work.remove(next);
            job.work.clear();
            engine.remote.epoch += 1;
            if changed_folder {
                engine
                    .remote
                    .tree
                    .as_mut()
                    .unwrap()
                    .get_mut(&3)
                    .unwrap()
                    .remove(0);
            }
            let replay = engine.prepare(&mut job, next);
            if changed_folder {
                assert!(replay.is_err());
            } else {
                replay.unwrap();
                let next = job.work.pop().unwrap();
                engine.prepare(&mut job, next).unwrap();
                assert!(
                    matches!(&job.work[..], [Work::Download { entry, .. }] if entry.id == rows.last().unwrap().id)
                );
                job.work.clear();
            }
        }
    }

    #[test]
    fn cancellation_after_staging_keeps_the_destination() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("test.txt"), b"original").unwrap();
        let shared = shared();
        let mut remote = Fake::default();
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine
            .enqueue(Command::Download {
                entry: entry(2),
                destination: tmp.path().canonicalize().unwrap(),
                overwrite: true,
            })
            .unwrap();
        engine.tick();
        engine.remote.complete = true;
        engine.tick();
        assert!(engine.active.as_ref().unwrap().local.is_some());
        engine.command(Command::List {
            request: 7,
            parent: 1,
            offset: 0,
        });
        assert_eq!(snapshot(&shared).page.unwrap().request, 7);
        engine.command(Command::Cancel(vec![1]));
        tick_with_disk(&mut engine);
        assert!(snapshot(&shared).jobs.is_empty());
        assert_eq!(fs::read(tmp.path().join("test.txt")).unwrap(), b"original");
    }
    fn shared() -> Shared {
        Arc::new(Mutex::new(Snapshot::default()))
    }
    #[test]
    fn browsing_and_queueing_remain_available_during_a_pending_download() {
        let tmp = tempfile::tempdir().unwrap();
        let destination = fs::canonicalize(tmp.path()).unwrap();
        let shared = shared();
        let mut remote = Fake::default();
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine.command(Command::Download {
            entry: entry(2),
            destination,
            overwrite: false,
        });
        engine.tick();
        assert!(engine.active.as_ref().unwrap().active.is_some());
        engine.command(Command::List {
            request: 9,
            parent: 3,
            offset: 0,
        });
        let state = snapshot(&shared);
        assert_eq!(state.page.unwrap().request, 9);
        assert!(engine.active.is_some());
        assert_eq!(state.storage.unwrap().used, 1024);
        fs::write(tmp.path().join("next.txt"), b"next").unwrap();
        engine.command(Command::Upload {
            sources: vec![fs::canonicalize(tmp.path().join("next.txt")).unwrap()],
            parent: 3,
            destination: "/folder".into(),
        });
        assert_eq!(engine.queued.len(), 1);
        engine.shutdown(false).unwrap();
    }
    #[test]
    fn download_is_published_only_after_completion_and_staging_is_cleaned() {
        let tmp = tempfile::tempdir().unwrap();
        let destination = fs::canonicalize(tmp.path()).unwrap();
        let shared = shared();
        let mut remote = Fake::default();
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine.command(Command::Download {
            entry: entry(2),
            destination: destination.clone(),
            overwrite: false,
        });
        tick_with_disk(&mut engine);
        assert!(!destination.join("test.txt").exists());
        engine.remote.complete = true;
        tick_with_disk(&mut engine);
        assert_eq!(fs::read(destination.join("test.txt")).unwrap(), b"payload");
        assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
        assert_eq!(snapshot(&shared).jobs[0].state, JobState::Complete);
    }
    #[test]
    fn cancelled_download_never_publishes_and_browser_keeps_working() {
        let tmp = tempfile::tempdir().unwrap();
        let destination = fs::canonicalize(tmp.path()).unwrap();
        let shared = shared();
        let mut remote = Fake::default();
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine.command(Command::Download {
            entry: entry(2),
            destination: destination.clone(),
            overwrite: false,
        });
        engine.tick();
        engine.command(Command::Cancel(vec![1]));
        engine.tick();
        // The cancellation callback arrives on the next poll. Until then the
        // SDK must retain its private staging directory.
        assert!(!destination.join("test.txt").exists());
        engine.tick();
        assert!(!destination.join("test.txt").exists());
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
        assert!(snapshot(&shared).jobs.is_empty());
        engine.command(Command::List {
            request: 4,
            parent: 1,
            offset: 0,
        });
        assert_eq!(snapshot(&shared).page.unwrap().request, 4);
    }
    #[test]
    fn multi_upload_sends_each_selected_file_and_refreshes_storage() {
        let tmp = tempfile::tempdir().unwrap();
        let sources = ["a.txt", "b.txt"].map(|name| {
            let p = tmp.path().join(name);
            fs::write(&p, b"payload").unwrap();
            fs::canonicalize(p).unwrap()
        });
        let shared = shared();
        let mut remote = Fake {
            auto_upload: true,
            ..Fake::default()
        };
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine.command(Command::Upload {
            sources: sources.to_vec(),
            parent: 1,
            destination: "/".into(),
        });
        for _ in 0..5 {
            tick_with_disk(&mut engine);
        }
        assert_eq!(engine.remote.uploads, ["a.txt", "b.txt"]);
        let job = &snapshot(&shared).jobs[0];
        assert_eq!(job.current_item, Some(1));
        assert_eq!(
            job.items
                .iter()
                .map(|i| (i.name.as_str(), i.size))
                .collect::<Vec<_>>(),
            [("a.txt", Some(7)), ("b.txt", Some(7))]
        );
        assert!(engine.remote.quota_requests >= 2);
        let state = snapshot(&shared);
        assert_eq!(state.jobs[0].files, 2);
        assert_eq!(state.jobs[0].state, JobState::Complete);
    }
    #[test]
    fn cancelling_a_batch_leaves_later_jobs_queued_and_existing_files_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let destination = tmp.path().canonicalize().unwrap();
        let shared = shared();
        let mut remote = Fake::default();
        let mut engine = Engine::new(&mut remote, shared.clone());
        for _ in 0..3 {
            engine
                .enqueue(Command::Download {
                    entry: entry(2),
                    destination: destination.clone(),
                    overwrite: false,
                })
                .unwrap();
        }
        tick_with_disk(&mut engine);
        engine.command(Command::Cancel(vec![1, 2]));
        assert_eq!(engine.queued.len(), 1);
        assert_eq!(engine.queued[0].view.id, 3);
        tick_with_disk(&mut engine);
        tick_with_disk(&mut engine);
        let remaining = snapshot(&shared).jobs;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, 3);
        assert_eq!(remaining[0].state, JobState::Queued);
        assert!(!destination.join("test.txt").exists());
        tick_with_disk(&mut engine);
        engine.remote.complete = true;
        tick_with_disk(&mut engine);
        assert_eq!(fs::read(destination.join("test.txt")).unwrap(), b"payload");
    }

    #[test]
    fn queue_capacity_is_enforced_without_dropping_existing_jobs() {
        let shared = shared();
        let mut remote = Fake::default();
        let mut engine = Engine::new(&mut remote, shared.clone());
        for _ in 0..MAX_JOBS + 1 {
            engine.command(Command::Upload {
                sources: vec![PathBuf::from("Cargo.toml")],
                parent: 1,
                destination: "/".into(),
            });
        }
        assert_eq!(engine.queued.len(), MAX_JOBS);
        assert!(snapshot(&shared).notice.unwrap().1.contains("full"));
    }

    fn selection(id: u64) -> Selection {
        Selection {
            entry: entry(id),
            parent: 1,
        }
    }
    fn reviewed(engine: &mut Engine<'_, Fake>) -> Arc<Details> {
        for _ in 0..20 {
            engine.tick();
            if let Some(review) = engine.reviewed.clone() {
                return review;
            }
        }
        panic!("Review did not finish");
    }
    #[test]
    fn folder_review_scans_nested_pages_before_allowing_a_single_reversible_move() {
        let shared = shared();
        let mut nested = entry(5);
        nested.folder = true;
        let mut remote = Fake {
            tree: Some([(3, vec![nested]), (5, (10..80).map(entry).collect())].into()),
            stats_override: Some((70, 1)),
            ..Fake::default()
        };
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine.command(Command::Inspect {
            request: 7,
            selection: selection(3),
            review: true,
        });
        engine.tick();
        assert!(engine.reviewed.is_none());
        let review = reviewed(&mut engine);
        assert_eq!(review.checked, 71);
        assert_eq!(review.stats.as_ref().unwrap().files, 70);
        assert!(engine.remote.changes.is_empty());
        engine.command(Command::Change(Change::Trash { review }));
        assert_eq!(engine.remote.changes, [(4, 3, 0, String::new())]);
        assert!(snapshot(&shared).action.unwrap().contains("Rubbish Bin"));
        engine.tick();
        assert!(!engine.changing);
    }
    #[test]
    fn shared_linked_restricted_mismatched_and_oversized_folders_never_reach_mutation() {
        for remote in [
            Fake {
                shared_child: true,
                ..Fake::default()
            },
            Fake {
                linked_child: true,
                ..Fake::default()
            },
            Fake {
                restricted: true,
                ..Fake::default()
            },
            Fake {
                stats_override: Some((2, 0)),
                ..Fake::default()
            },
            Fake {
                stats_override: Some((100_001, 0)),
                ..Fake::default()
            },
            Fake {
                connection_code: 1,
                ..Fake::default()
            },
        ] {
            let mut remote = remote;
            let shared = shared();
            let mut engine = Engine::new(&mut remote, shared.clone());
            engine.command(Command::Inspect {
                request: 1,
                selection: selection(3),
                review: true,
            });
            for _ in 0..4 {
                engine.tick();
            }
            assert!(snapshot(&shared).details.unwrap().error.is_some());
            assert!(engine.reviewed.is_none());
            assert!(engine.remote.changes.is_empty());
        }
    }
    #[test]
    fn cancelled_forged_and_stale_reviews_are_rejected_at_submission() {
        for case in 0..4 {
            let mut remote = Fake::default();
            let shared = shared();
            let mut engine = Engine::new(&mut remote, shared.clone());
            engine.command(Command::Inspect {
                request: 1,
                selection: selection(3),
                review: true,
            });
            let mut review = reviewed(&mut engine);
            match case {
                0 => engine.command(Command::CancelInspect(1)),
                1 => review = Arc::new((*review).clone()),
                2 => engine.remote.epoch += 1,
                _ => engine.remote.changed_name = true,
            }
            engine.command(Command::Change(Change::Trash { review }));
            assert!(engine.remote.changes.is_empty());
            assert!(engine.reviewed.is_none());
            assert!(snapshot(&shared).notice.is_some());
        }
    }
    #[test]
    fn updates_during_and_after_review_invalidate_confirmation() {
        for finished in [false, true] {
            let mut remote = Fake::default();
            let shared = shared();
            let mut engine = Engine::new(&mut remote, shared.clone());
            engine.command(Command::Inspect {
                request: 1,
                selection: selection(3),
                review: true,
            });
            if finished {
                let _ = reviewed(&mut engine);
            }
            engine.remote.epoch += 1;
            engine.tick();
            assert!(
                snapshot(&shared)
                    .details
                    .unwrap()
                    .error
                    .unwrap()
                    .contains("Cloud changed")
            );
            assert!(engine.reviewed.is_none());
            assert!(engine.remote.changes.is_empty());
        }
    }
    #[test]
    fn mutations_refuse_collisions_and_unsafe_names_without_starting_work() {
        let mut remote = Fake {
            collision: true,
            ..Fake::default()
        };
        let shared = shared();
        let mut engine = Engine::new(&mut remote, shared.clone());
        for change in [
            Change::Rename {
                selection: selection(2),
                name: "other.txt".into(),
            },
            Change::Many {
                selections: vec![selection(2)],
                action: BatchAction::Move(8),
            },
            Change::Copy {
                selection: selection(2),
                destination: 8,
                name: "copy.txt".into(),
            },
            Change::NewFolder {
                parent: 1,
                name: "new".into(),
            },
        ] {
            engine.command(Command::Change(change));
            assert!(
                snapshot(&shared)
                    .notice
                    .as_ref()
                    .unwrap()
                    .1
                    .contains("exist")
                    || snapshot(&shared)
                        .notice
                        .as_ref()
                        .unwrap()
                        .1
                        .contains("conflicting")
            );
        }
        engine.remote.collision = false;
        for name in ["", "..", "bad/name", "escape\x1b", "bad\nname", "trail."] {
            engine.command(Command::Change(Change::NewFolder {
                parent: 1,
                name: name.into(),
            }));
        }
        assert!(engine.remote.changes.is_empty());
    }
    #[test]
    fn file_actions_and_transfer_jobs_cannot_overlap_or_wait_in_a_mutation_queue() {
        let mut remote = Fake::default();
        let shared = shared();
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine.command(Command::Inspect {
            request: 1,
            selection: selection(3),
            review: true,
        });
        let review = reviewed(&mut engine);
        engine.command(Command::Upload {
            sources: vec![PathBuf::from("Cargo.toml")],
            parent: 1,
            destination: "/".into(),
        });
        assert!(engine.queued.is_empty());
        engine.command(Command::CancelInspect(1));
        engine.command(Command::Upload {
            sources: vec![PathBuf::from("Cargo.toml")],
            parent: 1,
            destination: "/".into(),
        });
        assert_eq!(engine.queued.len(), 1);
        engine.command(Command::Change(Change::Trash { review }));
        engine.command(Command::Change(Change::NewFolder {
            parent: 1,
            name: "new".into(),
        }));
        assert!(engine.remote.changes.is_empty());
        engine.command(Command::Inspect {
            request: 2,
            selection: selection(3),
            review: true,
        });
        assert!(snapshot(&shared).details.unwrap().error.is_some());
    }
    #[test]
    fn rename_copy_move_creation_and_restore_use_server_side_requests() {
        let mut remote = Fake::default();
        let shared = shared();
        let mut engine = Engine::new(&mut remote, shared);
        for change in [
            Change::Rename {
                selection: selection(2),
                name: "renamed.txt".into(),
            },
            Change::Many {
                selections: vec![selection(2)],
                action: BatchAction::Move(8),
            },
            Change::Copy {
                selection: selection(2),
                destination: 8,
                name: "copy.txt".into(),
            },
            Change::NewFolder {
                parent: 1,
                name: "new".into(),
            },
        ] {
            engine.command(Command::Change(change));
            engine.tick();
        }
        engine.remote.in_bin = true;
        let mut item = selection(2);
        item.parent = 9;
        engine.command(Command::Change(Change::Many {
            selections: vec![item],
            action: BatchAction::Restore,
        }));
        engine.tick();
        assert_eq!(
            engine
                .remote
                .changes
                .iter()
                .map(|c| c.0)
                .collect::<Vec<_>>(),
            [1, 2, 3, 5, 6]
        );
        assert_eq!(engine.remote.changes.last().unwrap().2, 1);
        assert!(engine.remote.uploads.is_empty());
    }

    #[test]
    fn multiple_private_items_share_one_review_and_one_native_batch() {
        let shared = shared();
        let mut remote = Fake::default();
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine.command(Command::InspectMany {
            request: 1,
            selections: vec![selection(3), selection(2)],
        });
        let review = reviewed(&mut engine);
        assert_eq!(review.members.len(), 2);
        assert!(engine.remote.changes.is_empty());
        engine.command(Command::Change(Change::Trash { review }));
        assert_eq!(
            engine
                .remote
                .changes
                .iter()
                .map(|c| (c.0, c.1))
                .collect::<Vec<_>>(),
            [(4, 3), (4, 2)]
        );
    }
    #[test]
    fn grouped_downloads_keep_relative_paths_and_share_one_job() {
        let shared = shared();
        let mut remote = Fake::default();
        let tmp = tempfile::tempdir().unwrap();
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine.command(Command::DownloadMany {
            entries: vec![entry(2), entry(3)],
            destination: fs::canonicalize(tmp.path()).unwrap(),
            overwrite: false,
        });
        for _ in 0..20 {
            engine.remote.complete = true;
            tick_with_disk(&mut engine);
            if snapshot(&shared)
                .jobs
                .front()
                .is_some_and(|job| job.state == JobState::Complete)
            {
                break;
            }
        }
        let state = snapshot(&shared);
        assert_eq!(state.jobs.len(), 1);
        assert_eq!(state.jobs[0].state, JobState::Complete);
        assert_eq!(state.jobs[0].files, 2);
        assert_eq!(
            std::fs::read(tmp.path().join("test.txt")).unwrap(),
            b"payload"
        );
        assert_eq!(
            std::fs::read(tmp.path().join("folder/test.txt")).unwrap(),
            b"payload"
        );
    }
    #[test]
    fn batch_collisions_or_a_failed_review_submit_no_partial_operations() {
        let shared = shared();
        let mut remote = Fake::default();
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine.command(Command::Change(Change::Many {
            selections: vec![selection(2), selection(4)],
            action: BatchAction::Copy(8),
        }));
        assert!(engine.remote.changes.is_empty());
        engine.remote.shared_child = true;
        engine.command(Command::InspectMany {
            request: 7,
            selections: vec![selection(2), selection(3)],
        });
        for _ in 0..5 {
            engine.tick();
        }
        assert!(engine.reviewed.is_none());
        assert!(snapshot(&shared).details.unwrap().error.is_some());
        assert!(engine.remote.changes.is_empty());
    }
    #[test]
    fn preview_never_fetches_implicitly_and_uses_a_separate_cancel_path() {
        let shared = shared();
        let mut remote = Fake {
            preview_data: b"hello!\n".to_vec(),
            preview_pending: true,
            ..Fake::default()
        };
        let mut engine = Engine::new(&mut remote, shared.clone());
        engine.command(Command::List {
            request: 1,
            parent: 1,
            offset: 0,
        });
        engine.tick();
        assert_eq!(engine.remote.preview_started, 0);
        let mut too_big = entry(2);
        too_big.name = "a.png".into();
        too_big.size = crate::preview::IMAGE_LIMIT + 1;
        engine.command(Command::Preview {
            request: 2,
            entry: too_big,
        });
        assert_eq!(engine.remote.preview_started, 0);
        engine.command(Command::Preview {
            request: 3,
            entry: entry(2),
        });
        engine.tick();
        assert_eq!(engine.remote.preview_started, 1);
        engine.command(Command::CancelPreview(3));
        engine.tick();
        assert!(engine.remote.preview_cancelled);
        assert!(!engine.remote.cancelled);
        assert!(!engine.closing);
        assert!(snapshot(&shared).preview.is_none());
        engine.command(Command::Preview {
            request: 4,
            entry: entry(2),
        });
        for _ in 0..100 {
            engine.tick();
            if snapshot(&shared)
                .preview
                .as_ref()
                .is_some_and(|p| p.content.is_some())
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let output = snapshot(&shared).preview.unwrap();
        assert!(output.error.is_none());
        assert!(
            matches!(output.content.as_deref(),Some(crate::preview::Content::Text{lines,..}) if lines[0]=="hello!")
        );
        engine.command(Command::CancelPreview(4));
        assert!(snapshot(&shared).preview.is_none());
    }
}
