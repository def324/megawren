//! Reproducible, opt-in isolated-account upload measurements through the browser worker.
use super::*;
use anyhow::Context;
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

fn sample(
    remote: &mut Backend,
    command: Command,
    count: usize,
    bytes: usize,
    run: usize,
) -> Result<()> {
    let shared = Arc::new(Mutex::new(Snapshot::default()));
    let mut engine = Engine::new(remote, shared.clone());
    let direction = if matches!(command, Command::Download { .. }) {
        "download"
    } else {
        "upload"
    };
    let start = Instant::now();
    engine.enqueue(command)?;
    let mut first = None;
    let mut completed_at = None;
    loop {
        engine.tick();
        let state = snapshot(&shared);
        let job = &state.jobs[0];
        if job.bytes > 0 {
            first.get_or_insert(start.elapsed());
        }
        if count == 1 && job.total > 0 && job.bytes >= job.total && !job.state.terminal() {
            completed_at.get_or_insert(start.elapsed());
        }
        if job.state.terminal() {
            ensure!(
                job.state == JobState::Complete && job.files == count,
                "Upload sample failed after {} files: {:?}",
                job.files,
                job.error
            );
            let seconds = start.elapsed().as_secs_f64();
            eprintln!(
                "{direction} sample={run} files={count} bytes={bytes} seconds={seconds:.3} MiB/s={:.3} files/s={:.2} first_observed_s={:?} tail_after_full_file_s={:?}",
                bytes as f64 / 1048576.0 / seconds,
                count as f64 / seconds,
                first.map(|d| d.as_secs_f64()),
                completed_at.map(|d| seconds - d.as_secs_f64())
            );
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(900) {
            engine.shutdown(false)?;
            bail!("Upload sample timed out");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "uploads two 2 MiB single-file and 128-file samples to an injected isolated account"]
fn live_upload_workloads() -> Result<()> {
    workloads(false)
}
#[test]
#[ignore = "round-trips a fresh 256 MiB random file through the production transfer worker"]
fn live_large_random_roundtrip() -> Result<()> {
    workloads(true)
}
fn workloads(large: bool) -> Result<()> {
    let email = Zeroizing::new(std::env::var("MEGAWREN_TEST_USER")?);
    let password = Zeroizing::new(std::env::var("MEGAWREN_TEST_PASSWORD")?);
    let tmp = tempfile::tempdir()?;
    let base = tmp.path().canonicalize()?;
    let state = state::State::at(base.join("state"))?;
    let mut remote = Backend::new(&state, Arc::new(AtomicBool::new(false)))?;
    remote.login(&email, &password)?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| anyhow::anyhow!("Entropy unavailable"))?;
    let name = format!("megawren-benchmark-{:032x}", u128::from_be_bytes(random));
    let mut fixture = None;
    let result = (|| -> Result<()> {
        remote.load_drive()?;
        let root = remote.api().root()?;
        remote.api().mkdir(root, &name)?;
        remote.wait()?;
        let folder = remote.api().result_node()?;
        fixture = Some(folder);
        if large {
            use std::io::{Read, Write};
            const LARGE: usize = 256 * 1024 * 1024;
            let path = base.join("random.bin");
            let mut block = vec![0; 1024 * 1024];
            let mut file = fs::File::create(&path)?;
            for _ in 0..256 {
                getrandom::fill(&mut block).map_err(|_| anyhow::anyhow!("Entropy unavailable"))?;
                file.write_all(&block)?;
            }
            file.sync_all()?;
            drop(file);
            sample(
                &mut remote,
                Command::Upload {
                    parent: folder,
                    destination: "benchmark".into(),
                    sources: vec![path.clone()],
                },
                1,
                LARGE,
                1,
            )?;
            let entry = remote
                .find(folder, "random.bin")?
                .ok_or_else(|| anyhow::anyhow!("Published file missing"))?;
            let dest = base.join("download");
            fs::create_dir(&dest)?;
            sample(
                &mut remote,
                Command::Download {
                    entry: entry.into(),
                    destination: dest.clone(),
                    overwrite: false,
                },
                1,
                LARGE,
                1,
            )?;
            let mut original = fs::File::open(&path)?;
            let mut copy = fs::File::open(dest.join("random.bin"))?;
            ensure!(
                original.metadata()?.len() == copy.metadata()?.len(),
                "File length changed"
            );
            let mut other = vec![0; block.len()];
            for _ in 0..256 {
                original.read_exact(&mut block)?;
                copy.read_exact(&mut other)?;
                ensure!(block == other, "Downloaded payload differs");
            }
            return Ok(());
        }
        const BYTES: usize = 2 * 1024 * 1024;
        for run in 1..=2 {
            for count in [1, 128] {
                let mut paths = Vec::with_capacity(count);
                let mut payload = vec![0u8; BYTES / count];
                for i in 0..count {
                    let path = base.join(format!("sample-{run}-{count}-{i}"));
                    getrandom::fill(&mut payload)
                        .map_err(|_| anyhow::anyhow!("Entropy unavailable"))?;
                    fs::write(&path, &payload)?;
                    paths.push(path);
                }
                sample(
                    &mut remote,
                    Command::Upload {
                        parent: folder,
                        destination: "benchmark".into(),
                        sources: paths,
                    },
                    count,
                    BYTES,
                    run,
                )?;
            }
        }
        Ok(())
    })();
    let cleanup = (|| -> Result<()> {
        if let Some(id) = fixture {
            ensure!(
                remote.api().info(id)?.name == name,
                "Benchmark fixture changed"
            );
            let epoch = remote.api().generation();
            remote.api().change(4, id, 0, "", epoch)?;
            remote.wait()?;
        }
        Ok(())
    })();
    let logout = remote.revoke_on_exit();
    result?;
    cleanup?;
    logout?;
    remote.finish()
}

#[test]
#[ignore = "measures encrypted-index startup and clean exit on the injected isolated account"]
fn live_saved_startup() -> Result<()> {
    let email = Zeroizing::new(std::env::var("MEGAWREN_TEST_USER")?);
    let password = Zeroizing::new(std::env::var("MEGAWREN_TEST_PASSWORD")?);
    let tmp = tempfile::tempdir()?;
    let state = state::State::at(tmp.path().join("state"))?;
    let _lock = state.lock(true)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let mut remote = Backend::new(&state, cancel.clone())?;
    remote.login(&email, &password)?;
    let session = remote.session()?;
    let result = (|| -> Result<()> {
        let mut local_key = Zeroizing::new([0u8; 32]);
        getrandom::fill(local_key.as_mut()).map_err(|_| anyhow::anyhow!("Entropy unavailable"))?;
        // Ephemeral local passphrase material exercises the normal vault KDF.
        let vault = crate::vault::seal(local_key.as_ref(), session.as_bytes())?;
        remote.remember_session(session.as_bytes())?;
        let start = Instant::now();
        remote.load_drive()?;
        let cold = start.elapsed();
        remote.finish()?;
        let start = Instant::now();
        let plain = crate::vault::open(local_key.as_ref(), &vault)?;
        let mut restored = Backend::deferred(&state, cancel.clone())?;
        let unlocked = start.elapsed();
        restored.restore(std::str::from_utf8(&plain)?, true)?;
        let logged_in = start.elapsed();
        restored.load_current_drive()?;
        let warm = start.elapsed();
        let root = restored.api().root()?;
        let lookup = if let Some(entry) = restored
            .children(root, 0)?
            .into_iter()
            .find(|e| state::component(&e.name).is_ok())
        {
            let start = Instant::now();
            let _ = restored.find(root, &entry.name)?;
            Some(start.elapsed().as_micros())
        } else {
            None
        };
        let start = Instant::now();
        restored.finish()?;
        let close = start.elapsed();
        eprintln!(
            "cold_fetch_s={:.3} warm_unlock_to_drive_s={:.3} vault_ms={} cache_and_session_ms={} sdk_refresh_ms={} lookup_us={lookup:?} listing_exit_ms={} encrypted_index_bytes={}",
            cold.as_secs_f64(),
            warm.as_secs_f64(),
            unlocked.as_millis(),
            (logged_in - unlocked).as_millis(),
            (warm - logged_in).as_millis(),
            close.as_millis(),
            fs::metadata(state.path.join("cache.enc"))?.len()
        );
        // Compare both encodings on the very same closed SDK databases.
        use crate::cache::{self, Format};
        let key = cache::key(session.as_bytes())?;
        let (source, _) = cache::restore(&state.path.join("cache.enc"), &key, &cancel)?
            .context("Expected saved index")?;
        let sample = state.path.join("comparison.enc");
        for format in [Format::Original, Format::Zstd] {
            let start = Instant::now();
            cache::save_inner(&sample, source.path(), &key, &cancel, false, format)?;
            let save = start.elapsed();
            let start = Instant::now();
            let (check, decoded) =
                cache::restore(&sample, &key, &cancel)?.context("Expected comparison index")?;
            let load = start.elapsed();
            ensure!(format == decoded, "Wrong index format");
            for entry in fs::read_dir(source.path())? {
                let entry = entry?;
                ensure!(
                    fs::read(check.path().join(entry.file_name()))? == fs::read(entry.path())?,
                    "Index bytes differ"
                );
            }
            eprintln!(
                "index_format={format:?} bytes={} save_ms={} restore_ms={}",
                fs::metadata(&sample)?.len(),
                save.as_millis(),
                load.as_millis()
            );
        }
        // An unsupported snapshot must still open the saved session, fetch the
        // account index, and publish a cache that the current reader accepts.
        let path = state.path.join("cache.enc");
        fs::write(&path, b"MGWCACH\x02")?;
        let mut fresh = Backend::deferred(&state, cancel.clone())?;
        fresh.restore(std::str::from_utf8(&plain)?, true)?;
        fresh.load_drive()?;
        fresh.finish()?;
        let (_, format) = cache::restore(&path, &key, &cancel)?.context("Missing rebuilt index")?;
        ensure!(format == Format::configured(), "Wrong rebuilt index format");
        Ok(())
    })();
    let logout = (|| {
        let mut cleanup = Backend::deferred(&state, cancel)?;
        cleanup.restore(&session, false)?;
        cleanup.revoke()?;
        cleanup.finish()
    })();
    result.and(logout)
}
