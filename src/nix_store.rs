//! Fixed-width edits and detection of Nix store references.

use crate::artifact::Artifact;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileExt;
use std::path::{Component, Path};

const HASH_LEN: usize = 32;
const BLANK: [u8; HASH_LEN] = [b'e'; HASH_LEN];
const SCAN_BUFFER_SIZE: usize = 64 * 1024;

fn is_base32(byte: u8) -> bool {
    matches!(byte, b'0'..=b'9' | b'a'..=b'd' | b'f'..=b'n' | b'p'..=b's' | b'v'..=b'z')
}

/// A known Nix store hash, for fixed-width removal of specific references.
pub struct StoreHash([u8; HASH_LEN]);

impl StoreHash {
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let bytes: [u8; HASH_LEN] = bytes.try_into().ok()?;
        bytes.into_iter().all(is_base32).then_some(Self(bytes))
    }

    /// Parse the first store path component lexically. Paths with `..`
    /// below the store directory are rejected.
    pub fn from_store_path(store_dir: &Path, path: &Path) -> Option<Self> {
        let mut components = path.strip_prefix(store_dir).ok()?.components();
        let Component::Normal(item) = components.next()? else {
            return None;
        };
        if components.any(|component| component == Component::ParentDir) {
            return None;
        }
        let (hash, name) = item.as_bytes().split_at_checked(HASH_LEN)?;
        name.strip_prefix(b"-").filter(|name| !name.is_empty())?;
        Self::from_bytes(hash)
    }

    /// Replace nonoverlapping matches with `e` bytes, including bare hashes.
    /// Use only where the references to this item are incidental.
    pub fn blank_in(&self, mut bytes: &mut [u8]) -> bool {
        let mut changed = false;
        while let Some(offset) = bytes.windows(HASH_LEN).position(|window| window == self.0) {
            let (matched, rest) = bytes[offset..].split_at_mut(HASH_LEN);
            matched.copy_from_slice(&BLANK);
            bytes = rest;
            changed = true;
        }
        changed
    }
}

fn scan_candidates(bytes: &[u8], run: &mut usize) -> bool {
    bytes.iter().any(|&byte| {
        *run = if is_base32(byte) { *run + 1 } else { 0 };
        *run == HASH_LEN
    })
}

/// Test for any run of at least 32 Nix base32 bytes. The caller needs a set
/// of known hashes to tell whether a match is a store reference.
/// File scans preserve the seek position and use at most 64 KiB of scratch space.
pub fn contains_store_hash_candidate(output: &Artifact<'_>) -> io::Result<bool> {
    let mut run = 0;
    let (file, size) = match output {
        Artifact::Bytes(bytes) => return Ok(scan_candidates(bytes, &mut run)),
        Artifact::File { file, size } => (file, *size),
    };
    if size < HASH_LEN as u64 {
        return Ok(false);
    }
    // Reuse one buffer to amortize positioned reads. Only the length of the
    // trailing base32 run needs to carry over to the next block.
    let mut buffer = vec![0; size.min(SCAN_BUFFER_SIZE as u64) as usize];
    for offset in (0..size).step_by(buffer.len()) {
        let len = (size - offset).min(buffer.len() as u64) as usize;
        let block = &mut buffer[..len];
        file.read_exact_at(block, offset)?;
        if scan_candidates(block, &mut run) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Replace each group of 32 consecutive Nix base32 bytes with `e` bytes.
/// This matches potential references, including bare hashes. Pass only metadata
/// whose references the caller has determined are incidental.
pub fn blank_store_hashes(bytes: &mut [u8]) -> bool {
    let mut changed = false;
    let hashes = bytes
        .split_mut(|&byte| !is_base32(byte))
        .flat_map(|run| run.as_chunks_mut::<HASH_LEN>().0);
    for hash in hashes {
        *hash = BLANK;
        changed = true;
    }
    changed
}

#[cfg(all(test, feature = "publish"))]
#[test]
fn scans_a_candidate_ending_in_the_last_byte_of_a_file() -> io::Result<()> {
    use std::io::{Seek, SeekFrom};

    let mut file = tempfile::tempfile()?;
    let boundary = SCAN_BUFFER_SIZE as u64;
    let size = boundary + 1;
    file.write_all_at(&[b'0'; HASH_LEN], size - HASH_LEN as u64)?;
    file.seek(SeekFrom::Start(5))?;

    let scan = |size| contains_store_hash_candidate(&Artifact::File { file: &file, size });
    assert!(!scan(boundary)?);
    assert!(scan(size)?);
    file.write_all_at(b"e", boundary)?;
    assert!(!scan(size)?);
    assert_eq!(
        scan(size + 1).unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
    assert_eq!(file.stream_position()?, 5);
    Ok(())
}
