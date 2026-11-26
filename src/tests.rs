use crate::ByteEdits;
use crate::artifact::{Artifact, edit_elf_sections};
use crate::nix_store::{StoreHash, blank_store_hashes, contains_store_hash_candidate};
use std::path::Path;

#[test]
fn blanks_complete_nix_hash_candidates_in_place() {
    let hash = b"0123456789abcdfghijklmnpqrsvwxyz";
    let replacement = [b'e'; 32];
    let short = &hash[..31];
    let mut bytes = [
        b"\xff/store/".as_slice(),
        hash,
        b"-name\0",
        hash,
        hash,
        b"0",
        b"=",
        short,
    ]
    .concat();
    assert!(contains_store_hash_candidate(&Artifact::Bytes(&mut bytes)).unwrap());
    assert!(blank_store_hashes(&mut bytes));
    assert_eq!(
        bytes,
        [
            b"\xff/store/".as_slice(),
            &replacement,
            b"-name\0",
            &replacement,
            &replacement,
            b"0",
            b"=",
            short
        ]
        .concat()
    );
    assert!(!contains_store_hash_candidate(&Artifact::Bytes(&mut bytes)).unwrap());
    assert!(!blank_store_hashes(&mut bytes));
}

#[test]
fn blanks_only_the_requested_hash_even_inside_longer_runs() {
    let store = Path::new("/custom/store");
    let hash = b"0123456789abcdfghijklmnpqrsvwxyz";
    let other = b"123456789abcdfghijklmnpqrsvwxyz0z".as_slice();
    let path = Path::new("/custom/store/0123456789abcdfghijklmnpqrsvwxyz-package/lib");
    assert!(StoreHash::from_store_path(Path::new("/nix/store"), path).is_none());
    assert!(
        StoreHash::from_store_path(store, &store.join("0123456789abcdfghijklmnpqrsvwxyz-"))
            .is_none()
    );
    let target = StoreHash::from_store_path(store, path).unwrap();
    assert!(StoreHash::from_store_path(store, &path.join("../../another-item")).is_none());
    let mut bytes = [other, hash, hash].concat();
    assert!(target.blank_in(&mut bytes));
    assert_eq!(bytes, [other, &[b'e'; 32], &[b'e'; 32]].concat());
    assert!(!target.blank_in(&mut bytes));
}

#[test]
fn byte_edits_preserve_untouched_bytes() {
    let input = b"\xffone,two,three\xfe";
    assert_eq!(ByteEdits::new(input).finish(), None);

    let mut edits = ByteEdits::new(input);
    edits.replace(0..0, b">");
    edits.replace(1..4, b"1");
    edits.replace(8..9, b"");
    assert_eq!(
        edits.finish().as_deref(),
        Some(b">\xff1,twothree\xfe".as_slice())
    );
}

#[test]
fn borrowed_artifact_rewrite_preserves_its_length() -> std::io::Result<()> {
    let mut bytes = *b"old";
    let mut artifact = Artifact::Bytes(&mut bytes);
    artifact.rewrite(|_| Ok(None))?;
    let error = artifact
        .rewrite(|_| Ok(Some(b"longer".to_vec())))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    artifact.rewrite(|bytes| {
        assert_eq!(bytes, b"old");
        Ok(Some(b"new".to_vec()))
    })?;
    assert_eq!(&bytes, b"new");
    Ok(())
}

#[test]
fn elf_parser_rejects_truncated_buffers() {
    let mut header = [0; 64];
    header[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    header[40..48].copy_from_slice(&64u64.to_le_bytes());
    header[58..60].copy_from_slice(&64u16.to_le_bytes());
    assert!(
        edit_elf_sections(&mut Artifact::Bytes(&mut header), b".metadata", |_| panic!(
            "called editor for a malformed ELF file"
        ))
        .is_err()
    );
}

#[cfg(all(
    target_os = "linux",
    target_pointer_width = "64",
    target_endian = "little"
))]
#[test]
fn elf_file_edits_respect_size_and_preserve_the_callers_seek_offset() -> std::io::Result<()> {
    use std::fs::File;
    use std::io::{Seek, SeekFrom};

    let mut file = File::open(std::env::current_exe()?)?;
    let size = file.metadata()?.len();
    file.seek(SeekFrom::Start(5))?;

    let mut visited = false;
    let changed = edit_elf_sections(
        &mut Artifact::File { file: &file, size },
        b".text",
        |bytes| {
            assert!(!bytes.is_empty());
            visited = true;
            false
        },
    )?;

    assert!(visited);
    assert!(!changed);
    assert!(
        edit_elf_sections(
            &mut Artifact::File {
                file: &file,
                size: 64,
            },
            b".tool-wrapper-test-missing",
            |_| panic!("called editor for a truncated ELF file"),
        )
        .is_err()
    );
    assert_eq!(file.stream_position()?, 5);
    Ok(())
}

#[test]
fn archive_layout_is_validated_before_editing() {
    use crate::artifact::normalize_archive_metadata;

    let header = format!(
        "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
        "member/", 1234, 1000, 1000, 644, 3,
    );
    let valid = [b"!<arch>\n".as_slice(), header.as_bytes(), b"ok!\n"].concat();
    for original in [
        valid[..valid.len() - 1].to_vec(),
        [valid.as_slice(), b"truncated"].concat(),
    ] {
        let mut malformed = original.clone();
        assert!(normalize_archive_metadata(&mut Artifact::Bytes(&mut malformed)).is_err());
        assert_eq!(malformed, original);
    }

    let mut normalized = valid;
    normalize_archive_metadata(&mut Artifact::Bytes(&mut normalized)).unwrap();
    assert_eq!(&normalized[24..48], b"0           0     0     ");
    assert_eq!(&normalized[68..], b"ok!\n");
}
