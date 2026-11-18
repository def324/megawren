//! Optional isolated-account integration check. Never uses the saved login.
use super::*;
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

fn finish_change(engine: &mut Engine<'_, Backend>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    while engine.changing {
        ensure!(Instant::now() < deadline, "Live file action timed out");
        engine.tick();
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn checkpoint_after_folder_creation(
    owner: &mut Backend,
    parent: u64,
    scratch: &Path,
) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let state = state::State::at(scratch.join("checkpoint-test"))?;
    let session = owner.session()?;
    let open = || -> Result<Backend> {
        let mut client = Backend::deferred(&state, Arc::new(AtomicBool::new(false)))?;
        client.restore(&session, true)?;
        client.load_current_drive()?;
        Ok(client)
    };
    open()?.finish()?;
    let cache = state.path.join("cache.enc");
    let before = fs::metadata(&cache)?.ino();
    let mut editor = open()?;
    ensure!(!editor.api().needs_checkpoint(), "Expected a restored tree");
    editor.api().mkdir(parent, "checkpoint")?;
    editor.wait()?;
    ensure!(
        editor.api().needs_checkpoint(),
        "Folder creation was missed"
    );
    editor.finish()?;
    ensure!(fs::metadata(cache)?.ino() != before, "Edit was not saved");
    Ok(())
}

