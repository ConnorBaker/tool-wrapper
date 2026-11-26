use super::{Group, Kind};
use crate::process::Argument;
use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt;

/// How a tool interprets an option spelling.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    Plain,
    Operand,
    Files { separator: Option<u8> },
}

pub struct OptionArg<'a> {
    pub name: &'a [u8],
    pub value: &'a [u8],
    pub kind: OptionKind,
    pub words: &'a [Argument],
}

/// Group an option with a required operand in `--flag=value` or `--flag value` form.
/// The caller classifies the name before the first `=`. `Plain` keeps the whole
/// word as its value and consumes only that word. Named file options use the
/// same two forms and delegate traversal to the response-file walker.
///
/// `words` must be nonempty, as with [`super::Syntax::next`].
pub fn option_group<'a>(
    words: &'a [Argument],
    classify: impl FnOnce(&[u8]) -> OptionKind,
) -> io::Result<Group<'a, OptionArg<'a>>> {
    let word = words[0].as_slice();
    let (name, inline) = match word.iter().position(|&byte| byte == b'=') {
        Some(equal) => (&word[..equal], Some(&word[equal + 1..])),
        None => (word, None),
    };
    let kind = classify(name);
    let (value, count, prefix) = match (kind, inline, words.get(1)) {
        (OptionKind::Plain, _, _) => (word, 1, &word[..0]),
        (_, Some(value), _) => (value, 1, &word[..name.len() + 1]),
        (_, None, Some(value)) => (value.as_slice(), 2, &word[..0]),
        _ => {
            return Err(io::Error::other(format!(
                "{} is missing its value",
                OsStr::from_bytes(name).to_string_lossy()
            )));
        }
    };
    let kind = match kind {
        OptionKind::Files { separator } => Kind::Files {
            paths: value,
            separator,
        },
        _ => Kind::Word(OptionArg {
            name,
            value,
            kind,
            words: &words[..count],
        }),
    };
    Ok(Group {
        words: count,
        prefix,
        kind,
    })
}
