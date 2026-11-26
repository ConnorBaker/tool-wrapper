//! Fixed-width edits and detection of Nix store references.

use crate::artifact::Artifact;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

const HASH_LEN: usize = 32;
const BLANK: [u8; HASH_LEN] = [b'e'; HASH_LEN];
const SCAN_WINDOW: usize = 64 * 1024;

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

    /// Parse the hash from a path rooted at `store_dir` without accessing the filesystem.
    pub fn from_store_path(store_dir: &Path, path: &Path) -> Option<Self> {
        let Component::Normal(item) = path.strip_prefix(store_dir).ok()?.components().next()?
        else {
            return None;
        };
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

/// Test for a run of at least 32 Nix base32 bytes without changing the artifact.
/// File scans read at most 64 KiB at a time and also test runs across read boundaries.
pub fn contains_store_hash(output: &Artifact<'_>) -> io::Result<bool> {
    // Keep enough overlap to see every complete candidate at least once.
    let size = output.size();
    for offset in (0..size).step_by(SCAN_WINDOW - HASH_LEN + 1) {
        let count = (size - offset).min(SCAN_WINDOW as u64);
        if count < HASH_LEN as u64 {
            break;
        }
        let bytes = output.read_bytes(offset, count)?;
        if bytes
            .split(|&byte| !is_base32(byte))
            .any(|run| run.len() >= HASH_LEN)
        {
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