fn review(engine: &mut Engine<'_, Backend>, selections: Vec<Selection>) -> Result<Arc<Details>> {
    engine.command(Command::InspectMany {
        request: 91,
        selections,
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        engine.tick();
        if let Some(review) = engine.reviewed.clone() {
            return Ok(review);
        }
        ensure!(
            !snapshot(&engine.shared)
                .details
                .as_ref()
                .is_some_and(|i| i.error.is_some()),
            "Live review failed"
        );
        ensure!(Instant::now() < deadline, "Live review timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn cancel_uploads(remote: &mut Backend, folder: u64, scratch: &Path) -> Result<()> {
    let scratch = scratch.canonicalize()?;
    let first = scratch.join("cancel.bin");
    let mut data = vec![0u8; 8 * 1024 * 1024];
    getrandom::fill(&mut data).map_err(|_| anyhow::anyhow!("Cannot generate upload fixture"))?;
    fs::write(&first, data)?;
    let next = scratch.join("queued.txt");
    fs::write(&next, b"queued")?;
    let shared = Arc::new(Mutex::new(Snapshot::default()));
    let mut engine = Engine::new(remote, shared.clone());
    for path in [first, next] {
        engine.enqueue(Command::Upload {
            sources: vec![path],
            parent: folder,
            destination: "test folder".into(),
        })?;
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        engine.tick();
        let state = snapshot(&shared).jobs[0].state;
        if state == JobState::Transferring {
            break;
        }
        ensure!(
            !state.terminal() && Instant::now() < deadline,
            "Upload did not enter the SDK"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let jobs = snapshot(&shared).jobs;
    ensure!(
        jobs.len() == 2
            && jobs[0].state == JobState::Transferring
            && jobs[1].state == JobState::Queued,
        "Expected one native upload in flight and one queued"
    );
    engine.command(Command::Cancel(jobs.iter().map(|job| job.id).collect()));
    let deadline = Instant::now() + Duration::from_secs(45);
    while engine.active.is_some() {
        engine.tick();
        if Instant::now() >= deadline {
            engine.shutdown(false)?;
            anyhow::bail!("Live upload cancellation timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    ensure!(
        snapshot(&shared).jobs.is_empty(),
        "Live upload cancellation failed"
    );
    ensure!(
        engine.remote.list(folder, 0, PAGE_SIZE)?.is_empty(),
        "Cancelled uploads left a file in the test folder"
    );
    Ok(())
}

fn collision_roundtrip(
    first: &mut Backend,
    second: &mut Backend,
    parent: u64,
    local: &Path,
) -> Result<()> {
    let left = local.join("left.bin");
    let right = local.join("right.bin");
    let original = [vec![0x31; 131_072], vec![0x42; 131_072]];
    fs::write(&left, &original[0])?;
    fs::write(&right, &original[1])?;
    second.load_drive()?;
    let location = first.upload_location(parent)?;
    Remote::upload(first, &left, parent, "race.bin", &location)?;
    Remote::upload(second, &right, parent, "race.bin", &location)?;
    for client in [&mut *first, &mut *second] {
        let code = client.wait_code(crate::operation::Stage::Uploading)?;
        ensure!(
            code == 0 || code == -12,
            "Unexpected collision outcome ({code})"
        );
    }
    first.load_drive()?;
    let rows = Remote::list(first, parent, 0, 32)?;
    ensure!(
        rows.len() == 2 && rows.iter().any(|e| e.name == "race.bin"),
        "Concurrent upload lost a node"
    );
    let mut received = Vec::new();
    for row in rows {
        let file = local.join(format!("received-{}", row.id));
        Remote::download(first, row.id, &file)?;
        first.wait()?;
        received.push(fs::read(&file)?);
    }
    received.sort();
    ensure!(received == original, "Concurrent uploads changed a payload");
    Ok(())
}

#[test]
#[ignore = "requires explicitly injected isolated live-account credentials; creates disposable fixtures"]
fn live_previews_and_grouped_actions() -> Result<()> {
    let user = Zeroizing::new(
        std::env::var("MEGAWREN_TEST_USER")
            .map_err(|_| anyhow::anyhow!("Live test user was not injected"))?,
    );
    let password = Zeroizing::new(
        std::env::var("MEGAWREN_TEST_PASSWORD")
            .map_err(|_| anyhow::anyhow!("Live test password was not injected"))?,
    );
    let scratch = tempfile::tempdir()?;
    let state = state::State::at(scratch.path().join("state"))?;
    let cancel = Arc::new(AtomicBool::new(false));
    let mut remote = Backend::new(&state, cancel)?;
    let mut random = [0u8; 8];
    getrandom::fill(&mut random)
        .map_err(|_| anyhow::anyhow!("Cannot create unique test fixture"))?;
    let name = format!(
        "megawren-preview-test-{}",
        random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let mut created = None;
    let result = (|| -> Result<()> {
        remote.login(&user, &password)?;
        remote.load_drive()?;
        let account = Remote::account_label(&mut remote)?;
        ensure!(
            account.contains('@') && account.matches('…').count() >= 2,
            "Live account identity was unavailable"
        );
        ensure!(
            !account.contains(user.as_str()) && !account.contains(user.split_once('@').unwrap().1),
            "Account label was not fully redacted"
        );
        let root = remote.api().root()?;
        remote.api().mkdir(root, &name)?;
        remote.wait()?;
        let folder = remote.api().result_node()?;
        created = Some(folder);
        cancel_uploads(&mut remote, folder, scratch.path())?;
        let text = scratch.path().join("notes.txt");
        std::fs::write(&text, b"live preview fixture\n".repeat(10_000))?;
        let image = scratch.path().join("image.png");
        let pixels = image::RgbImage::from_fn(160, 80, |x, y| {
            image::Rgb([(x % 256) as u8, (y * 3 % 256) as u8, 180])
        });
        image::DynamicImage::ImageRgb8(pixels)
            .write_to(&mut std::fs::File::create(&image)?, image::ImageFormat::Png)?;
        let mut entries = Vec::new();
        for (path, name) in [(&text, "notes.txt"), (&image, "image.png")] {
            let location = remote.upload_location(folder)?;
            Remote::upload(&mut remote, path, folder, name, &location)?;
            remote.wait()?;
            let id = remote.api().result_node()?;
            ensure!(
                remote.api().metadata(id)?.direct_link == ffi::LinkState::None,
                "A fresh private upload was labelled as linked"
            );
            let entry = Entry::from(remote.api().info(id)?);
            ensure!(
                !entry.exported
                    && Remote::list(&mut remote, folder, 0, 32)?
                        .iter()
                        .any(|e| e.id == id && !e.exported),
                "Browser reported a public link for a private fixture"
            );
            entries.push(entry);
        }
        for entry in &entries {
            let plan = crate::preview::plan(&entry.name, entry.size, false)?;
            let file = Remote::preview_file(&mut remote)?;
            Remote::preview(&mut remote, entry.id, &file, plan.bytes)?;
            let deadline = Instant::now() + Duration::from_secs(45);
            loop {
                let event = Remote::preview_event(&mut remote)?;
                if event.done {
                    ensure!(
                        event.code == 0,
                        "Live range preview failed ({})",
                        event.code
                    );
                    break;
                }
                ensure!(Instant::now() < deadline, "Live preview timed out");
                std::thread::sleep(Duration::from_millis(20));
            }
            ensure!(
                file.metadata()?.len() == plan.bytes as u64,
                "Preview exceeded its byte budget"
            );
            let content = crate::preview::decode(file, plan, Arc::new(AtomicBool::new(false)))?;
            match content {
                crate::preview::Content::Text { truncated, .. } => {
                    ensure!(truncated, "Text should be a bounded prefix")
                }
                crate::preview::Content::Image(image) => ensure!(
                    image.width() == 160 && image.height() == 80,
                    "Image preview dimensions changed"
                ),
            }
        }
        remote.api().mkdir(folder, "copies")?;
        remote.wait()?;
        let destination = remote.api().result_node()?;
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let selections: Vec<_> = entries
            .iter()
            .cloned()
            .map(|entry| Selection {
                entry,
                parent: folder,
            })
            .collect();
        {
            let mut engine = Engine::new(&mut remote, shared.clone());
            engine.command(Command::Change(Change::Many {
                selections: selections.clone(),
                action: BatchAction::Copy(destination),
            }));
            finish_change(&mut engine)?;
        }
        ensure!(
            Remote::list(&mut remote, destination, 0, PAGE_SIZE)?.len() == 2,
            "Grouped copy did not create both files"
        );
        {
            let mut engine = Engine::new(&mut remote, shared.clone());
            let reviewed = review(&mut engine, selections.clone())?;
            engine.command(Command::Change(Change::Trash { review: reviewed }));
            finish_change(&mut engine)?;
        }
        let bin = remote.api().rubbish()?;
        for entry in &entries {
            ensure!(
                remote.api().metadata(entry.id)?.in_rubbish,
                "Grouped bin move failed"
            );
        }
        {
            let mut engine = Engine::new(&mut remote, shared.clone());
            engine.command(Command::Change(Change::Many {
                selections: entries
                    .iter()
                    .cloned()
                    .map(|entry| Selection { entry, parent: bin })
                    .collect(),
                action: BatchAction::Restore,
            }));
            finish_change(&mut engine)?;
        }
        for entry in &entries {
            ensure!(
                remote.api().metadata(entry.id)?.parent == folder,
                "Grouped restore failed"
            );
        }
        // Exercise the shared CLI/browser walker and compare every payload byte.
        let tree = scratch.path().canonicalize()?.join("roundtrip");
        fs::create_dir_all(tree.join("nested"))?;
        fs::write(tree.join("empty"), [])?;
        fs::write(tree.join("nested/payload"), b"payload".repeat(1024))?;
        let companion = scratch.path().canonicalize()?.join("companion.txt");
        fs::write(&companion, b"companion")?;
        super::super::upload(
            &mut remote,
            vec![tree.clone(), companion.clone()],
            folder,
            name.clone(),
        )?;
        let folder_path = format!("/{name}/roundtrip");
        let companion_path = format!("/{name}/companion.txt");
        let paths = [folder_path.as_str(), companion_path.as_str()];
        let start = Instant::now();
        let uploaded = remote.download_entries(paths.into_iter())?;
        eprintln!("batch_lookup_us={}", start.elapsed().as_micros());
        let output = scratch.path().canonicalize()?.join("output");
        fs::create_dir(&output)?;
        super::super::download(&mut remote, uploaded, output.clone(), false)?;
        ensure!(
            fs::read(output.join("companion.txt"))? == fs::read(companion)?,
            "Companion file differed"
        );
        for rel in ["empty", "nested/payload"] {
            ensure!(
                fs::read(tree.join(rel))? == fs::read(output.join("roundtrip").join(rel))?,
                "Round trip verification failed"
            );
        }

        remote.api().mkdir(folder, "concurrent")?;
        remote.wait()?;
        let race = remote.api().result_node()?;
        let second_state = state::State::at(scratch.path().join("second-state"))?;
        let mut second = Backend::new(&second_state, Arc::new(AtomicBool::new(false)))?;
        second.login(&user, &password)?;
        let outcome = collision_roundtrip(&mut remote, &mut second, race, scratch.path());
        let logout = second.revoke_on_exit();
        let finish = second.finish();
        outcome?;
        logout?;
        finish?;
        checkpoint_after_folder_creation(&mut remote, folder, scratch.path())?;
        let root_selection = Selection {
            entry: remote.api().info(folder)?.into(),
            parent: root,
        };
        let mut engine = Engine::new(&mut remote, shared);
        let reviewed = review(&mut engine, vec![root_selection])?;
        engine.command(Command::Change(Change::Trash { review: reviewed }));
        finish_change(&mut engine)?;
        ensure!(
            engine.remote.api().metadata(folder)?.in_rubbish,
            "Fixture cleanup did not reach the bin"
        );
        created = None;
        Ok(())
    })();
    // Cleanup identifies only the random fixture folder created by this invocation.
    if let Some(id) = created
        && remote.api().info(id).is_ok_and(|entry| entry.name == name)
    {
        let epoch = remote.api().generation();
        if remote.api().change(4, id, 0, "", epoch).is_ok() {
            let _ = remote.wait();
        }
    }
    let revoke = if remote.authenticated {
        remote.revoke_on_exit()
    } else {
        Ok(())
    };
    let finish = remote.finish();
    result?;
    revoke?;
    finish?;
    Ok(())
}
