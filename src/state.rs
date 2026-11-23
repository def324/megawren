pub use crate::text::display;
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

pub fn home() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("Cannot determine home directory")?
        .join(".megawren"))
}
fn validate(path: &Path, directory: bool) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(!meta.file_type().is_symlink(), "State path is a symlink");
    ensure!(
        meta.is_dir() == directory && (directory || meta.is_file()),
        "Invalid state file type"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            meta.mode() & 0o077 == 0,
            "State permissions must be private: {}",
            path.display()
        );
    }
    Ok(())
}
fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options
}
pub struct State {
    pub path: PathBuf,
}
pub fn private_tempdir(parent: &Path, prefix: &str) -> Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    Ok(builder.tempdir_in(parent)?)
}
#[derive(Debug)]
pub struct Run {
    dir: tempfile::TempDir,
    _lease: File,
}
impl Run {
    pub fn new(parent: &Path) -> Result<Self> {
        let dir = private_tempdir(parent, "run-")?;
        let lease = private_options()
            .create_new(true)
            .open(dir.path().join(".megawren-run-v1"))?;
        lease.try_lock()?;
        Ok(Self { dir, _lease: lease })
    }
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}
impl State {
    pub fn open() -> Result<Self> {
        Self::at(home()?)
    }
    pub fn at(path: PathBuf) -> Result<Self> {
        #[cfg(not(unix))]
        anyhow::bail!("Secure state permissions on this platform are not implemented yet");
        if !path.try_exists()? {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => crate::feedback::note(format!("Working data: {}", path.display())),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e.into()),
            }
        }
        validate(&path, true)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            ensure!(
                fs::symlink_metadata(&path)?.uid() == unsafe { libc::geteuid() },
                "State directory belongs to another user"
            );
        }
        Ok(Self { path })
    }
    pub fn lock(&self, exclusive: bool) -> Result<File> {
        let path = self.path.join("state.lock");
        let f = private_options().create(true).truncate(false).open(&path)?;
        validate(&path, false)?;
        if exclusive {
            f.try_lock()
                .context("Account in use by another megawren process")?;
            self.recover_runs()?;
        } else {
            if f.try_lock().is_ok() {
                self.recover_runs()?;
                f.unlock()?;
            }
            f.try_lock_shared()
                .context("Account in use by another megawren process")?;
        }
        Ok(f)
    }
    pub fn run(&self) -> Result<Run> {
        Run::new(&self.path)
    }
    // Called only under the exclusive account lock. Every active SDK holds a reader lock.
    fn recover_runs(&self) -> Result<()> {
        for child in fs::read_dir(&self.path)? {
            let child = child?;
            if !child.file_name().as_encoded_bytes().starts_with(b"run-")
                || !child.file_type()?.is_dir()
            {
                continue;
            }
            let dir = child.path();
            let marker = dir.join(".megawren-run-v1");
            if validate(&dir, true).is_ok()
                && validate(&marker, false).is_ok()
                && let Ok(lease) = private_options().open(&marker)
                && lease.metadata()?.len() == 0
                && lease.try_lock().is_ok()
            {
                fs::remove_dir_all(dir)?;
            }
        }
        Ok(())
    }
    pub fn read_vault(&self) -> Result<Vec<u8>> {
        use std::io::Read;
        let path = self.path.join("session.enc");
        validate(&path, false)?;
        let f = private_options().write(false).open(&path)?;
        ensure!(f.metadata()?.len() <= 65536, "Session vault is too large");
        let mut data = Vec::new();
        f.take(65537).read_to_end(&mut data)?;
        ensure!(data.len() <= 65536, "Session vault is too large");
        Ok(data)
    }
    pub fn save(&self, bytes: &[u8]) -> Result<()> {
        use std::io::Write;
        let mut temp = tempfile::NamedTempFile::new_in(&self.path)?;
        temp.write_all(bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(self.path.join("session.enc"))
            .map_err(|e| e.error)?;
        File::open(&self.path)?.sync_all()?;
        crate::feedback::note(format!(
            "Saved login: {}",
            self.path.join("session.enc").display()
        ));
        Ok(())
    }
}
pub fn component(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name.len() <= 255 && !matches!(name, "." | ".."),
        "Invalid filename"
    );
    ensure!(
        !name
            .chars()
            .any(|c| c.is_control() || "/\\:<>\"|?*".contains(c)),
        "Unsafe filename"
    );
    ensure!(!name.ends_with(['.', ' ']), "Ambiguous filename");
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    ensure!(
        !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            && !(stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit()),
        "Reserved filename"
    );
    Ok(())
}
pub fn native(path: &Path) -> Result<&str> {
    path.to_str()
        .context("Path cannot be represented in the SDK encoding")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_cleanup_preserves_active_and_unowned_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let state = State::at(tmp.path().join("state")).unwrap();
        let Run { dir, _lease: lease } = state.run().unwrap();
        let run = dir.keep();
        fs::write(run.join("partial.db"), b"partial restore").unwrap();
        let other = private_tempdir(&state.path, "run-").unwrap().keep();
        drop(state.lock(true).unwrap());
        assert!(run.is_dir());
        drop(lease);
        drop(state.lock(true).unwrap());
        assert!(!run.exists());
        assert!(other.is_dir());
    }
    #[test]
    fn rejects_hostile_names() {
        for n in ["..", "/a", "a\\b", "x:y", "CON.txt", "a\u{1b}", "foo."] {
            assert!(component(n).is_err(), "{n}");
        }
        assert!(component("photo.jpg").is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_state() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("link");
        std::os::unix::fs::symlink(t.path(), &p).unwrap();
        assert!(State::at(p).is_err());
    }
    #[test]
    fn exclusive_lock_conflicts() {
        let t = tempfile::tempdir().unwrap();
        let s = State::at(t.path().join("state")).unwrap();
        let _a = s.lock(false).unwrap();
        assert!(s.lock(true).is_err());
    }
}

#[test]
fn release_display_escapes_invisible_controls() {
    assert_eq!(display("dir\u{202e}.txt"), "dir\\u{202e}.txt");
}
