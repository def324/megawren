//! The mkdir, mv, cp, and rm commands. Each command resolves every path before
//! it changes anything. The native layer checks ownership, names, and conflicts
//! again when it submits the change, as it does for the file browser.
use crate::{backend::Backend, bridge::ffi, operation::Stage, state::display};
use anyhow::{Result, anyhow, bail, ensure};
use std::collections::HashSet;

// Actions understood by the native change functions.
const RENAME: u8 = 1;
const MOVE: u8 = 2;
const COPY: u8 = 3;
const TRASH: u8 = 4;
const NEW_FOLDER: u8 = 5;
const MOVE_AS: u8 = 7;
// The native layer accepts at most this many items per batch.
const BATCH: usize = 128;
// Removal checks folder contents within the same limits as the file browser.
const REVIEW_LIMIT: usize = 100_000;
const DEPTH_LIMIT: usize = 256;

/// Splits a remote path into its parent folder and final name. The root has
/// neither.
fn split(path: &str) -> Option<(&str, &str)> {
    let (parent, name) = path.trim_end_matches('/').rsplit_once('/')?;
    (!name.is_empty()).then_some((if parent.is_empty() { "/" } else { parent }, name))
}

fn join(folder: &str, name: &str) -> String {
    format!("{}/{name}", folder.trim_end_matches('/'))
}

#[derive(Debug, PartialEq)]
enum Destination {
    /// Into an existing folder, keeping each name.
    Into,
    /// To a new name in the destination's parent folder. Only for one source.
    As,
}

/// Treats the last argument of mv and cp like their Unix namesakes do, except
/// that an existing file is never replaced. `existing` says whether the path
/// names a folder, a file, or nothing.
fn destination(path: &str, sources: usize, existing: Option<bool>) -> Result<Destination> {
    match existing {
        Some(true) => Ok(Destination::Into),
        Some(false) => bail!("{} already exists", display(path)),
        None => {
            ensure!(
                sources == 1,
                "With two or more sources, the last path must be an existing remote folder"
            );
            ensure!(
                !path.ends_with('/'),
                "Remote folder not found: {}",
                display(path)
            );
            Ok(Destination::As)
        }
    }
}

/// Picks the native action that takes one source to a new name.
fn action(copy: bool, same_folder: bool, same_name: bool) -> u8 {
    match (copy, same_folder, same_name) {
        (true, ..) => COPY,
        (false, true, _) => RENAME,
        (false, false, true) => MOVE,
        (false, false, false) => MOVE_AS,
    }
}

fn reason(code: i32) -> String {
    match code {
        -9 => "an item no longer exists".into(),
        -11 => "MEGA denied access".into(),
        -12 => "an item with that name already exists".into(),
        -17 | -24 => "MEGA storage is full".into(),
        _ => format!("MEGA request failed ({code})"),
    }
}

struct Item {
    entry: ffi::Entry,
    parent: u64,
    path: String,
}

fn folder(backend: &mut Backend, path: &str) -> Result<u64> {
    let id = backend.resolve(path)?;
    ensure!(
        backend.api().info(id)?.folder,
        "{} is not a folder",
        display(path)
    );
    Ok(id)
}

/// Like the file browser, Megawren changes only private items owned by this
/// account, outside any shared or linked folder.
fn private(backend: &mut Backend, id: u64, path: &str) -> Result<()> {
    if id == backend.api().root()? {
        return Ok(());
    }
    ensure!(
        !backend.api().metadata(id)?.restricted,
        "{} is shared or linked, or sits in a shared or linked folder. Megawren changes only private items.",
        display(path)
    );
    Ok(())
}

fn item(backend: &mut Backend, path: &str) -> Result<Item> {
    let (parent_path, name) =
        split(path).ok_or_else(|| anyhow!("Choose files or folders below /"))?;
    let parent = folder(backend, parent_path)?;
    let entry = backend
        .find(parent, name)?
        .ok_or_else(|| anyhow!("Remote item not found: {}", display(path)))?;
    private(backend, entry.id, path)?;
    Ok(Item {
        entry,
        parent,
        path: path.to_owned(),
    })
}

fn items(backend: &mut Backend, paths: &[String]) -> Result<Vec<Item>> {
    let mut seen = HashSet::new();
    let mut items = Vec::with_capacity(paths.len());
    for path in paths {
        let item = item(backend, path)?;
        ensure!(
            seen.insert(item.entry.id),
            "{} is listed twice",
            display(path)
        );
        items.push(item);
    }
    Ok(items)
}

