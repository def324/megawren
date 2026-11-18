//! Descriptor-relative local transfer I/O. Only private snapshots reach the path-based SDK.
use crate::state;
use anyhow::{Context, Result, bail, ensure};
use rustix::fs::{self as at, AtFlags, Mode, OFlags, RenameFlags};
use std::{
    fs::{File, Metadata},
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path},
    sync::{Arc, atomic::AtomicBool},
};
use tempfile::TempDir;

type Identity = (u64, u64);
fn identity(m: &Metadata) -> Identity {
    (m.dev(), m.ino())
}

pub struct Boundary(Option<Identity>);
impl Boundary {
    pub fn new(state: Option<&Path>) -> Result<Self> {
        Ok(Self(
            state
                .map(std::fs::metadata)
                .transpose()?
                .as_ref()
                .map(identity),
        ))
    }
    fn check(&self, file: &File) -> Result<()> {
        ensure!(
            Some(identity(&file.metadata()?)) != self.0,
            "Private Megawren data excluded"
        );
        Ok(())
    }
    pub fn directory(&self, path: &Path) -> Result<Arc<File>> {
        let absolute = std::path::absolute(path)?;
        let mut dir = File::open("/")?;
        for component in absolute.components() {
            let name = match component {
                Component::RootDir | Component::CurDir => continue,
                Component::Normal(name) => name,
                Component::ParentDir => std::ffi::OsStr::new(".."),
                _ => bail!("Unsupported path"),
            };
            if name.as_encoded_bytes().starts_with(b".megawren-part-") {
                bail!("Private Megawren staging data excluded");
            }
            dir = at::openat(&dir, name, READ | OFlags::DIRECTORY, Mode::empty())
                .with_context(|| {
                    format!(
                        "Cannot open directory {}",
                        state::display(&path.display().to_string())
                    )
                })?
                .into();
            self.check(&dir)?;
        }
        Ok(Arc::new(dir))
    }
    pub fn source(&self, path: &Path) -> Result<Source> {
        let path = std::path::absolute(path)?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("Choose a named file or folder")?;
        self.child(
            self.directory(path.parent().context("Missing source directory")?)?
                .as_ref(),
            name,
        )
    }
    pub fn child(&self, parent: &File, name: &str) -> Result<Source> {
        permitted(name)?;
        let kind = at::FileType::from_raw_mode(
            at::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?.st_mode,
        );
        ensure!(
            kind.is_file() || kind.is_dir(),
            "Choose a regular file or folder"
        );
        let file: File = at::openat(parent, name, READ, Mode::empty())?.into();
        self.check(&file)?;
        let meta = file.metadata()?;
        ensure!(meta.is_dir() || meta.is_file(), "Unsupported special file");
        Ok(Source {
            name: name.into(),
            file,
            meta,
        })
    }
    pub fn folder(&self, parent: &File, name: &str) -> Result<(Arc<File>, bool)> {
        permitted(name)?;
        let created = match at::mkdirat(parent, name, Mode::RWXU) {
            Ok(()) => {
                parent.sync_all()?;
                true
            }
            Err(rustix::io::Errno::EXIST) => false,
            Err(e) => return Err(e.into()),
        };
        let child: File = at::openat(parent, name, READ | OFlags::DIRECTORY, Mode::empty())?.into();
        self.check(&child)?;
        Ok((Arc::new(child), created))
    }
}
const READ: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC)
    .union(OFlags::NONBLOCK);
fn permitted(name: &str) -> Result<()> {
    state::component(name)?;
    ensure!(
        !name.starts_with(".megawren-part-"),
        "Private Megawren staging data excluded"
    );
    Ok(())
}
pub struct Source {
    pub name: String,
    pub file: File,
    pub meta: Metadata,
}
impl Source {
    pub fn snapshot(&self, scratch: &TempDir, cancel: &AtomicBool) -> Result<()> {
        let dir = File::open(scratch.path())?;
        let file = copy_to(&self.file, &dir, "data", cancel)?;
        let after = self.file.metadata()?;
        ensure!(
            self.meta.len() == after.len()
                && self.meta.modified()? == after.modified()?
                && self.meta.ctime() == after.ctime()
                && self.meta.ctime_nsec() == after.ctime_nsec()
                && file.metadata()?.len() == self.meta.len(),
            "Upload source changed. Select it again."
        );
        file.set_modified(self.meta.modified()?)?;
        Ok(())
    }
}

