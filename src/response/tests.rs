use super::*;
use crate::files::{anonymous_input, descriptor_path};
use std::fs::{self, File};
use std::os::fd::AsRawFd;

fn response(text: &[u8]) -> File {
    anonymous_input(c"test-response", text).unwrap()
}

fn arguments(file: &[u8]) -> Arguments {
    Gnu.decode(&fs::read(OsStr::from_bytes(file)).unwrap())
        .unwrap()
}

fn replace(word: &[u8], _: &mut InputFiles) -> io::Result<ControlFlow<(), Option<Argument>>> {
    Ok(ControlFlow::Continue(
        (word == b"replace").then(|| b"new \"value\"".to_vec()),
    ))
}

#[test]
fn groups_inline_and_separate_option_operands() {
    let classify = |name: &[u8]| match name {
        b"--output" => OptionKind::Operand,
        b"--files" => OptionKind::Files {
            separator: Some(b','),
        },
        _ => OptionKind::Plain,
    };
    for (words, expected_prefix, expected_count) in [
        (
            vec![b"--output=raw\xff=a".to_vec()],
            b"--output=".as_slice(),
            1,
        ),
        (
            vec![b"--output".to_vec(), b"raw\xff=a".to_vec()],
            b"".as_slice(),
            2,
        ),
    ] {
        let group = option_group(&words, classify).unwrap();
        assert_eq!(
            (group.prefix, group.words),
            (expected_prefix, expected_count)
        );
        let Kind::Word(option) = group.kind else {
            panic!("expected an operand")
        };
        assert_eq!(option.name, b"--output");
        assert_eq!(option.value, b"raw\xff=a");
        assert_eq!(option.words, words);
    }

    let words = [b"--unknown=value".to_vec(), b"untouched".to_vec()];
    let group = option_group(&words, classify).unwrap();
    let Kind::Word(option) = group.kind else {
        panic!("expected a plain word")
    };
    assert_eq!(option.value, words[0]);
    assert_eq!(group.words, 1);

    let words = [b"--files=one,two".to_vec()];
    let group = option_group(&words, classify).unwrap();
    assert!(matches!(
        group.kind,
        Kind::Files {
            paths: b"one,two",
            separator: Some(b',')
        }
    ));
    assert_eq!(group.prefix, b"--files=");
    assert!(option_group(&[b"--output".to_vec()], classify).is_err());
}

#[test]
fn replays_changed_ancestors_and_keeps_sibling_files() {
    let inner = response(b"replace 'opaque # value' raw-\xff");
    let sibling = response(b"unchanged");
    let inner_arg = format!("@{}", descriptor_path(&inner)).into_bytes();
    let sibling_arg = format!("@{}", descriptor_path(&sibling)).into_bytes();
    let outer = response(&[&inner_arg[..], b" ", &sibling_arg].concat());
    let args = vec![
        b"before".to_vec(),
        format!("@{}", descriptor_path(&outer)).into_bytes(),
        b"after".to_vec(),
    ];
    let mut visited = Vec::new();
    visit(&args, Gnu, Limits::default(), |word| {
        visited.push(word.to_vec())
    })
    .unwrap();
    assert_eq!(
        visited,
        [
            b"before".as_slice(),
            b"replace",
            b"opaque # value",
            b"raw-\xff",
            b"unchanged",
            b"after"
        ]
    );

    let mut walker = Walker {
        syntax: Gnu,
        replay: true,
        required: false,
        limits: Limits::default(),
        edit: replace,
        files: InputFiles::default(),
    };
    let replayed = walker.walk(Cow::Borrowed(&args), None, 0).unwrap().unwrap();
    assert_eq!((&replayed[0], &replayed[2]), (&args[0], &args[2]));
    assert_ne!(replayed[1], args[1]);
    let outer_words = arguments(replayed[1].strip_prefix(b"@").unwrap());
    assert_eq!(outer_words[1], sibling_arg);
    assert_ne!(outer_words[0], inner_arg);
    assert_eq!(
        arguments(outer_words[0].strip_prefix(b"@").unwrap()),
        [b"new \"value\"".as_slice(), b"opaque # value", b"raw-\xff"]
    );
    assert_eq!(arguments(descriptor_path(&inner).as_bytes())[0], b"replace");
}

#[test]
fn preserves_list_separators_and_untouched_paths() {
    let inner = response(b"replace");
    let sibling = response(b"unchanged");
    let sibling_path = descriptor_path(&sibling);
    let paths = format!(",{sibling_path},,{},,", descriptor_path(&inner));
    let mut walker = Walker {
        syntax: Gnu,
        replay: true,
        required: false,
        limits: Limits::default(),
        edit: replace,
        files: InputFiles::default(),
    };
    let (replayed, edited) = walker
        .walk_paths(paths.as_bytes(), Some(b','), None, 1)
        .unwrap()
        .unwrap();
    assert!(edited);
    let paths: Vec<_> = replayed.split(|&b| b == b',').collect();
    assert_eq!(&paths[..3], &[b"".as_slice(), sibling_path.as_bytes(), b""]);
    assert_eq!(&paths[4..], &[b"".as_slice(), b""]);
    assert_eq!(arguments(paths[3]), [b"new \"value\"".as_slice()]);
}

