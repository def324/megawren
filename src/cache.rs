//! Encrypted snapshots of closed SDK databases. Plaintext exists only in a private
//! per-process working directory. No transfer database or file contents are saved.
use crate::{
    bridge::ffi,
    input,
    operation::{Stage, Status},
    state,
};
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Instant,
};
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"MGWCACH\x01";
const COMPRESSED_MAGIC: &[u8; 8] = b"MGWCACH\x03";
const FORMAT_RECORD: &[u8] = b"\x00zstd";
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Original,
    Zstd,
}
impl Format {
    pub fn configured() -> Self {
        if std::env::var_os("MEGAWREN_COMPRESS_CACHE").as_deref() == Some(std::ffi::OsStr::new("0"))
        {
            Self::Original
        } else {
            Self::Zstd
        }
    }
}
const CHUNK: usize = 256 * 1024;
const OVERHEAD: usize = 17; // libsodium secretstream authentication + encrypted tag
const MAX_FILES: usize = 128;
const MAX_FILE: u64 = 16 * 1024 * 1024 * 1024;
const MAX_TOTAL: u64 = 64 * 1024 * 1024 * 1024;

pub fn key(session: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    Ok(Zeroizing::new(ffi::cache_key(session)?))
}

// Only SDK account node/state/status SQLite files and their sidecars. In particular,
// never retain transfer state, arbitrary paths, or downloaded/uploaded contents.
fn database_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("megaclient_statecache") else {
        return false;
    };
    let Some((version, rest)) = rest.split_once('_') else {
        return false;
    };
    if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let rest = rest.strip_prefix("status_").unwrap_or(rest);
    let Some((handle, suffix)) = rest.split_once(".db") else {
        return false;
    };
    !handle.is_empty()
        && handle.len() <= 128
        && handle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && !handle.starts_with("transfers_")
        && matches!(suffix, "" | "-wal" | "-shm" | "-journal")
}

fn open_file(path: &Path, create: bool, private: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(!create).write(create).create_new(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    ensure!(meta.is_file(), "Invalid local index file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            (!private || meta.mode() & 0o077 == 0) && meta.uid() == unsafe { libc::geteuid() },
            "Local index must be private"
        );
    }
    Ok(file)
}

struct Progress<'a> {
    cancel: &'a AtomicBool,
    status: Status<std::io::Stderr>,
    done: u64,
    total: u64,
}
impl<'a> Progress<'a> {
    fn new(cancel: &'a AtomicBool, stage: Stage, total: u64) -> Self {
        Self {
            cancel,
            status: Status::new(stage, true),
            done: 0,
            total,
        }
    }
    fn advance(&mut self, count: u64) -> Result<()> {
        input::check(self.cancel)?;
        self.done += count;
        self.status.show(
            &ffi::Event {
                done: false,
                code: 0,
                bytes: self.done as i64,
                total: self.total as i64,
                retry: 0,
            },
            false,
            Instant::now(),
        );
        Ok(())
    }
}

fn write_record(
    output: &mut File,
    cipher: &mut cxx::UniquePtr<ffi::CacheCipher>,
    data: &[u8],
    last: bool,
) -> Result<()> {
    let mut encrypted = vec![0u8; data.len() + OVERHEAD];
    cipher.pin_mut().encrypt(data, &mut encrypted, last)?;
    output.write_all(&(encrypted.len() as u32).to_le_bytes())?;
    output.write_all(&encrypted)?;
    Ok(())
}

fn read_record(
    input: &mut File,
    cipher: &mut cxx::UniquePtr<ffi::CacheCipher>,
    progress: &mut Progress<'_>,
) -> Result<(Zeroizing<Vec<u8>>, bool)> {
    let mut size = [0u8; 4];
    input.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    ensure!(
        (OVERHEAD..=CHUNK + 1 + OVERHEAD).contains(&size),
        "Invalid local index chunk size"
    );
    let mut encrypted = vec![0u8; size];
    input.read_exact(&mut encrypted)?;
    let mut plain = Zeroizing::new(vec![0u8; size - OVERHEAD]);
    let last = cipher.pin_mut().decrypt(&encrypted, &mut plain)?;
    progress.advance(size as u64 + 4)?;
    Ok((plain, last))
}