fn copy_to(source: &File, directory: &File, name: &str, cancel: &AtomicBool) -> Result<File> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        let c = std::ffi::CString::new(name)?;
        // Both descriptors and the NUL-terminated name live through this call.
        if unsafe { libc::fclonefileat(source.as_raw_fd(), directory.as_raw_fd(), c.as_ptr(), 0) }
            == 0
        {
            return Ok(at::openat(directory, name, READ, Mode::empty())?.into());
        }
    }
    let file: File = at::openat(
        directory,
        name,
        OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )?
    .into();
    #[cfg(target_os = "linux")]
    if at::ioctl_ficlone(&file, source).is_ok() {
        return Ok(file);
    }
    copy_file(source, &file, cancel)?;
    Ok(file)
}
pub fn existing(directory: &File, name: &str) -> Result<Option<(u64, u64)>> {
    permitted(name)?;
    match at::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(m) => {
            #[cfg(target_os = "macos")]
            let dev = m.st_dev as u64;
            #[cfg(not(target_os = "macos"))]
            let dev = m.st_dev;
            Ok(Some((dev, m.st_ino)))
        }
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn copy_file(mut source: &File, mut target: &File, cancel: &AtomicBool) -> Result<()> {
    let mut buf = vec![0; 256 * 1024];
    loop {
        crate::input::check(cancel)?;
        let n = match source.read(&mut buf) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if n == 0 {
            return Ok(());
        }
        target.write_all(&buf[..n])?;
    }
}

// The staging name belongs to this operation and is removed even if the
// transfer is cancelled after the copy completes but before publication.
struct StagingFile {
    directory: Arc<File>,
    name: String,
}
impl Drop for StagingFile {
    fn drop(&mut self) {
        if !self.name.is_empty() {
            let _ = at::unlinkat(&self.directory, self.name.as_str(), AtFlags::empty());
        }
    }
}
pub struct PreparedDownload {
    source_dir: File,
    destination: Arc<File>,
    staged: Option<StagingFile>,
    id: Identity,
    _scratch: TempDir,
}
impl PreparedDownload {
    pub fn prepare(
        scratch: TempDir,
        destination: Arc<File>,
        size: i64,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        Self::stage(scratch, destination, size, cancel, false)
    }
    pub fn copy_to_destination(self, size: i64, cancel: &AtomicBool) -> Result<Self> {
        ensure!(self.staged.is_none(), "Cannot publish staged file");
        Self::stage(self._scratch, self.destination, size, cancel, true)
    }
    fn stage(
        scratch: TempDir,
        destination: Arc<File>,
        size: i64,
        cancel: &AtomicBool,
        force_copy: bool,
    ) -> Result<Self> {
        crate::input::check(cancel)?;
        let source_dir = File::open(scratch.path())?;
        let source: File = at::openat(&source_dir, "data", READ, Mode::empty())?.into();
        let meta = source.metadata()?;
        ensure!(
            meta.is_file() && i64::try_from(meta.len())? == size,
            "Downloaded file has unexpected length or type"
        );
        at::fchmod(&source, Mode::RUSR | Mode::WUSR)?;
        let (staged, id) = if !force_copy && meta.dev() == destination.metadata()?.dev() {
            source.sync_all()?;
            (None, identity(&meta))
        } else {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random)
                .map_err(|_| anyhow::anyhow!("System entropy unavailable"))?;
            let name = format!(".megawren-part-{:032x}", u128::from_be_bytes(random));
            let file: File = at::openat(
                &destination,
                name.as_str(),
                OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )?
            .into();
            let guard = StagingFile {
                directory: destination.clone(),
                name,
            };
            copy_file(&source, &file, cancel)?;
            file.set_modified(meta.modified()?)?;
            file.sync_all()?;
            let copied = file.metadata()?;
            ensure!(copied.len() == meta.len(), "Incomplete download copy");
            (Some(guard), identity(&copied))
        };
        Ok(Self {
            source_dir,
            destination,
            staged,
            id,
            _scratch: scratch,
        })
    }
    pub fn publish(&mut self, name: &str, overwrite: bool) -> Result<Identity> {
        permitted(name)?;
        let flags = if overwrite {
            match at::statat(&self.destination, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(m) => ensure!(
                    at::FileType::from_raw_mode(m.st_mode).is_file(),
                    "Refusing to replace a nonregular file"
                ),
                Err(rustix::io::Errno::NOENT) => (),
                Err(e) => return Err(e.into()),
            }
            RenameFlags::empty()
        } else {
            RenameFlags::NOREPLACE
        };
        if let Some(staged) = self.staged.as_mut() {
            at::renameat_with(
                &self.destination,
                staged.name.as_str(),
                &self.destination,
                name,
                flags,
            )?;
            staged.name.clear();
        } else {
            at::renameat_with(&self.source_dir, "data", &self.destination, name, flags)
                .context("Cannot publish download at the selected destination")?;
        }
        self.destination
            .sync_all()
            .context("File published. Directory durability could not be confirmed")?;
        Ok(self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_publication_never_replaces_without_consent_and_checks_size() {
        let root = tempfile::tempdir().unwrap();

        let dir = File::open(root.path()).unwrap();
        std::fs::write(root.path().join("file"), b"old").unwrap();
        let publish = |overwrite, size| -> Result<_> {
            let scratch = tempfile::tempdir_in(root.path())?;
            std::fs::write(scratch.path().join("data"), b"new")?;
            PreparedDownload::prepare(
                scratch,
                Arc::new(dir.try_clone()?),
                size,
                &AtomicBool::new(false),
            )?
            .publish("file", overwrite)
        };
        assert!(publish(false, 3).is_err());
        assert!(publish(true, 4).is_err());
        assert_eq!(std::fs::read(root.path().join("file")).unwrap(), b"old");
        publish(true, 3).unwrap();
        assert_eq!(std::fs::read(root.path().join("file")).unwrap(), b"new");
    }
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires a dedicated tiny filesystem at MEGAWREN_TEST_SMALL_VOLUME"]
    fn disk_full_preserves_previous_file_and_removes_incomplete_copy() -> Result<()> {
        let destination = tempfile::tempdir_in(std::env::var("MEGAWREN_TEST_SMALL_VOLUME")?)?;
        let dir = File::open(destination.path())?;
        let stats = at::fstatvfs(&dir)?;
        ensure!(
            stats.f_blocks.saturating_mul(stats.f_frsize) <= 2 * 1024 * 1024,
            "Expected a dedicated filesystem of at most 2 MiB"
        );
        let scratch = tempfile::tempdir()?;
        std::fs::write(destination.path().join("file"), b"original")?;
        let size = 4 * 1024 * 1024;
        std::fs::write(scratch.path().join("data"), vec![1; size])?;
        let error =
            PreparedDownload::prepare(scratch, Arc::new(dir), size as i64, &AtomicBool::new(false))
                .and_then(|mut file| file.publish("file", true))
                .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .and_then(|e| e.raw_os_error()),
            Some(rustix::io::Errno::NOSPC.raw_os_error())
        );
        assert_eq!(std::fs::read(destination.path().join("file"))?, b"original");
        assert_eq!(std::fs::read_dir(destination.path())?.count(), 1);
        Ok(())
    }
    #[test]
    fn directory_and_source_handles_survive_renames_and_exclude_private_state() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let state = base.join("state");
        std::fs::create_dir(&state).unwrap();
        let guard = Boundary::new(Some(&state)).unwrap();
        assert!(guard.directory(&state).is_err());
        let dir = base.join("work");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("x"), b"original").unwrap();
        let selected = guard.source(&dir.join("x")).unwrap();
        std::fs::rename(&dir, base.join("old")).unwrap();
        std::os::unix::fs::symlink(&state, &dir).unwrap();
        assert!(guard.source(&dir.join("x")).is_err());
        let scratch = tempfile::tempdir().unwrap();
        selected
            .snapshot(&scratch, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            std::fs::read(scratch.path().join("data")).unwrap(),
            b"original"
        );
    }
}
