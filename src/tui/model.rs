use crate::{bridge::ffi, state};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

pub const PAGE_SIZE: usize = 255;
pub const MAX_JOBS: usize = 16;
pub const MAX_SELECTION: usize = 128;
pub const HISTORY: usize = 4;
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u64,
    pub name: String,
    pub size: i64,
    pub folder: bool,
    pub modified: i64,
    pub shared: bool,
    pub exported: bool,
    pub parent: u64,
    pub fingerprint: String,
}
impl From<ffi::Entry> for Entry {
    fn from(e: ffi::Entry) -> Self {
        Self {
            id: e.id,
            name: e.name,
            size: e.size,
            folder: e.folder,
            modified: e.modified,
            shared: e.shared,
            exported: e.exported,
            parent: e.parent,
            fingerprint: e.fingerprint,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Download,
    Upload,
}
impl Direction {
    pub fn verb(self) -> &'static str {
        match self {
            Self::Download => "Downloading",
            Self::Upload => "Uploading",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Download => "↓",
            Self::Upload => "↑",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobState {
    Queued,
    Preparing,
    Transferring,
    Finishing,
    Waiting,
    Cancelling,
    Complete,
    Failed,
}
impl JobState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Preparing => "Preparing",
            Self::Transferring => "Transferring",
            Self::Finishing => "Finishing",
            Self::Waiting => "Waiting for MEGA",
            Self::Cancelling => "Cancelling",
            Self::Complete => "Complete",
            Self::Failed => "Failed",
        }
    }
    pub fn cancellable(self) -> bool {
        !self.terminal() && self != Self::Cancelling
    }
    pub fn terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Failed)
    }
}
#[derive(Clone, Debug)]
pub struct TransferItem {
    pub name: String,
    pub size: Option<i64>,
    pub folder: bool,
}
impl From<&Entry> for TransferItem {
    fn from(entry: &Entry) -> Self {
        Self {
            name: entry.name.clone(),
            size: (!entry.folder && entry.size >= 0).then_some(entry.size),
            folder: entry.folder,
        }
    }
}
#[derive(Clone, Debug)]
pub struct JobView {
    pub id: u64,
    pub direction: Direction,
    pub items: Arc<[TransferItem]>,
    pub current_item: Option<usize>,
    pub name: String,
    pub file: String,
    pub destination: String,
    pub state: JobState,
    pub bytes: i64,
    pub total: i64,
    pub speed: f64,
    pub files: usize,
    pub folders: usize,
    pub error: Option<String>,
}
impl JobView {
    pub fn completed(&self) -> String {
        let files = if self.files == 1 { "file" } else { "files" };
        if self.folders > 0 {
            format!(
                "{} {} complete · {} {} created",
                self.files,
                files,
                self.folders,
                if self.folders == 1 {
                    "folder"
                } else {
                    "folders"
                }
            )
        } else {
            format!("{} {} complete", self.files, files)
        }
    }
}
#[derive(Clone)]
pub struct Page {
    pub more: bool,
    pub request: u64,
    pub parent: u64,
    pub offset: usize,
    pub entries: Arc<Vec<Entry>>,
    pub error: Option<String>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    Checking,
    Ready,
    Waiting,
    Expired,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Storage {
    pub used: i64,
    pub total: i64,
}
#[derive(Clone)]
pub struct Snapshot {
    pub account: Option<String>,
    pub preview: Option<Arc<PreviewState>>,
    pub revision: u64,
    pub page: Option<Page>,
    pub connection: Connection,
    pub storage: Option<Storage>,
    pub storage_pending: bool,
    pub jobs: VecDeque<JobView>,
    pub notice: Option<(u64, String)>,
    pub closed: bool,
    pub preview_failed: bool,
    pub rubbish: Option<u64>,
    pub details: Option<Inspection>,
    pub picker_page: Option<Page>,
    pub action: Option<String>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TransferFocus {
    pub job: u64,
    pub item: usize,
}
impl Snapshot {
    pub fn preferred_transfer(&self) -> Option<TransferFocus> {
        let job = self
            .jobs
            .iter()
            .find(|job| !job.state.terminal())
            .or_else(|| self.jobs.back())?;
        Some(TransferFocus {
            job: job.id,
            item: job.current_item.unwrap_or(0),
        })
    }
    pub fn transfer_rows(&self) -> impl Iterator<Item = (TransferFocus, &JobView, &TransferItem)> {
        self.jobs.iter().flat_map(|job| {
            job.items
                .iter()
                .enumerate()
                .map(move |(item, entry)| (TransferFocus { job: job.id, item }, job, entry))
        })
    }
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            account: None,
            preview: None,
            revision: 0,
            page: None,
            connection: Connection::Checking,
            storage: None,
            storage_pending: true,
            jobs: VecDeque::new(),
            notice: None,
            closed: false,
            preview_failed: false,
            rubbish: None,
            details: None,
            picker_page: None,
            action: None,
        }
    }
}
pub type Shared = Arc<Mutex<Snapshot>>;
pub fn update(shared: &Shared, change: impl FnOnce(&mut Snapshot)) {
    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
    change(&mut state);
    state.revision = state.revision.wrapping_add(1);
}
pub fn snapshot(shared: &Shared) -> Snapshot {
    shared.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub enum Command {
    Preview {
        request: u64,
        entry: Entry,
    },
    CancelPreview(u64),
    InspectMany {
        request: u64,
        selections: Vec<Selection>,
    },
    DownloadMany {
        entries: Vec<Entry>,
        destination: PathBuf,
        overwrite: bool,
    },
    List {
        request: u64,
        parent: u64,
        offset: usize,
    },
    Download {
        entry: Entry,
        destination: PathBuf,
        overwrite: bool,
    },
    Upload {
        sources: Vec<PathBuf>,
        parent: u64,
        destination: String,
    },
    Cancel(Vec<u64>),
    Refresh,
    Quit,
    Inspect {
        request: u64,
        selection: Selection,
        review: bool,
    },
    CancelInspect(u64),
    PickFolder {
        request: u64,
        parent: u64,
        offset: usize,
    },
    Change(Change),
}
#[derive(Clone)]
pub struct PreviewState {
    pub request: u64,
    pub name: String,
    pub bytes: i64,
    pub total: i64,
    pub decoding: bool,
    pub content: Option<Arc<crate::preview::Content>>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub entry: Entry,
    pub parent: u64,
}
#[derive(Clone, Debug)]
pub struct FolderStats {
    pub files: i64,
    pub folders: i64,
    pub bytes: i64,
    pub versions: i64,
    pub version_bytes: i64,
}
#[derive(Clone, Debug)]
pub struct Details {
    pub members: Vec<Arc<Details>>,
    pub selection: Selection,
    pub path: String,
    pub created: i64,
    pub access: i32,
    pub in_rubbish: bool,
    pub restore: Option<u64>,
    pub generation: u64,
    pub restricted: bool,
    pub favourite: bool,
    pub direct_link: ffi::LinkState,
    pub stats: Option<FolderStats>,
    pub review_ready: bool,
    pub checked: usize,
}
#[derive(Clone)]
pub struct Inspection {
    pub request: u64,
    pub details: Option<Arc<Details>>,
    pub error: Option<String>,
}
#[derive(Clone)]
pub enum Change {
    Many {
        selections: Vec<Selection>,
        action: BatchAction,
    },
    Rename {
        selection: Selection,
        name: String,
    },
    Copy {
        selection: Selection,
        destination: u64,
        name: String,
    },
    NewFolder {
        parent: u64,
        name: String,
    },
    Trash {
        review: Arc<Details>,
    },
}
#[derive(Clone)]
pub enum BatchAction {
    Move(u64),
    Copy(u64),
    Restore,
}
pub use crate::display::size;
pub fn percent(bytes: i64, total: i64) -> u16 {
    if total <= 0 {
        0
    } else {
        ((bytes.max(0) as u128 * 100 / total as u128).min(100)) as u16
    }
}
pub fn modified(timestamp: i64) -> String {
    if timestamp <= 0 {
        return "—".into();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let seconds = now.saturating_sub(timestamp).max(0);
    match seconds {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86399 => format!("{}h ago", seconds / 3600),
        86400..=2591999 => format!("{}d ago", seconds / 86400),
        2592000..=31535999 => format!("{}mo ago", seconds / 2592000),
        _ => format!("{}y ago", seconds / 31536000),
    }
}
pub fn kind(name: &str, folder: bool) -> (&'static str, &'static str) {
    if folder {
        return ("📁", "Folder");
    }
    match name
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "svg" | "heic" => ("▧", "Image"),
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "m4v" | "mpeg" | "mpg" | "mts" | "m2ts" => {
            ("▶", "Video")
        }
        "mp3" | "wav" | "flac" | "ogg" | "m4a" => ("♫", "Audio"),
        "zip" | "gz" | "7z" | "rar" | "tar" | "xz" | "zst" | "zstd" | "bz" | "bz2" | "tgz"
        | "txz" | "tbz" | "tbz2" | "tzst" | "lz" | "lz4" | "lzma" | "br" => ("▣", "Archive"),
        "pdf" | "txt" | "md" | "doc" | "docx" | "csv" | "xlsx" => ("▤", "Document"),
        _ => ("·", "File"),
    }
}
pub fn safe(text: &str) -> String {
    state::display(text)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_dates_are_utc_and_reject_invalid_timestamps() {
        assert_eq!(ffi::utc_date(951_827_696), "2000-02-29 12:34:56 UTC");
        assert_eq!(ffi::utc_date(253_402_300_799), "9999-12-31 23:59:59 UTC");
        for t in [-1, 0, i64::MAX] {
            assert_eq!(ffi::utc_date(t), "Unknown");
        }
    }
    #[test]
    fn recognizes_compressed_archives_and_distinguishes_folder_from_video() {
        for name in [
            "backup.tar",
            "backup.tar.zst",
            "BACKUP.TAR.ZST",
            "backup.tar.bz2",
            "backup.tgz",
            "backup.txz",
            "backup.7z",
        ] {
            assert_eq!(kind(name, false).1, "Archive", "{name}");
        }
        assert_eq!(kind("tar", false).1, "File");
        assert_ne!(kind("Movies", true).0, kind("movie.mp4", false).0);
    }
}
