#![forbid(unsafe_code)]
#![cfg(not(target_arch = "wasm32"))]

use std::{
    fs,
    path::{Path, PathBuf},
};

mod kithara {
    pub(crate) use kithara_test_macros::test;
}

use kithara_bufpool::BytePool;
use kithara_platform::{CancelToken, time::Duration};
use tempfile::TempDir;

use super::core::{AtomicChunked, OpenIntent, make_tmp_path};
use crate::{MmapDriver, MmapOptions, OpenMode, Resource, StorageResult};

fn open_chunked(dir: &TempDir, name: &str) -> (AtomicChunked<MmapDriver>, PathBuf, PathBuf) {
    let canonical = dir.path().join(name);
    let res = open_chunked_result(dir, name).unwrap();
    let tmp = make_tmp_path(&canonical).unwrap();
    (res, canonical, tmp)
}

fn open_chunked_result(dir: &TempDir, name: &str) -> StorageResult<AtomicChunked<MmapDriver>> {
    let canonical = dir.path().join(name);
    let cancel = CancelToken::never();
    AtomicChunked::<MmapDriver>::open(canonical, move |target, intent| {
        let mode = match intent {
            OpenIntent::Fresh => OpenMode::ReadWrite,
            OpenIntent::Reopen => OpenMode::ReadOnly,
        };
        Resource::open(
            cancel.clone(),
            MmapOptions {
                mode,
                initial_len: None,
                path: target.to_path_buf(),
            },
        )
    })
}

#[kithara::test(timeout(Duration::from_secs(2)))]
fn canonical_invisible_until_commit() {
    let dir = TempDir::new().unwrap();
    let (res, canonical, tmp) = open_chunked(&dir, "seg.bin");

    res.write_at(0, b"chunk-1-").unwrap();
    res.write_at(8, b"chunk-2!").unwrap();

    assert!(
        !canonical.exists(),
        "canonical must not exist before commit"
    );
    assert!(tmp.exists(), "tmp file must hold in-flight bytes");

    res.commit(Some(16)).unwrap();
    assert!(canonical.exists(), "canonical materialised on commit");
    assert!(!tmp.exists(), "tmp consumed by atomic rename");

    let bytes = fs::read(&canonical).unwrap();
    assert_eq!(&bytes, b"chunk-1-chunk-2!");
}

#[kithara::test(timeout(Duration::from_secs(2)))]
fn drop_without_commit_cleans_tmp() {
    let dir = TempDir::new().unwrap();
    let (res, canonical, tmp) = open_chunked(&dir, "abandoned.bin");

    res.write_at(0, b"will-not-commit").unwrap();
    assert!(tmp.exists());
    drop(res);

    assert!(!tmp.exists(), "Drop must remove the orphaned tmp");
    assert!(!canonical.exists(), "canonical must never appear");
}

#[kithara::test(timeout(Duration::from_secs(2)))]
fn fail_cleans_tmp() {
    let dir = TempDir::new().unwrap();
    let (res, canonical, tmp) = open_chunked(&dir, "failed.bin");

    res.write_at(0, b"oops").unwrap();
    res.fail("test".into());

    assert!(!tmp.exists(), "fail() must remove the tmp");
    assert!(!canonical.exists());
}

/// A tmp nobody holds is an orphan: its owner is gone, so nothing will ever
/// commit or clean it. Rejecting it would make the canonical path
/// permanently unwritable — the segment stays unfetchable for good.
#[kithara::test(timeout(Duration::from_secs(2)))]
fn open_reclaims_a_stale_tmp_no_live_writer_holds() {
    let dir = TempDir::new().unwrap();
    let stale_tmp = make_tmp_path(&dir.path().join("survivor.bin")).unwrap();
    fs::write(&stale_tmp, b"stale-from-previous-process").unwrap();

    let (res, canonical, tmp) = open_chunked(&dir, "survivor.bin");
    assert_eq!(tmp, stale_tmp, "the reclaimed tmp must be the planted one");

    res.write_at(0, b"written-by-successor").unwrap();
    res.commit(Some(20)).unwrap();
    assert_eq!(
        &fs::read(&canonical).unwrap(),
        b"written-by-successor",
        "the committed bytes must be the successor's, not the dead writer's"
    );
    assert!(!tmp.exists(), "tmp consumed by the atomic rename");
}

/// What a dead owner left in the tmp a successor reclaims.
const DEAD_OWNERS_BYTES: &[u8] = b"stale-from-previous-process";

/// Reclaiming a stale tmp starts from an empty file. `create_new` used to
/// give that for free; the lock-based claim has to do it itself.
#[kithara::test(timeout(Duration::from_secs(2)))]
fn a_reclaimed_tmp_carries_no_byte_of_its_dead_owner() {
    let dir = TempDir::new().unwrap();
    let stale_tmp = make_tmp_path(&dir.path().join("tail.bin")).unwrap();
    fs::write(&stale_tmp, DEAD_OWNERS_BYTES).unwrap();

    let (_res, _canonical, tmp) = open_chunked(&dir, "tail.bin");

    let bytes = fs::read(&tmp).unwrap();
    assert!(
        bytes.iter().all(|byte| *byte == 0),
        "reclaimed tmp still carries the dead owner's bytes: {:?}",
        &bytes[..bytes.len().min(32)]
    );
}