#[test]
fn rewrite_falls_back_on_inspection_errors_and_reports_edit_errors() {
    let inner = response(b"replace");
    let args = [format!("@{}", descriptor_path(&inner)).into_bytes()];
    for limits in [
        Limits {
            depth: 0,
            bytes: usize::MAX,
        },
        Limits {
            depth: 15,
            bytes: 0,
        },
    ] {
        assert!(
            rewrite_until(&args, Gnu, limits, replace)
                .unwrap()
                .is_none()
        );
    }
    let error = rewrite(&args, Gnu, Limits::default(), |_, _| {
        Err(io::Error::other("edit failed"))
    })
    .err()
    .unwrap();
    assert_eq!(error.to_string(), "edit failed");
    let malformed = response(br#""ambiguous\q""#);
    let too_deep = [format!("@{}", descriptor_path(&malformed)).into_bytes()];
    let error = visit(
        &too_deep,
        Gnu,
        Limits {
            depth: 0,
            ..Limits::default()
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "options files are nested too deeply");
    let error = rewrite_required_until(
        &too_deep,
        Gnu,
        Limits {
            depth: 0,
            ..Limits::default()
        },
        replace,
    )
    .err()
    .unwrap();
    assert_eq!(error.to_string(), "options files are nested too deeply");
    let [malformed_arg] = too_deep;
    let args = [b"replace".to_vec(), malformed_arg];
    assert!(
        rewrite_until(&args, Gnu, Limits::default(), replace)
            .unwrap()
            .is_none()
    );
}

#[test]
fn discards_inputs_staged_by_rejected_edits() {
    let mut walker = Walker {
        syntax: Gnu,
        replay: true,
        required: false,
        limits: Limits::default(),
        edit: |word: &[u8], files: &mut InputFiles| {
            let path = files.stage(c"tentative-edit", word)?;
            Ok(ControlFlow::Continue((word == b"keep").then_some(path)))
        },
        files: InputFiles::default(),
    };
    let args = [b"drop".to_vec(), b"keep".to_vec()];
    let edited = walker.walk(Cow::Borrowed(&args), None, 0).unwrap().unwrap();
    assert_eq!(edited[0], args[0]);
    assert_eq!(fs::read(OsStr::from_bytes(&edited[1])).unwrap(), b"keep");
    assert_eq!(walker.files.len(), 1);
}

#[test]
fn break_in_nested_file_discards_edits_and_stops_traversal() {
    let files = response(b"edit stop unexpected");
    let args = [
        b"edit".to_vec(),
        format!("@{}", descriptor_path(&files)).into_bytes(),
        b"unexpected".to_vec(),
    ];
    let visited = visit_until(&args, Gnu, Limits::default(), |word| match word {
        b"edit" => ControlFlow::Continue(()),
        b"stop" => ControlFlow::Break(()),
        _ => panic!("visited a word after break"),
    })
    .unwrap();
    assert!(visited.is_break());

    let mut staged = Vec::new();
    let prepared = rewrite_until(&args, Gnu, Limits::default(), |word, files| match word {
        b"edit" => {
            let path = files.stage(c"discard-on-break", b"replacement")?;
            staged.push(path.clone());
            Ok(ControlFlow::Continue(Some(path)))
        }
        b"stop" => Ok(ControlFlow::Break(())),
        _ => panic!("visited a word after break"),
    })
    .unwrap();
    assert!(prepared.is_none());
    assert_eq!(staged.len(), 2);
    for path in staged {
        let link = fs::read_link(OsStr::from_bytes(&path)).ok();
        assert!(!link.is_some_and(|link| link.to_string_lossy().contains("discard-on-break")));
    }
}

#[test]
fn gnu_quotes_round_trip_byte_words() {
    let words = Gnu
        .decode(b"empty=\"\" '' a\x0bb 'raw-\xff' \"\\$\\\"\"")
        .unwrap();
    assert_eq!(
        words,
        [b"empty=".as_slice(), b"", b"a", b"b", b"raw-\xff", b"$\""]
    );
    assert_eq!(Gnu.decode(&Gnu.encode(&words)).unwrap(), words);
    assert!(Gnu.decode(br#"'escaped\quote'"#).is_err());
}

struct FileRelative;

impl Syntax for FileRelative {
    type Word<'a> = &'a [u8];

    const STAGED_NAME: &'static CStr = c"relative-response";
    const FILE_RELATIVE: bool = true;

    fn next<'a>(&self, words: &'a [Argument]) -> io::Result<Group<'a, Self::Word<'a>>> {
        Gnu.next(words)
    }

    fn decode(&self, contents: &[u8]) -> io::Result<Arguments> {
        Gnu.decode(contents)
    }

    fn encode(&self, words: &[Argument]) -> Vec<u8> {
        Gnu.encode(words)
    }
}

#[test]
fn rebases_unchanged_relative_includes_only_when_replaying_an_edit() {
    let changed = response(b"replace");
    let unchanged = response(b"untouched");
    let unchanged_path = descriptor_path(&unchanged);
    let root = response(format!("@{} @{}", changed.as_raw_fd(), unchanged.as_raw_fd(),).as_bytes());
    let args = [format!("@{}", descriptor_path(&root)).into_bytes()];
    assert!(
        rewrite(&args, FileRelative, Limits::default(), |_, _| Ok(None))
            .unwrap()
            .is_none()
    );
    let mut walker = Walker {
        syntax: FileRelative,
        replay: true,
        required: false,
        limits: Limits::default(),
        edit: replace,
        files: InputFiles::default(),
    };
    let replayed = walker.walk(Cow::Borrowed(&args), None, 0).unwrap().unwrap();
    let staged_words = arguments(&replayed[0][1..]);
    assert_eq!(staged_words[1][1..], *unchanged_path.as_bytes());
    assert_eq!(
        arguments(&staged_words[0][1..]),
        [b"new \"value\"".as_slice()]
    );
}