/// Restores into a new private directory. No restored file is handed to the SDK
/// until every record and the final tag have authenticated and EOF is verified.
// The restored plaintext is private to this process. Include inode and change
// time so replacing a database or restoring an old mtime requires resealing.
#[derive(PartialEq, Eq)]
pub struct Stamp(Vec<(String, u64, u64, i64, i64)>);
pub fn stamp(run: &Path) -> Result<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let mut files = Vec::new();
    for e in fs::read_dir(run)? {
        let e = e?;
        let name = e.file_name();
        let Some(name) = name.to_str().filter(|s| database_name(s)) else {
            continue;
        };
        let m = fs::symlink_metadata(e.path())?;
        ensure!(
            m.is_file() && files.len() < MAX_FILES,
            "Invalid SDK database set"
        );
        files.push((name.into(), m.ino(), m.len(), m.ctime(), m.ctime_nsec()));
    }
    files.sort_unstable();
    Ok(Stamp(files))
}
pub fn restore(
    path: &Path,
    key: &[u8],
    cancel: &AtomicBool,
) -> Result<Option<(state::Run, Format)>> {
    if !path.try_exists()? {
        return Ok(None);
    }
    let mut input = open_file(path, false, true)?;
    let total = input.metadata()?.len();
    ensure!(
        total <= MAX_TOTAL + MAX_TOTAL / CHUNK as u64 * 32 + 65536,
        "Local index is too large"
    );
    let mut magic = [0u8; 8];
    let mut header = [0u8; 24];
    input.read_exact(&mut magic)?;
    let format = match &magic {
        m if m == MAGIC => Format::Original,
        m if m == COMPRESSED_MAGIC => Format::Zstd,
        _ => anyhow::bail!("Unsupported local index"),
    };
    input.read_exact(&mut header)?;
    let mut cipher = ffi::cache_cipher(key, &header)?;
    let root = path.parent().context("Invalid local index path")?;
    let restored = state::Run::new(root)?;
    let mut progress = Progress::new(cancel, Stage::ReadingIndex, total);
    progress.advance(32)?;
    // Bind the outer version byte to an authenticated record.
    if format == Format::Zstd {
        let (marker, last) = read_record(&mut input, &mut cipher, &mut progress)?;
        ensure!(
            !last && marker.as_slice() == FORMAT_RECORD,
            "Invalid local index format"
        );
    }
    let mut decoder = if format == Format::Zstd {
        Some((
            zstd::bulk::Decompressor::new()?,
            Zeroizing::new(vec![0u8; CHUNK]),
        ))
    } else {
        None
    };
    let mut files = 0;
    let mut bytes = 0;
    loop {
        let (record, final_chunk) = read_record(&mut input, &mut cipher, &mut progress)?;
        if final_chunk {
            ensure!(record.is_empty(), "Invalid local index terminator");
            let mut extra = [0u8; 1];
            ensure!(input.read(&mut extra)? == 0, "Trailing local index data");
            break;
        }
        files += 1;
        ensure!(
            files <= MAX_FILES && record.len() >= 11 && record[0] == 1,
            "Invalid local index entry"
        );
        let name_len = u16::from_le_bytes(record[1..3].try_into().unwrap()) as usize;
        let size = u64::from_le_bytes(record[3..11].try_into().unwrap());
        ensure!(
            record.len() == 11 + name_len && name_len <= 255 && size <= MAX_FILE,
            "Invalid local index entry size"
        );
        bytes += size;
        ensure!(bytes <= MAX_TOTAL, "Local index is too large");
        let name = std::str::from_utf8(&record[11..])?;
        ensure!(database_name(name), "Invalid local index filename");
        let mut output = open_file(&restored.path().join(name), true, true)?;
        let mut remaining = size;
        while remaining > 0 {
            let (data, last) = read_record(&mut input, &mut cipher, &mut progress)?;
            ensure!(!last && data.len() > 1, "Invalid local index data");
            let payload = &data[1..];
            let count = match data[0] {
                2 => {
                    ensure!(
                        payload.len() as u64 <= remaining,
                        "Local index entry overflow"
                    );
                    output.write_all(payload)?;
                    payload.len() as u64
                }
                3 if format == Format::Zstd => {
                    let (codec, plain) = decoder.as_mut().context("Missing index decoder")?;
                    let expected = remaining.min(CHUNK as u64) as usize;
                    ensure!(
                        zstd::zstd_safe::find_frame_compressed_size(payload) == Ok(payload.len())
                            && matches!(zstd::zstd_safe::get_frame_content_size(payload), Ok(Some(n)) if n == expected as u64),
                        "Invalid compressed index frame"
                    );
                    let written = codec.decompress_to_buffer(payload, &mut plain[..expected])?;
                    ensure!(written == expected, "Invalid compressed index block size");
                    output.write_all(&plain[..expected])?;
                    expected as u64
                }
                _ => anyhow::bail!("Invalid local index block"),
            };
            remaining -= count;
        }
    }
    Ok(Some((restored, format)))
}

