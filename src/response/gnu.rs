use super::{Group, Kind, Syntax};
use crate::process::{Argument, Arguments};
use crate::quoting::{Escape, double_quoted_lines, split_words};
use std::ffi::CStr;
use std::io;

/// GNU libiberty-style `@file` arguments as used by GNU ar and GCC.
///
/// Ambiguous escape sequences are left to the tool. Libiberty versions differ
/// in how they interpret backslashes inside quotes and before a newline.
pub struct Gnu;

impl Syntax for Gnu {
    type Word<'a> = &'a [u8];

    const STAGED_NAME: &'static CStr = c"gnu-response";

    fn next<'a>(&self, words: &'a [Argument]) -> io::Result<Group<'a, Self::Word<'a>>> {
        let word = words[0].as_slice();
        let (prefix, kind) = match word {
            [b'@', path @ ..] if !path.is_empty() => (
                b"@".as_slice(),
                Kind::Files {
                    paths: path,
                    separator: None,
                },
            ),
            _ => (b"".as_slice(), Kind::Word(word)),
        };
        Ok(Group {
            words: 1,
            prefix,
            kind,
        })
    }

    fn decode(&self, contents: &[u8]) -> io::Result<Arguments> {
        if contents.contains(&0) {
            return Err(invalid_input());
        }
        split_words(contents, b"'\"", |quote, escaped| match (quote, escaped) {
            (_, b'\n') | (Some(b'\''), _) => Escape::Reject,
            (Some(b'"'), byte) if !matches!(byte, b'\\' | b'"' | b'$' | b'`') => Escape::Reject,
            _ => Escape::Keep,
        })
        .ok_or_else(invalid_input)
    }

    fn encode(&self, words: &[Argument]) -> Vec<u8> {
        double_quoted_lines(words)
    }
}

fn invalid_input() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "unsupported GNU response-file quoting",
    )
}
