use std::ffi::CStr;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::ExitCode;
use tool_wrapper::process::{Argument, Arguments};
use tool_wrapper::{
    response::{Gnu, Group, Kind, Limits, Syntax, rewrite},
    wrap_tool,
};

// A conservative subset of Unix Clang config syntax for this demonstration.
struct ClangConfig;

impl Syntax for ClangConfig {
    type Word<'a> = Option<&'a [u8]>;

    const STAGED_NAME: &'static CStr = c"clang-config";
    const FILE_RELATIVE: bool = true;

    fn next<'a>(&self, words: &'a [Argument]) -> io::Result<Group<'a, Self::Word<'a>>> {
        self.next_in(words, None)
    }

    fn next_in<'a>(
        &self,
        words: &'a [Argument],
        source: Option<&Path>,
    ) -> io::Result<Group<'a, Self::Word<'a>>> {
        let word = words[0].as_slice();
        let prefix = if source.is_some() {
            b"@".as_slice()
        } else {
            b"--config=".as_slice()
        };
        if let Some(path) = word.strip_prefix(prefix)
            && !path.is_empty()
            && (source.is_some() || path.contains(&b'/'))
        {
            return Ok(Group {
                words: 1,
                prefix,
                kind: Kind::Files {
                    paths: path,
                    separator: None,
                },
            });
        }
        Ok(Group {
            words: 1,
            prefix: b"",
            kind: Kind::Word(source.map(|_| word)),
        })
    }

    fn decode(&self, contents: &[u8]) -> io::Result<Arguments> {
        let contents = contents.strip_prefix(b"\xef\xbb\xbf").unwrap_or(contents);
        if contents.starts_with(b"\xff\xfe") || contents.starts_with(b"\xfe\xff") {
            return Err(unsupported());
        }
        let mut words = Vec::new();
        for line in contents.split(|&b| b == b'\n') {
            if line.trim_ascii_start().starts_with(b"#") {
                continue;
            }
            if line.iter().any(|b| matches!(b, b'#' | 0x0b | 0x0c))
                || line.strip_suffix(b"\r").unwrap_or(line).ends_with(b"\\")
            {
                return Err(unsupported());
            }
            words.extend(Gnu.decode(line)?);
        }
        Ok(words)
    }

    fn prepare_file(&self, source: &Path, words: &mut [Argument]) -> io::Result<()> {
        const CFGDIR: &[u8] = b"<CFGDIR>";
        let base = source
            .parent()
            .ok_or_else(unsupported)?
            .as_os_str()
            .as_bytes();
        for word in words {
            if word == b"--config" || word.starts_with(b"--config=") {
                return Err(unsupported());
            }
            if let Some(offset) = word.windows(CFGDIR.len()).position(|part| part == CFGDIR) {
                let tail = &word[offset + CFGDIR.len()..];
                if !self.can_encode(base)
                    || base == b"/"
                    || base.windows(CFGDIR.len()).any(|part| part == CFGDIR)
                    || (!tail.is_empty() && !tail.starts_with(b"/"))
                    || tail.windows(CFGDIR.len()).any(|part| part == CFGDIR)
                {
                    return Err(unsupported());
                }
                *word = [&word[..offset], base, tail].concat();
            }
        }
        Ok(())
    }

    fn encode(&self, words: &[Argument]) -> Vec<u8> {
        Gnu.encode(words)
    }

    fn can_encode(&self, value: &[u8]) -> bool {
        !value.iter().any(|b| matches!(b, 0 | b'\n' | b'\r'))
    }
}

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "unsupported Clang config syntax",
    )
}

fn main() -> ExitCode {
    wrap_tool!(Some("clang"), |tool| {
        // Leave alternate driver modes and opaque command-line @files to Clang.
        if tool
            .args
            .iter()
            .any(|arg| arg == b"--" || arg.starts_with(b"@") || arg.starts_with(b"--driver-mode="))
        {
            return Ok(tool.forward());
        }
        let rewritten = rewrite(&tool.args, ClangConfig, Limits::default(), |word, _| {
            Ok((word == Some(b"-DWRAPPER_DEMO=0")).then(|| b"-DWRAPPER_DEMO=1".to_vec()))
        })?;
        Ok(tool.with_prepared_args(rewritten).forward())
    })
}