/// Waits for one submitted change and explains a failure, counting the items
/// that a batch completed before it.
fn finish(
    backend: &mut Backend,
    stage: Stage,
    before: usize,
    batch: usize,
    total: usize,
) -> Result<()> {
    let code = backend.wait_code(stage)?;
    if code == 0 {
        return Ok(());
    }
    let detail = backend.api().errors()?;
    let reason = if detail.is_empty() {
        reason(code)
    } else {
        display(&detail)
    };
    if total > 1 {
        let completed = if batch > 1 {
            backend.api().event()?.bytes.clamp(0, batch as i64) as usize
        } else {
            0
        };
        bail!(
            "{stage} stopped after {} of {total} items: {reason}",
            before + completed
        );
    }
    bail!("{stage} failed: {reason}")
}

fn submit(
    backend: &mut Backend,
    kind: u8,
    stage: Stage,
    mut epoch: u64,
    sources: &[u64],
    parents: &[u64],
    names: &[String],
) -> Result<()> {
    let total = sources.len();
    for start in (0..total).step_by(BATCH) {
        let end = (start + BATCH).min(total);
        backend.check_generation(epoch)?;
        if end - start == 1 {
            backend
                .api()
                .change(kind, sources[start], parents[start], &names[start], epoch)?;
        } else {
            backend.api().change_many(
                kind,
                &sources[start..end],
                &parents[start..end],
                &names[start..end],
                epoch,
            )?;
        }
        finish(backend, stage, start, end - start, total)?;
        epoch = backend.api().generation();
    }
    Ok(())
}

fn create(backend: &mut Backend, parent: u64, parent_path: &str, name: &str) -> Result<u64> {
    private(backend, parent, parent_path)?;
    let epoch = backend.api().generation();
    submit(
        backend,
        NEW_FOLDER,
        Stage::CreatingFolder,
        epoch,
        &[parent],
        &[parent],
        &[name.to_owned()],
    )?;
    Ok(backend.api().result_node()?)
}

/// Creates each folder. With `parents`, creates missing parent folders too and
/// accepts folders that already exist.
pub fn mkdir(backend: &mut Backend, paths: &[String], parents: bool) -> Result<()> {
    for path in paths {
        let Some((parent_path, name)) = split(path) else {
            ensure!(parents, "/ already exists");
            continue;
        };
        if !parents {
            let parent = folder(backend, parent_path)?;
            ensure!(
                backend.find(parent, name)?.is_none(),
                "{} already exists",
                display(path)
            );
            create(backend, parent, parent_path, name)?;
            continue;
        }
        let mut id = backend.api().root()?;
        let mut walked = String::from("/");
        for part in path.split('/').filter(|p| !p.is_empty()) {
            let next = join(&walked, part);
            id = match backend.find(id, part)? {
                Some(entry) if entry.folder => entry.id,
                Some(_) => bail!("{} is a file", display(&next)),
                None => create(backend, id, &walked, part)?,
            };
            walked = next;
        }
    }
    Ok(())
}

/// Moves or copies the sources. An existing folder as the destination receives
/// them under their own names. A new name in an existing folder renames the
/// single source. Existing items are never replaced.
pub fn relocate(backend: &mut Backend, sources: &[String], to: &str, copy: bool) -> Result<()> {
    let epoch = backend.api().generation();
    let items = items(backend, sources)?;
    let stage = if copy { Stage::Copying } else { Stage::Moving };
    let root = backend.api().root()?;
    let (target, target_path, new_name) = match split(to) {
        None => (root, "/", None),
        Some((parent_path, name)) => {
            let parent = folder(backend, parent_path)?;
            let found = backend.find(parent, name)?;
            match destination(to, items.len(), found.as_ref().map(|e| e.folder))? {
                Destination::Into => (found.unwrap().id, to, None),
                Destination::As => (parent, parent_path, Some(name)),
            }
        }
    };
    private(backend, target, target_path)?;
    if let Some(name) = new_name {
        let item = &items[0];
        let kind = action(copy, item.parent == target, item.entry.name == name);
        return submit(
            backend,
            kind,
            stage,
            epoch,
            &[item.entry.id],
            &[target],
            &[name.to_owned()],
        );
    }
    let mut names = HashSet::new();
    for item in &items {
        ensure!(
            copy || item.parent != target,
            "{} is already in {}",
            display(&item.path),
            display(target_path)
        );
        ensure!(
            names.insert(item.entry.name.as_str()),
            "Two sources are named {}",
            display(&item.entry.name)
        );
        ensure!(
            backend.find(target, &item.entry.name)?.is_none(),
            "{} already exists",
            display(&join(target_path, &item.entry.name))
        );
    }
    let ids: Vec<u64> = items.iter().map(|i| i.entry.id).collect();
    let names: Vec<String> = items.iter().map(|i| i.entry.name.clone()).collect();
    submit(
        backend,
        if copy { COPY } else { MOVE },
        stage,
        epoch,
        &ids,
        &vec![target; ids.len()],
        &names,
    )
}