/// Emptiness is the invariant the driver reads the tmp against:
/// `MmapDriver::open` adopts a non-empty file whole — available `0..len`,
/// committed, `final_len` set — so a dead owner's bytes come back as readable
/// payload, not as residue nothing can reach.
#[kithara::test(timeout(Duration::from_secs(2)))]
fn a_reclaimed_tmp_offers_none_of_its_dead_owner_as_payload() {
    let dir = TempDir::new().unwrap();
    let stale_tmp = make_tmp_path(&dir.path().join("adopted.bin")).unwrap();
    fs::write(&stale_tmp, DEAD_OWNERS_BYTES).unwrap();

    let (res, _canonical, _tmp) = open_chunked(&dir, "adopted.bin");

    assert!(
        !res.contains_range(0..DEAD_OWNERS_BYTES.len() as u64),
        "the dead owner's bytes came back as available payload"
    );
}

/// Since the claim is a lock rather than a file, it has to be released when
/// the writer settles — otherwise a rewrite of the same canonical path would
/// see `TmpClaimed` from a holder that is done.
#[kithara::test(timeout(Duration::from_secs(2)))]
fn a_settled_claim_frees_the_tmp_for_a_successor() {
    let dir = TempDir::new().unwrap();
    let (holder, canonical, tmp) = open_chunked(&dir, "held.bin");
    holder.write_at(0, b"in-flight").unwrap();
    holder.commit(Some(9)).unwrap();
    drop(holder);
    assert!(!tmp.exists());
    assert!(canonical.is_file());

    let successor = open_chunked_result(&dir, "held.bin").expect("settled claim must be released");
    successor.write_at(0, b"rewritten").unwrap();
    successor.commit(Some(9)).unwrap();
    assert_eq!(&fs::read(&canonical).unwrap(), b"rewritten");
}

#[kithara::test(timeout(Duration::from_secs(2)))]
fn concurrent_open_atomic_claim_returns_tmp_claimed() {
    let dir = TempDir::new().unwrap();
    let canonical = dir.path().join("contested.bin");

    let cancel = CancelToken::never();
    let factory = {
        let cancel = cancel;
        move |target: &Path, intent: OpenIntent| {
            let mode = match intent {
                OpenIntent::Fresh => OpenMode::ReadWrite,
                OpenIntent::Reopen => OpenMode::ReadOnly,
            };
            Resource::open(
                cancel.clone(),
                MmapOptions {
                    mode,
                    initial_len: None,
                    path: target.to_path_buf(),
                },
            )
        }
    };

    let _holder = AtomicChunked::<MmapDriver>::open(canonical.clone(), factory.clone())
        .expect("first open claims tmp");

    let err = AtomicChunked::<MmapDriver>::open(canonical, factory)
        .expect_err("second concurrent open must be rejected");
    assert!(
        matches!(err, crate::StorageError::TmpClaimed(_)),
        "expected TmpClaimed, got {err:?}"
    );
}

#[kithara::test(timeout(Duration::from_secs(2)))]
fn read_after_commit_returns_payload_via_decorator() {
    let dir = TempDir::new().unwrap();
    let (res, _, _) = open_chunked(&dir, "post-commit-read.bin");
    res.write_at(0, b"chunk-1-").unwrap();
    res.write_at(8, b"chunk-2!").unwrap();
    res.commit(Some(16)).unwrap();

    let mut buf = [0u8; 16];
    let n = res.read_at(0, &mut buf).unwrap();
    assert_eq!(n, 16, "post-commit read must return all bytes");
    assert_eq!(&buf, b"chunk-1-chunk-2!");

    let mut tail = [0u8; 1];
    let n = res.read_at(15, &mut tail).unwrap();
    assert_eq!(n, 1);
    assert_eq!(tail[0], b'!');
}

#[kithara::test(timeout(Duration::from_secs(2)))]
fn read_during_writes_observes_inner_state() {
    let dir = TempDir::new().unwrap();
    let (res, _, _) = open_chunked(&dir, "live.bin");
    res.write_at(0, b"live-bytes").unwrap();
    let mut buf = [0u8; 10];
    let n = res.read_at(0, &mut buf).unwrap();
    assert_eq!(n, 10);
    assert_eq!(&buf, b"live-bytes");
}

#[kithara::test(timeout(Duration::from_secs(2)))]
fn passthrough_for_memory_inner_has_no_tmp() {
    let mem = crate::MemResource::new(CancelToken::never(), BytePool::default());
    let res = AtomicChunked::passthrough(mem, PathBuf::from("virtual"));
    res.write_at(0, b"in-mem").unwrap();
    res.commit(Some(6)).unwrap();
    let mut buf = [0u8; 6];
    res.read_at(0, &mut buf).unwrap();
    assert_eq!(&buf, b"in-mem");
    assert_eq!(res.path(), Some(Path::new("virtual")));
}