/// Called only after the SDK has closed all database connections.
pub fn save(path: &Path, run: &Path, key: &[u8], cancel: &AtomicBool) -> Result<()> {
    save_inner(path, run, key, cancel, false, Format::configured())
}

/// Work cancellation has already stopped SDK activity. Keep its flag set while
/// finishing local cleanup; the signal handler still forces exit on a second
/// Ctrl-C. A separate token avoids treating the first interrupt as a request to
/// discard an already complete index.
pub fn save_after_interrupt(path: &Path, run: &Path, key: &[u8]) -> Result<()> {
    save_inner(
        path,
        run,
        key,
        &AtomicBool::new(false),
        true,
        Format::configured(),
    )
}

pub(crate) fn save_inner(
    path: &Path,
    run: &Path,
    key: &[u8],
    cancel: &AtomicBool,
    immediate: bool,
    format: Format,
) -> Result<()> {
    input::check(cancel)?;
    let mut files: Vec<(PathBuf, String, u64)> = Vec::new();
    let mut total = 0;
    for entry in fs::read_dir(run)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !database_name(&name) {
            continue;
        }
        let meta = fs::symlink_metadata(entry.path())?;
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "Invalid SDK database file"
        );
        ensure!(meta.len() <= MAX_FILE, "SDK database is too large");
        total += meta.len();
        ensure!(
            total <= MAX_TOTAL && files.len() < MAX_FILES,
            "SDK database limits exceeded"
        );
        files.push((entry.path(), name, meta.len()));
    }
    if files.is_empty() {
        return Ok(());
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    let root = path.parent().context("Invalid local index path")?;
    let mut output = tempfile::NamedTempFile::new_in(root)?;
    let mut cipher = ffi::cache_cipher(key, &[])?;
    output.write_all(if format == Format::Zstd {
        COMPRESSED_MAGIC
    } else {
        MAGIC
    })?;
    output.write_all(&cipher.header())?;
    if format == Format::Zstd {
        write_record(output.as_file_mut(), &mut cipher, FORMAT_RECORD, false)?;
    }
    let mut encoder = if format == Format::Zstd {
        let capacity = zstd::zstd_safe::compress_bound(CHUNK);
        let mut encoded = Zeroizing::new(vec![0; capacity + 1]);
        encoded[0] = 3;
        Some((zstd::bulk::Compressor::new(1)?, encoded))
    } else {
        None
    };
    let mut progress = Progress::new(cancel, Stage::SavingIndex, total);
    if immediate {
        progress.status = progress.status.immediate();
    }
    progress.advance(0)?;
    let mut buffer = Zeroizing::new(vec![0u8; CHUNK + 1]);
    buffer[0] = 2;
    for (file, name, size) in files {
        // SDK-created databases may be 0644, inside the private 0700 run directory.
        let mut input = open_file(&file, false, false)?;
        ensure!(
            input.metadata()?.len() == size,
            "SDK database changed during snapshot"
        );
        let mut record = Zeroizing::new(vec![1u8]);
        record.extend_from_slice(&(name.len() as u16).to_le_bytes());
        record.extend_from_slice(&size.to_le_bytes());
        record.extend_from_slice(name.as_bytes());
        write_record(output.as_file_mut(), &mut cipher, &record, false)?;
        let mut remaining = size;
        while remaining > 0 {
            progress.advance(0)?;
            let count = remaining.min(CHUNK as u64) as usize;
            input.read_exact(&mut buffer[1..=count])?;
            let mut record = &buffer[..=count];
            if let Some((codec, encoded)) = &mut encoder {
                let size = codec.compress_to_buffer(&buffer[1..=count], &mut encoded[1..])?;
                if size < count {
                    record = &encoded[..=size];
                }
            }
            write_record(output.as_file_mut(), &mut cipher, record, false)?;
            remaining -= count as u64;
            progress.advance(count as u64)?;
        }
    }
    write_record(output.as_file_mut(), &mut cipher, &[], true)?;
    output.as_file().sync_all()?;
    input::check(cancel)?;
    output.persist(path).map_err(|e| e.error)?;
    File::open(root)?.sync_all()?;
    Ok(())
}

