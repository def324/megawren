use crate::{bridge::ffi, cache, operation::Stage, state};
use anyhow::{Result, bail, ensure};
use std::{
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use zeroize::Zeroizing;
pub struct Backend {
    // Native object must die before the working directory.
    client: cxx::UniquePtr<ffi::Client>,
    _run: state::Run,
    cache_path: PathBuf,
    cache_key: Option<Zeroizing<Vec<u8>>>,
    drive_ready: bool,
    restored_stamp: Option<cache::Stamp>,
    pub cancel: Arc<AtomicBool>,
    pub remembered: bool,
    pub authenticated: bool,
    pub progress: Option<std::sync::mpsc::SyncSender<(i64, i64)>>,
    pub interactive_ui: bool,
}
impl Backend {
    #[cfg(test)]
    pub fn new(s: &state::State, cancel: Arc<AtomicBool>) -> Result<Self> {
        let mut backend = Self::deferred(s, cancel)?;
        backend.initialize()?;
        Ok(backend)
    }
    pub fn deferred(s: &state::State, cancel: Arc<AtomicBool>) -> Result<Self> {
        let run = s.run()?;
        Ok(Self {
            client: cxx::UniquePtr::null(),
            _run: run,
            cache_path: s.path.join("cache.enc"),
            cache_key: None,
            drive_ready: false,
            restored_stamp: None,
            cancel,
            remembered: false,
            authenticated: false,
            progress: None,
            interactive_ui: false,
        })
    }
    pub fn initialize(&mut self) -> Result<()> {
        if self.client.is_null() {
            self.client = ffi::client(state::native(self._run.path())?)?;
        }
        Ok(())
    }
    fn expired(&mut self) -> Result<()> {
        self.authenticated = false;
        self.remembered = false;
        self.cache_key = None;
        Err(crate::error::ExpiredSession.into())
    }
    pub fn stop_sdk(&mut self) {
        self.client = cxx::UniquePtr::null();
        self.authenticated = false;
    }
    pub fn api(&mut self) -> Pin<&mut ffi::Client> {
        self.client.pin_mut()
    }
    pub fn scratch(&self) -> Result<tempfile::TempDir> {
        state::private_tempdir(self._run.path(), "transfer-")
    }
    pub fn state_root(&self) -> &Path {
        self.cache_path.parent().unwrap()
    }
    pub fn preview_file(&self) -> Result<std::fs::File> {
        Ok(tempfile::tempfile_in(self._run.path())?)
    }
    pub fn wait_code(&mut self, label: Stage) -> Result<i32> {
        self.wait_with_timeout(label, Duration::from_secs(300))
    }
    fn wait_with_timeout(&mut self, label: Stage, idle_limit: Duration) -> Result<i32> {
        crate::operation::wait(
            &mut crate::operation::Native {
                client: self.client.pin_mut(),
                revoke_session: !self.remembered,
            },
            &self.cancel,
            self.progress.as_ref(),
            label,
            self.interactive_ui,
            idle_limit,
        )
    }
    pub fn wait_for(&mut self, label: Stage) -> Result<()> {
        let code = self.wait_code(label)?;
        if code != 0 {
            let detail = self.api().errors()?;
            if !detail.is_empty() {
                bail!("{detail}");
            }
            bail!("MEGA request failed ({code})");
        }
        Ok(())
    }
    pub fn wait(&mut self) -> Result<()> {
        self.wait_for(Stage::Working)
    }
    pub fn restore(&mut self, session: &str, use_cache: bool) -> Result<()> {
        crate::input::check(&self.cancel)?;
        let key = cache::key(session.as_bytes())?;
        if use_cache {
            match cache::restore(&self.cache_path, &key, &self.cancel) {
                Ok(Some((run, format))) => {
                    // Close the empty SDK before installing the restored databases.
                    self.client = cxx::UniquePtr::null();
                    if format == cache::Format::configured() {
                        self.restored_stamp = cache::stamp(run.path()).ok();
                    }
                    self._run = run;
                }
                Ok(None) => (),
                Err(e) if e.downcast_ref::<crate::input::Cancelled>().is_some() => return Err(e),
                Err(_) => crate::feedback::note("Local index unavailable; rebuilding from MEGA."),
            }
        }
        self.cache_key = Some(key);
        self.remembered = true;
        self.initialize()?;
        self.api().restore(session)?;
        let code = self.wait_code(Stage::RestoringLogin)?;
        if code == -15 {
            return self.expired();
        }
        ensure!(code == 0, "Could not restore saved login ({code})");
        self.authenticated = true;
        Ok(())
    }
    pub fn login(&mut self, email: &str, password: &str) -> Result<()> {
        crate::input::check(&self.cancel)?;
        self.initialize()?;
        self.api().login(email, password, "")?;
        let mut code = self.wait_code(Stage::SigningIn)?;
        let mut submitted_otp = false;
        while code == -26 || (submitted_otp && code == -11) {
            let otp = crate::prompt("Authenticator code: ", &self.cancel)?;
            if otp.is_empty() {
                bail!("Login cancelled");
            }
            self.api().login(email, password, &otp)?;
            submitted_otp = true;
            code = self.wait_code(Stage::SigningIn)?;
            if code != -26 && code != -11 {
                break;
            }
            crate::feedback::note("Code not accepted. Try the next authenticator code.");
        }
        match code {
            0 => (),
            -9 => bail!("MEGA did not accept the email or password. Check them and try again."),
            -6 => bail!("MEGA rate-limited login. Wait before trying again."),
            _ => bail!("Login failed ({code})"),
        }
        self.authenticated = true;
        Ok(())
    }
    pub fn load_drive(&mut self) -> Result<()> {
        crate::input::check(&self.cancel)?;
        self.api().fetch()?;
        self.wait_for(Stage::LoadingCloud)?;
        self.drive_ready = true;
        Ok(())
    }
    pub fn load_current_drive(&mut self) -> Result<()> {
        crate::input::check(&self.cancel)?;
        self.api().fetch_current()?;
        self.wait_for(Stage::LoadingCloud)?;
        self.drive_ready = true;
        Ok(())
    }
    pub fn remember_session(&mut self, session: &[u8]) -> Result<()> {
        self.cache_key = Some(cache::key(session)?);
        self.remembered = true;
        Ok(())
    }
    pub fn finish(mut self) -> Result<()> {
        // SQLite status changes alone need not republish a recent snapshot.
        // Cloud writes and full tree reloads go through the normal save path.
        let reuse = self.restored_stamp.is_some()
            && self
                .client
                .as_ref()
                .is_some_and(|sdk| !sdk.needs_checkpoint())
            && std::fs::metadata(&self.cache_path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age < Duration::from_secs(24 * 3600));
        // Closing SQLite first gives a consistent, checkpointed snapshot. Old
        // encrypted snapshots remain valid if sealing is interrupted or fails.
        self.client = cxx::UniquePtr::null();
        if !reuse
            && self.remembered
            && self.authenticated
            && self.drive_ready
            && let Some(key) = self.cache_key.as_ref()
        {
            let first = !self.cache_path.try_exists()?;
            if !first
                && self
                    .restored_stamp
                    .is_some_and(|old| cache::stamp(self._run.path()).is_ok_and(|now| old == now))
            {
                return Ok(());
            }
            if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                cache::save_after_interrupt(&self.cache_path, self._run.path(), key)?;
            } else {
                cache::save(&self.cache_path, self._run.path(), key, &self.cancel)?;
            }
            if first && self.cache_path.try_exists()? {
                crate::feedback::note(format!(
                    "Local index: {} (encrypted)",
                    self.cache_path.display()
                ));
            }
        }
        Ok(())
    }
    pub fn session(&mut self) -> Result<Zeroizing<String>> {
        Ok(Zeroizing::new(self.api().session()?))
    }
    pub fn children(&mut self, id: u64, offset: usize) -> Result<Vec<ffi::Entry>> {
        crate::input::check(&self.cancel)?;
        Ok(self.api().children(id, offset, 256)?)
    }
    pub fn resolve(&mut self, path: &str) -> Result<u64> {
        crate::input::check(&self.cancel)?;
        ensure!(path.starts_with('/'), "Remote paths must begin with /");
        let epoch = self.api().generation();
        let mut id = self.api().root()?;
        for part in path.split('/').filter(|s| !s.is_empty()) {
            state::component(part)?;
            id = self.child(id, part)?.id;
        }
        self.check_generation(epoch)?;
        Ok(id)
    }
    fn child(&mut self, id: u64, name: &str) -> Result<ffi::Entry> {
        self.find(id, name)?
            .ok_or_else(|| anyhow::anyhow!("Remote item not found: {}", state::display(name)))
    }
    pub fn check_generation(&mut self, epoch: u64) -> Result<()> {
        ensure!(
            self.api().generation() == epoch,
            "Cloud files changed. Run the command again."
        );
        Ok(())
    }
    pub fn download_entries<'a>(
        &mut self,
        paths: impl Iterator<Item = &'a str>,
    ) -> Result<Vec<ffi::Entry>> {
        // Shell globs and lists often contain many files in the same folder.
        let epoch = self.api().generation();
        let mut folders = std::collections::HashMap::new();
        let root = self.api().root()?;
        folders.insert("", root);
        let mut entries = Vec::new();
        for path in paths {
            crate::input::check(&self.cancel)?;
            let path = path.trim_end_matches('/');
            if path.is_empty() {
                entries.push(self.api().info(root)?);
                continue;
            }
            let (folder, name) = path
                .rsplit_once('/')
                .ok_or_else(|| anyhow::anyhow!("Remote paths must begin with /"))?;
            let parent = match folders.entry(folder) {
                std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                std::collections::hash_map::Entry::Vacant(e) => *e.insert(self.resolve(folder)?),
            };
            entries.push(self.child(parent, name)?);
        }
        self.check_generation(epoch)?;
        Ok(entries)
    }
    pub fn find(&mut self, id: u64, name: &str) -> Result<Option<ffi::Entry>> {
        if !self.api().exists(id, name)? {
            return Ok(None);
        }
        let epoch = self.api().generation();
        let found = unique_named(name, |offset| {
            crate::input::check(&self.cancel)?;
            Ok(self.api().named_children(id, name, offset)?)
        })?;
        self.check_generation(epoch)?;
        Ok(found)
    }
    pub fn revoke(&mut self) -> Result<()> {
        self.revoke_with_timeout(Duration::from_secs(300))
    }
    pub fn revoke_on_exit(&mut self) -> Result<()> {
        self.revoke_with_timeout(Duration::from_secs(2))
    }
    fn revoke_with_timeout(&mut self, idle_limit: Duration) -> Result<()> {
        self.api().logout()?;
        let c = self.wait_with_timeout(Stage::SigningOut, idle_limit)?;
        ensure!(c == 0 || c == -15, "Could not revoke session ({c})");
        self.authenticated = false;
        Ok(())
    }
}