/// Refuses a folder whose contents include shared or linked items, as the file
/// browser does before it moves a folder to the Rubbish Bin.
fn contents_are_private(backend: &mut Backend, folder: u64, path: &str) -> Result<()> {
    let mut stack = vec![(folder, 0, 0)];
    let mut checked = 0usize;
    while let Some((id, offset, depth)) = stack.pop() {
        let rows = backend.children(id, offset)?;
        if rows.len() == 256 {
            stack.push((id, offset + rows.len(), depth));
        }
        for row in rows {
            ensure!(
                !row.shared && !row.exported,
                "{} contains shared or linked items. Remove those shares and links in MEGA first.",
                display(path)
            );
            checked += 1;
            ensure!(
                checked <= REVIEW_LIMIT,
                "{} holds more than 100,000 items, too many to check. Remove it in MEGA instead.",
                display(path)
            );
            if row.folder {
                ensure!(
                    depth < DEPTH_LIMIT && stack.len() < 8192,
                    "{} is nested too deeply to check. Remove it in MEGA instead.",
                    display(path)
                );
                stack.push((row.id, 0, depth + 1));
            }
        }
    }
    Ok(())
}

/// Moves each item to the Rubbish Bin. Folders need `recursive`.
pub fn rm(backend: &mut Backend, paths: &[String], recursive: bool) -> Result<()> {
    let epoch = backend.api().generation();
    let items = items(backend, paths)?;
    for item in &items {
        if item.entry.folder {
            ensure!(
                recursive,
                "{} is a folder. Use rm -r to move a folder and everything in it to the Rubbish Bin.",
                display(&item.path)
            );
            contents_are_private(backend, item.entry.id, &item.path)?;
        }
    }
    let ids: Vec<u64> = items.iter().map(|i| i.entry.id).collect();
    submit(
        backend,
        TRASH,
        Stage::Trashing,
        epoch,
        &ids,
        &vec![0; ids.len()],
        &vec![String::new(); ids.len()],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_split_into_parent_and_name_and_the_root_has_neither() {
        assert_eq!(split("/Photos"), Some(("/", "Photos")));
        assert_eq!(split("/Photos/2026/"), Some(("/Photos", "2026")));
        assert_eq!(split("/a/b/c"), Some(("/a/b", "c")));
        assert_eq!(split("/"), None);
        assert_eq!(split("//"), None);
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("/Reports/", "a"), "/Reports/a");
    }

    #[test]
    fn destinations_follow_unix_rules_but_never_replace_a_file() {
        assert_eq!(
            destination("/Archive", 3, Some(true)).unwrap(),
            Destination::Into
        );
        assert_eq!(destination("/new.txt", 1, None).unwrap(), Destination::As);
        let error = |path, sources, existing| {
            destination(path, sources, existing)
                .unwrap_err()
                .to_string()
        };
        assert!(error("/a.txt", 1, Some(false)).contains("already exists"));
        assert!(error("/missing", 2, None).contains("existing remote folder"));
        assert!(error("/missing/", 1, None).contains("Remote folder not found"));
    }

    #[test]
    fn one_source_with_a_new_name_uses_a_single_native_action() {
        assert_eq!(action(true, true, false), COPY);
        assert_eq!(action(true, false, true), COPY);
        assert_eq!(action(false, true, false), RENAME);
        assert_eq!(action(false, false, true), MOVE);
        assert_eq!(action(false, false, false), MOVE_AS);
    }

    #[test]
    fn failures_name_the_problem_instead_of_a_bare_code() {
        assert_eq!(reason(-12), "an item with that name already exists");
        assert_eq!(reason(-17), "MEGA storage is full");
        assert_eq!(reason(-999), "MEGA request failed (-999)");
    }

    #[test]
    #[ignore = "requires explicitly injected isolated live-account credentials; creates disposable fixtures"]
    fn live_command_line_file_management() -> Result<()> {
        use crate::state::State;
        use std::sync::{Arc, atomic::AtomicBool};
        use zeroize::Zeroizing;
        let user = Zeroizing::new(
            std::env::var("MEGAWREN_TEST_USER")
                .map_err(|_| anyhow!("Live test user was not injected"))?,
        );
        let password = Zeroizing::new(
            std::env::var("MEGAWREN_TEST_PASSWORD")
                .map_err(|_| anyhow!("Live test password was not injected"))?,
        );
        let scratch = tempfile::tempdir()?;
        let state = State::at(scratch.path().join("state"))?;
        let mut backend = Backend::new(&state, Arc::new(AtomicBool::new(false)))?;
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).map_err(|_| anyhow!("Cannot create unique test fixture"))?;
        let name = format!(
            "megawren-cli-test-{}",
            random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let base = format!("/{name}");
        let result = (|| -> Result<()> {
            backend.login(&user, &password)?;
            backend.load_drive()?;
            let p = |s: &str| format!("{base}{s}");
            let ps = |list: &[&str]| list.iter().map(|s| p(s)).collect::<Vec<_>>();
            let fails = |result: Result<()>, text: &str| -> Result<()> {
                let error = result
                    .err()
                    .ok_or_else(|| anyhow!("expected an error with {text:?}"))?;
                ensure!(
                    error.to_string().contains(text),
                    "unexpected error: {error}"
                );
                Ok(())
            };
            mkdir(&mut backend, &ps(&["/a/b"]), true)?;
            mkdir(&mut backend, &ps(&["/a/b"]), true)?;
            fails(mkdir(&mut backend, &ps(&["/a"]), false), "already exists")?;
            mkdir(&mut backend, &ps(&["/x", "/y"]), false)?;
            relocate(&mut backend, &ps(&["/x"]), &p("/a"), true)?;
            relocate(&mut backend, &ps(&["/x"]), &p("/x2"), true)?;
            relocate(&mut backend, &ps(&["/x"]), &p("/x3"), false)?;
            relocate(&mut backend, &ps(&["/x3"]), &p("/a/b/x4"), false)?;
            relocate(&mut backend, &ps(&["/x2", "/y"]), &p("/a/b"), false)?;
            relocate(&mut backend, &ps(&["/a/x", "/a/b/x4"]), &base, true)?;
            for path in ["/a/b", "/a/x", "/a/b/x4", "/a/b/x2", "/a/b/y", "/x", "/x4"] {
                ensure!(backend.resolve(&p(path)).is_ok(), "{path} is missing");
            }
            for path in ["/x2", "/x3", "/y"] {
                ensure!(backend.resolve(&p(path)).is_err(), "{path} is still there");
            }
            fails(
                relocate(&mut backend, &ps(&["/x"]), &p("/a"), true),
                "already exists",
            )?;
            fails(
                relocate(&mut backend, &ps(&["/x", "/x4"]), &p("/missing"), false),
                "existing remote folder",
            )?;
            fails(
                relocate(&mut backend, &ps(&["/x", "/x"]), &p("/a"), false),
                "listed twice",
            )?;
            fails(
                relocate(&mut backend, &ps(&["/a/x"]), &p("/a"), false),
                "already in",
            )?;
            ensure!(
                relocate(&mut backend, &ps(&["/a"]), &p("/a/b"), false).is_err(),
                "a folder moved into itself"
            );
            fails(rm(&mut backend, &ps(&["/a"]), false), "rm -r")?;
            rm(&mut backend, &ps(&["/a", "/x"]), true)?;
            for path in ["/a", "/x"] {
                ensure!(backend.resolve(&p(path)).is_err(), "{path} was not removed");
            }
            ensure!(backend.resolve(&p("/x4")).is_ok(), "rm removed too much");
            rm(&mut backend, std::slice::from_ref(&base), true)?;
            Ok(())
        })();
        // Cleanup touches only the random fixture folder created by this run.
        if result.is_err()
            && let Ok(root) = backend.api().root()
            && let Ok(Some(entry)) = backend.find(root, &name)
        {
            let epoch = backend.api().generation();
            if backend.api().change(TRASH, entry.id, 0, "", epoch).is_ok() {
                let _ = backend.wait();
            }
        }
        let revoke = if backend.authenticated {
            backend.revoke_on_exit()
        } else {
            Ok(())
        };
        let finish = backend.finish();
        result?;
        revoke?;
        finish
    }
}