pub fn remove(root: &Path) -> Result<()> {
    match fs::remove_file(root.join("cache.enc")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    const DB: &str = "megaclient_statecache20_account-id.db";
    const STATUS: &str = "megaclient_statecache20_status_account-id.db";

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, Zeroizing<Vec<u8>>) {
        let root = tempfile::tempdir().unwrap();
        let run = root.path().join("source");
        fs::create_dir(&run).unwrap();
        let cache = root.path().join("cache.enc");
        let key = key(b"synthetic test session, never an account credential").unwrap();
        (root, run, cache, key)
    }

    #[test]
    fn streams_multiple_chunks_and_only_account_databases_with_private_permissions() {
        let (_root, run, cache, key) = fixture();
        let cancel = AtomicBool::new(false);
        let mut data = vec![0x53; CHUNK * 3 + 7];
        let marker = b"a private filename.txt!!";
        data[..marker.len()].copy_from_slice(marker);
        fs::write(run.join(DB), &data).unwrap();
        fs::write(run.join(STATUS), []).unwrap();
        fs::write(
            run.join("megaclient_statecache20_transfers_account-id.db"),
            b"unwanted transfer",
        )
        .unwrap();
        fs::write(run.join("other-file"), b"not a database").unwrap();
        save(&cache, &run, &key, &cancel).unwrap();
        let first = fs::read(&cache).unwrap();
        assert!(!first.windows(DB.len()).any(|s| s == DB.as_bytes()));
        assert!(!first.windows(marker.len()).any(|s| s == marker));
        let (restored, _) = restore(&cache, &key, &cancel).unwrap().unwrap();
        assert_eq!(fs::read(restored.path().join(DB)).unwrap(), data);
        assert_eq!(fs::metadata(restored.path().join(STATUS)).unwrap().len(), 0);
        assert!(restored.path().join(".megawren-run-v1").is_file());
        assert_eq!(fs::read_dir(restored.path()).unwrap().count(), 3);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(fs::metadata(&cache).unwrap().mode() & 0o777, 0o600);
            assert_eq!(fs::metadata(restored.path()).unwrap().mode() & 0o777, 0o700);
            assert_eq!(
                fs::metadata(restored.path().join(DB)).unwrap().mode() & 0o777,
                0o600
            );
        }
        save(&cache, &run, &key, &cancel).unwrap();
        assert_ne!(
            first,
            fs::read(&cache).unwrap(),
            "fresh stream headers randomize each snapshot"
        );
    }

    #[test]
    fn rejects_corruption_wrong_key_truncation_reordering_and_trailing_data_without_leaks() {
        let (root, run, cache, key) = fixture();
        let cancel = AtomicBool::new(false);
        fs::write(run.join(DB), vec![0x54; CHUNK + 42]).unwrap();
        save(&cache, &run, &key, &cancel).unwrap();
        let good = fs::read(&cache).unwrap();
        let count = fs::read_dir(root.path()).unwrap().count();
        let other_key = super::key(b"different synthetic session").unwrap();
        assert!(restore(&cache, &other_key, &cancel).is_err());
        let mut variants = Vec::new();
        for n in [0, 8, 31, 32, good.len() - 1, good.len() - 21] {
            variants.push(good[..n].to_vec());
        }
        for n in [0, 8, 31, 40, good.len() / 2, good.len() - 1] {
            let mut bad = good.clone();
            bad[n] ^= 1;
            variants.push(bad);
        }
        let mut trailing = good.clone();
        trailing.push(0);
        variants.push(trailing);
        let record_len = |offset: usize| {
            4 + u32::from_le_bytes(good[offset..offset + 4].try_into().unwrap()) as usize
        };
        let first = 32;
        let second = first + record_len(first);
        let third = second + record_len(second);
        let fourth = third + record_len(third);
        let mut reordered = good[..second].to_vec();
        reordered.extend_from_slice(&good[third..fourth]);
        reordered.extend_from_slice(&good[second..third]);
        reordered.extend_from_slice(&good[fourth..]);
        variants.push(reordered);
        let mut huge_chunk = good.clone();
        huge_chunk[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
        variants.push(huge_chunk);
        for bad in variants {
            fs::write(&cache, bad).unwrap();
            assert!(restore(&cache, &key, &cancel).is_err());
            assert_eq!(
                fs::read_dir(root.path()).unwrap().count(),
                count,
                "failed restore leaked plaintext"
            );
        }
    }

    #[test]
    fn zstd_and_raw_blocks_roundtrip_without_changing_database_bytes() {
        let (_root, run, cache, key) = fixture();
        let cancel = AtomicBool::new(false);
        let mut data = vec![0; 2 * CHUNK + 777];
        getrandom::fill(&mut data[CHUNK..2 * CHUNK]).unwrap();
        fs::write(run.join(DB), &data).unwrap();
        save_inner(&cache, &run, &key, &cancel, false, Format::Original).unwrap();
        let (old, format) = restore(&cache, &key, &cancel).unwrap().unwrap();
        assert_eq!(format, Format::Original);
        save_inner(&cache, old.path(), &key, &cancel, false, Format::Zstd).unwrap();
        let mut input = File::open(&cache).unwrap();
        let mut header = [0; 32];
        input.read_exact(&mut header).unwrap();
        assert_eq!(&header[..8], b"MGWCACH\x03");
        let mut cipher = ffi::cache_cipher(&key, &header[8..]).unwrap();
        let mut progress = Progress::new(
            &cancel,
            Stage::ReadingIndex,
            input.metadata().unwrap().len(),
        );
        let (marker, _) = read_record(&mut input, &mut cipher, &mut progress).unwrap();
        assert_eq!(marker.as_slice(), b"\0zstd");
        let _file_header = read_record(&mut input, &mut cipher, &mut progress).unwrap();
        let (packed, _) = read_record(&mut input, &mut cipher, &mut progress).unwrap();
        assert_eq!(packed[0], 3);
        assert_eq!(
            zstd::bulk::decompress(&packed[1..], CHUNK).unwrap(),
            data[..CHUNK]
        );
        let (raw, _) = read_record(&mut input, &mut cipher, &mut progress).unwrap();
        assert_eq!(raw[0], 2);
        assert!(fs::metadata(&cache).unwrap().len() < CHUNK as u64 + 8192);
        let (new, format) = restore(&cache, &key, &cancel).unwrap().unwrap();
        assert_eq!(format, Format::Zstd);
        assert_eq!(fs::read(new.path().join(DB)).unwrap(), data);
        let mut changed_version = fs::read(&cache).unwrap();
        changed_version[..8].copy_from_slice(MAGIC);
        fs::write(&cache, &changed_version).unwrap();
        assert!(restore(&cache, &key, &cancel).is_err());
        save_inner(&cache, new.path(), &key, &cancel, false, Format::Original).unwrap();
        let (old, format) = restore(&cache, &key, &cancel).unwrap().unwrap();
        assert_eq!(format, Format::Original);
        assert_eq!(fs::read(old.path().join(DB)).unwrap(), data);
    }

    #[test]
    fn compressed_blocks_must_consume_the_payload_and_match_the_bounded_output() {
        let (root, _run, cache, key) = fixture();
        let cancel = AtomicBool::new(false);
        let packed = zstd::bulk::compress(&[7; 4096], 1).unwrap();
        let before = fs::read_dir(root.path()).unwrap().count();
        for (size, extra) in [(1024u64, false), (8192, false), (4096, true)] {
            let mut file = open_file(&cache, true, true).unwrap();
            let mut cipher = ffi::cache_cipher(&key, &[]).unwrap();
            file.write_all(COMPRESSED_MAGIC).unwrap();
            file.write_all(&cipher.header()).unwrap();
            write_record(&mut file, &mut cipher, FORMAT_RECORD, false).unwrap();
            let mut header = vec![1];
            header.extend_from_slice(&(DB.len() as u16).to_le_bytes());
            header.extend_from_slice(&size.to_le_bytes());
            header.extend_from_slice(DB.as_bytes());
            write_record(&mut file, &mut cipher, &header, false).unwrap();
            let mut record = vec![3];
            record.extend_from_slice(&packed);
            if extra {
                record.push(0);
            }
            write_record(&mut file, &mut cipher, &record, false).unwrap();
            write_record(&mut file, &mut cipher, &[], true).unwrap();
            drop(file);
            assert!(restore(&cache, &key, &cancel).is_err());
            fs::remove_file(&cache).unwrap();
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), before);
        }
    }

    fn forged(path: &Path, key: &[u8], entries: &[(&str, u64)]) {
        let mut file = open_file(path, true, true).unwrap();
        let mut cipher = ffi::cache_cipher(key, &[]).unwrap();
        file.write_all(MAGIC).unwrap();
        file.write_all(&cipher.header()).unwrap();
        for (name, size) in entries {
            let mut header = vec![1];
            header.extend_from_slice(&(name.len() as u16).to_le_bytes());
            header.extend_from_slice(&size.to_le_bytes());
            header.extend_from_slice(name.as_bytes());
            write_record(&mut file, &mut cipher, &header, false).unwrap();
        }
        write_record(&mut file, &mut cipher, &[], true).unwrap();
    }

    #[test]
    fn rejects_authenticated_unsafe_names_duplicate_entries_and_resource_limits() {
        let (_root, _run, cache, key) = fixture();
        let cancel = AtomicBool::new(false);
        for entries in [
            vec![("../escape.db", 0)],
            vec![(DB, MAX_FILE + 1)],
            vec![(DB, 0), (DB, 0)],
            vec![("megaclient_statecache20_transfers_account-id.db", 0)],
        ] {
            forged(&cache, &key, &entries);
            assert!(restore(&cache, &key, &cancel).is_err());
            fs::remove_file(&cache).unwrap();
        }
        let names: Vec<_> = (0..=MAX_FILES)
            .map(|n| format!("megaclient_statecache20_account{n}.db"))
            .collect();
        let entries: Vec<_> = names.iter().map(|n| (n.as_str(), 0)).collect();
        forged(&cache, &key, &entries);
        assert!(restore(&cache, &key, &cancel).is_err());
    }

    #[test]
    fn cancellation_preserves_previous_snapshot_and_cleans_restored_plaintext() {
        let (root, run, cache, key) = fixture();
        let cancel = AtomicBool::new(false);
        fs::write(run.join(DB), b"original").unwrap();
        save(&cache, &run, &key, &cancel).unwrap();
        let previous = fs::read(&cache).unwrap();
        let count = fs::read_dir(root.path()).unwrap().count();
        cancel.store(true, Ordering::Relaxed);
        assert!(
            save(&cache, &run, &key, &cancel)
                .unwrap_err()
                .is::<input::Cancelled>()
        );
        assert!(
            restore(&cache, &key, &cancel)
                .unwrap_err()
                .is::<input::Cancelled>()
        );
        assert_eq!(fs::read(&cache).unwrap(), previous);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), count);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_and_public_cache_permissions_preserving_existing_snapshot() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let (root, run, cache, key) = fixture();
        let cancel = AtomicBool::new(false);
        fs::write(run.join(DB), b"original").unwrap();
        save(&cache, &run, &key, &cancel).unwrap();
        let previous = fs::read(&cache).unwrap();
        let link = root.path().join("link.enc");
        symlink(&cache, &link).unwrap();
        assert!(restore(&link, &key, &cancel).is_err());
        symlink(run.join(DB), run.join(STATUS)).unwrap();
        assert!(save(&cache, &run, &key, &cancel).is_err());
        assert_eq!(fs::read(&cache).unwrap(), previous);
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(restore(&cache, &key, &cancel).is_err());
    }
}