// The SDK's name filter can match substrings and case variants. Only exact
// names count, including a second exact name on a subsequent result page.
fn unique_named(
    name: &str,
    mut page: impl FnMut(usize) -> Result<Vec<ffi::Entry>>,
) -> Result<Option<ffi::Entry>> {
    let mut found = None;
    let mut offset = 0;
    loop {
        let rows = page(offset)?;
        let n = rows.len();
        for row in rows.into_iter().filter(|row| row.name == name) {
            ensure!(found.is_none(), "Ambiguous remote name");
            found = Some(row);
        }
        if n < 256 {
            return Ok(found);
        }
        offset += n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn named(name: &str) -> ffi::Entry {
        ffi::Entry {
            name: name.into(),
            id: 1,
            size: 0,
            folder: false,
            shared: false,
            exported: false,
            parent: 0,
            fingerprint: String::new(),
            modified: 0,
        }
    }
    #[test]
    fn name_lookup_requires_exact_unique_match_across_all_candidate_pages() {
        let result = unique_named("x", |_| Ok(vec![named("X"), named("prefix x"), named("x")]))
            .unwrap()
            .unwrap();
        assert_eq!(result.name, "x");
        assert!(
            unique_named("x", |_| Ok(vec![named("x1")]))
                .unwrap()
                .is_none()
        );
        assert!(
            unique_named("x", |offset| {
                let mut rows = vec![named("x")];
                if offset == 0 {
                    rows.extend((1..256).map(|_| named("xx")));
                }
                Ok(rows)
            })
            .is_err()
        );
    }

    fn read_only_exit_sample(blocks: usize) -> Result<()> {
        use std::{
            fs,
            io::Write,
            os::unix::fs::MetadataExt,
            time::{Instant, SystemTime},
        };
        let tmp = tempfile::tempdir()?;
        let state = state::State::at(tmp.path().join("state"))?;
        let cancel = Arc::new(AtomicBool::new(false));
        let cache_path = state.path.join("cache.enc");
        let db = "megaclient_statecache20_fixture.db";
        let status = "megaclient_statecache20_status_fixture.db";
        let session = b"synthetic benchmark session";
        let key = cache::key(session)?;
        {
            let run = state.run()?;
            let mut file = fs::File::create(run.path().join(db))?;
            let mut block = vec![0u8; 256 * 1024];
            getrandom::fill(&mut block[..104 * 1024])
                .map_err(|_| anyhow::anyhow!("Entropy unavailable"))?;
            for _ in 0..blocks {
                file.write_all(&block)?;
            }
            fs::write(run.path().join(status), [0])?;
            cache::save(&cache_path, run.path(), &key, &cancel)?;
        }
        for (i, (interrupted, stale)) in [(false, false), (true, false), (false, true)]
            .into_iter()
            .enumerate()
        {
            if stale {
                fs::File::options()
                    .write(true)
                    .open(&cache_path)?
                    .set_modified(SystemTime::now() - Duration::from_secs(2 * 86400))?;
            }
            let before = fs::metadata(&cache_path)?;
            let start = Instant::now();
            let (run, _) = cache::restore(&cache_path, &key, &cancel)?.unwrap();
            let restore = start.elapsed();
            let mut backend = Backend::deferred(&state, cancel.clone())?;
            backend._run = run;
            backend.restored_stamp = Some(cache::stamp(backend._run.path())?);
            backend.initialize()?;
            backend.remember_session(session)?;
            backend.authenticated = true;
            backend.drive_ready = true;
            fs::write(backend._run.path().join(status), [i as u8 + 1])?;
            let end_of_listing = Instant::now();
            cancel.store(interrupted, std::sync::atomic::Ordering::Relaxed);
            backend.finish()?;
            cancel.store(false, std::sync::atomic::Ordering::Relaxed);
            let after = fs::metadata(&cache_path)?;
            assert_eq!(before.ino() == after.ino(), !stale);
            if blocks > 1 {
                eprintln!(
                    "interrupted={interrupted} stale={stale} encrypted_bytes={} restore_ms={} after_rows_ms={}",
                    after.len(),
                    restore.as_millis(),
                    end_of_listing.elapsed().as_millis()
                );
            }
        }
        Ok(())
    }
    #[test]
    fn read_only_exit_keeps_recent_snapshot_on_quit_and_interrupt_and_renews_it_when_old() {
        read_only_exit_sample(1).unwrap();
    }
    #[test]
    #[ignore = "times shutdown with 768 MiB of synthetic database bytes"]
    fn measure_large_listing_exit() {
        read_only_exit_sample(3072).unwrap();
    }
    #[test]
    fn cancelling_metadata_requests_quiesces_without_a_server_response() {
        let temp = tempfile::tempdir().unwrap();
        let state = state::State::at(temp.path().join("state")).unwrap();
        let mut backend = Backend::new(&state, Arc::new(AtomicBool::new(false))).unwrap();
        for _ in 0..8 {
            // Cancel immediately without depending on any server response.
            // No login or account credentials are involved.
            backend.api().fetch().unwrap();
            backend.api().cancel(false).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !backend.api().event().unwrap().done {
                assert!(
                    std::time::Instant::now() < deadline,
                    "SDK cancellation did not complete"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(backend.api().root().is_err());
        }
    }
    #[test]
    fn native_initialization_without_app_key_and_teardown_leave_no_run_directory() {
        let temp = tempfile::tempdir().unwrap();
        let state = state::State::at(temp.path().join("state")).unwrap();
        for _ in 0..8 {
            let backend = Backend::new(&state, Arc::new(AtomicBool::new(false))).unwrap();
            assert!(backend._run.path().is_dir());
            drop(backend);
            assert_eq!(std::fs::read_dir(&state.path).unwrap().count(), 0);
        }
    }
    #[test]
    fn closes_sdk_before_saving_and_keeps_only_encrypted_index_after_exit() {
        for interrupted in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let state = state::State::at(temp.path().join("state")).unwrap();
            let cancel = Arc::new(AtomicBool::new(false));
            let mut backend = Backend::new(&state, cancel.clone()).unwrap();
            // Synthetic bytes exercise local lifecycle only, without an account/login.
            let session = b"synthetic cache lifecycle test";
            backend.remember_session(session).unwrap();
            backend.authenticated = true;
            backend.drive_ready = true;
            let filename = "megaclient_statecache20_synthetic.db";
            std::fs::write(backend._run.path().join(filename), b"index contents").unwrap();
            cancel.store(interrupted, std::sync::atomic::Ordering::Relaxed);
            backend.finish().unwrap();
            assert_eq!(
                cancel.load(std::sync::atomic::Ordering::Relaxed),
                interrupted
            );
            assert_eq!(std::fs::read_dir(&state.path).unwrap().count(), 1);
            let key = cache::key(session).unwrap();
            let (restored, _) =
                cache::restore(&state.path.join("cache.enc"), &key, &AtomicBool::new(false))
                    .unwrap()
                    .unwrap();
            assert_eq!(
                std::fs::read(restored.path().join(filename)).unwrap(),
                b"index contents"
            );
        }
    }
    #[test]
    fn interrupted_incomplete_load_does_not_replace_a_previous_index() {
        let temp = tempfile::tempdir().unwrap();
        let state = state::State::at(temp.path().join("state")).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut backend = Backend::new(&state, cancel.clone()).unwrap();
        backend
            .remember_session(b"synthetic incomplete-load session")
            .unwrap();
        backend.authenticated = true;
        let file = backend
            ._run
            .path()
            .join("megaclient_statecache20_synthetic.db");
        std::fs::write(&file, b"previous index").unwrap();
        cache::save(
            &state.path.join("cache.enc"),
            backend._run.path(),
            backend.cache_key.as_ref().unwrap(),
            &cancel,
        )
        .unwrap();
        let previous = std::fs::read(state.path.join("cache.enc")).unwrap();
        std::fs::write(&file, b"partial rebuild").unwrap();
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        backend.finish().unwrap();
        assert_eq!(
            std::fs::read(state.path.join("cache.enc")).unwrap(),
            previous
        );
    }
}
