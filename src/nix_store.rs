//! Fixed-width edits to candidate Nix store references in selected metadata.

/// Replace each group of 32 consecutive Nix base32 bytes with `e` bytes.
/// This matches potential references, including bare hashes. Pass only metadata
/// whose references the caller has determined are incidental.
pub fn blank_store_hashes(bytes: &mut [u8]) -> bool {
    const ALPHABET: &[u8] = b"0123456789abcdfghijklmnpqrsvwxyz";
    let mut changed = false;
    let hashes = bytes
        .split_mut(|byte| !ALPHABET.contains(byte))
        .flat_map(|run| run.as_chunks_mut::<32>().0);
    for hash in hashes {
        hash.fill(b'e');
        changed = true;
    }
    changed
}
