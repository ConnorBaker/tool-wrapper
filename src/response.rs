//! Inspect or replay nested response files without flattening them into argv.

mod gnu;
mod option;
pub use gnu::Gnu;
pub use option::{OptionArg, OptionKind, option_group};

use crate::ByteEdits;
use crate::files::{InputFiles, read_regular};
use crate::process::{Argument, Arguments, PreparedArgs};
use std::borrow::Cow;
use std::ffi::{CStr, OsStr};
use std::io;
use std::ops::ControlFlow;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, absolute};

/// A tool's byte-level argument grouping and response-file quoting rules.
///
/// The tool must accept staged response files from `/proc/self/fd`.
pub trait Syntax {
    type Word<'a>;

    const STAGED_NAME: &'static CStr;

    /// Resolve nested relative includes against the including file's directory.
    /// The default resolves them against the working directory.
    const FILE_RELATIVE: bool = false;

    /// Parse one group from a nonempty argument list.
    fn next<'a>(&self, words: &'a [Argument]) -> io::Result<Group<'a, Self::Word<'a>>>;

    /// Parse a group when the grammar varies between argv and response files.
    fn next_in<'a>(
        &self,
        words: &'a [Argument],
        _source: Option<&Path>,
    ) -> io::Result<Group<'a, Self::Word<'a>>> {
        self.next(words)
    }

    fn decode(&self, contents: &[u8]) -> io::Result<Arguments>;

    /// Expand any file-local syntax before inspection and potential relocation.
    /// With `FILE_RELATIVE` enabled, `source` is an absolute path.
    fn prepare_file(&self, _source: &Path, _words: &mut [Argument]) -> io::Result<()> {
        Ok(())
    }

    fn encode(&self, words: &[Argument]) -> Vec<u8>;

    /// Whether an edited value can round-trip through a response file. Edits
    /// that cannot round-trip keep the original value.
    fn can_encode(&self, value: &[u8]) -> bool {
        !value.contains(&0)
    }
}

/// A group consumes one or more words. An edit replaces the last word,
/// prepending `prefix` to the new value for forms such as `--option=value`.
pub struct Group<'a, W> {
    pub words: usize,
    pub prefix: &'a [u8],
    pub kind: Kind<'a, W>,
}

pub enum Kind<'a, W> {
    Word(W),
    Files {
        paths: &'a [u8],
        separator: Option<u8>,
    },
}

/// Limits for inspecting a command and its nested response files.
///
/// By default, files can nest 15 levels with no total byte cap.
pub struct Limits {
    /// The outer argv has depth zero.
    pub depth: u8,
    /// Total bytes read across all referenced files, including repeated reads.
    pub bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            depth: 15,
            bytes: usize::MAX,
        }
    }
}

/// Visit effective arguments in command-line order, reporting inspection errors.
///
/// A later error can follow earlier calls to the visitor.
pub fn visit<S: Syntax>(
    args: &[Argument],
    syntax: S,
    limits: Limits,
    mut visit: impl FnMut(S::Word<'_>),
) -> io::Result<()> {
    visit_until(args, syntax, limits, |word| {
        visit(word);
        ControlFlow::Continue(())
    })
    .map(|_| ())
}

/// Visit effective arguments until the callback returns `Break(())`.
/// Arguments and files past the break are left uninspected.
pub fn visit_until<S: Syntax>(
    args: &[Argument],
    syntax: S,
    limits: Limits,
    mut visit: impl FnMut(S::Word<'_>) -> ControlFlow<()>,
) -> io::Result<ControlFlow<()>> {
    let result = Walker {
        syntax,
        limits,
        files: InputFiles::default(),
        replay: false,
        required: false,
        edit: |word: S::Word<'_>, _: &mut InputFiles| Ok(visit(word).map_continue(|()| None)),
    }
    .walk(Cow::Borrowed(args), None, 0);
    match result {
        Ok(_) => Ok(ControlFlow::Continue(())),
        Err(Error::Stopped) => Ok(ControlFlow::Break(())),
        Err(Error::Inspect(error) | Error::Edit(error)) => Err(error),
    }
}

/// Edit effective arguments, staging only the changed files and their ancestors.
///
/// An inspection error or a command with no changes returns `Ok(None)`, letting
/// the caller forward the original argv. Errors from the edit callback or from
/// staging a replacement are returned to the caller. Descriptors added by the
/// callback are retained only when its replacement is applied.
pub fn rewrite<S: Syntax>(
    args: &[Argument],
    syntax: S,
    limits: Limits,
    mut edit: impl FnMut(S::Word<'_>, &mut InputFiles) -> io::Result<Option<Argument>>,
) -> io::Result<Option<PreparedArgs>> {
    rewrite_until(args, syntax, limits, |word, files| {
        edit(word, files).map(ControlFlow::Continue)
    })
}

/// Rewrite like [`rewrite`], with an option to stop at any effective argument.
/// Returning `Break(())` drops all staged replacements, stops traversal,
/// and returns `Ok(None)` so the caller can launch with the original argv.
pub fn rewrite_until<S: Syntax>(
    args: &[Argument],
    syntax: S,
    limits: Limits,
    edit: impl FnMut(S::Word<'_>, &mut InputFiles) -> io::Result<ControlFlow<(), Option<Argument>>>,
) -> io::Result<Option<PreparedArgs>> {
    rewrite_using(args, syntax, limits, false, edit)
}

/// Like [`rewrite_until`], but return inspection errors and reject replacements
/// that cannot be represented in the response file.
pub fn rewrite_required_until<S: Syntax>(
    args: &[Argument],
    syntax: S,
    limits: Limits,
    edit: impl FnMut(S::Word<'_>, &mut InputFiles) -> io::Result<ControlFlow<(), Option<Argument>>>,
) -> io::Result<Option<PreparedArgs>> {
    rewrite_using(args, syntax, limits, true, edit)
}

fn rewrite_using<S: Syntax>(
    args: &[Argument],
    syntax: S,
    limits: Limits,
    required: bool,
    edit: impl FnMut(S::Word<'_>, &mut InputFiles) -> io::Result<ControlFlow<(), Option<Argument>>>,
) -> io::Result<Option<PreparedArgs>> {
    let mut walker = Walker {
        syntax,
        limits,
        files: InputFiles::default(),
        replay: true,
        required,
        edit,
    };
    match walker.walk(Cow::Borrowed(args), None, 0) {
        Ok(Some(args)) => Ok(Some(PreparedArgs::new(args, walker.files))),
        Ok(None) | Err(Error::Stopped) => Ok(None),
        Err(Error::Inspect(_)) if !required => Ok(None),
        Err(Error::Inspect(error) | Error::Edit(error)) => Err(error),
    }
}

#[derive(Debug)]
enum Error {
    Inspect(io::Error),
    Edit(io::Error),
    Stopped,
}

struct Walker<S, F> {
    syntax: S,
    limits: Limits,
    files: InputFiles,
    replay: bool,
    required: bool,
    edit: F,
}

impl<S, F> Walker<S, F>
where
    S: Syntax,
    F: FnMut(S::Word<'_>, &mut InputFiles) -> io::Result<ControlFlow<(), Option<Argument>>>,
{
    fn walk(
        &mut self,
        mut args: Cow<'_, [Argument]>,
        source: Option<&Path>,
        depth: usize,
    ) -> Result<Option<Arguments>, Error> {
        let include_base = if S::FILE_RELATIVE {
            source.and_then(Path::parent)
        } else {
            None
        };
        let mut edited = false;
        let mut index = 0;
        while index < args.len() {
            let checkpoint = self.files.len();
            let replacement = {
                let tail = &args[index..];
                let Group {
                    words,
                    prefix,
                    kind,
                } = self.syntax.next_in(tail, source).map_err(Error::Inspect)?;
                if words == 0 || words > tail.len() {
                    return Err(Error::Inspect(io::Error::other(
                        "invalid response-file argument group",
                    )));
                }
                index += words;
                let value = match kind {
                    Kind::Word(word) => {
                        match (self.edit)(word, &mut self.files).map_err(Error::Edit)? {
                            ControlFlow::Continue(replacement) => {
                                replacement.map(|value| (value, true))
                            }
                            ControlFlow::Break(()) => return Err(Error::Stopped),
                        }
                    }
                    Kind::Files { paths, separator } => {
                        self.walk_paths(paths, separator, include_base, depth + 1)?
                    }
                };
                match value {
                    Some((value, _)) if depth != 0 && !self.syntax.can_encode(&value) => {
                        if self.required {
                            return Err(Error::Edit(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "cannot represent a rewritten argument in the response file",
                            )));
                        }
                        None
                    }
                    Some((value, edited_arg)) => {
                        let word = if prefix.is_empty() {
                            value
                        } else {
                            [prefix, &value].concat()
                        };
                        Some((word, edited_arg))
                    }
                    None => None,
                }
            };
            if let Some((word, edited_arg)) = replacement {
                args.to_mut()[index - 1] = word;
                edited |= edited_arg;
            } else {
                self.files.truncate(checkpoint);
            }
        }
        Ok(edited.then(|| args.into_owned()))
    }

    fn walk_paths(
        &mut self,
        paths: &[u8],
        separator: Option<u8>,
        include_base: Option<&Path>,
        depth: usize,
    ) -> Result<Option<(Argument, bool)>, Error> {
        let mut edited = false;
        let mut replacement = ByteEdits::new(paths);
        for field in paths
            .split(|&b| Some(b) == separator)
            .filter(|field| !field.is_empty())
        {
            let path = Path::new(OsStr::from_bytes(field));
            let rebased = include_base
                .filter(|_| path.is_relative())
                .map(|base| base.join(path));
            let staged = self.walk_file(rebased.as_deref().unwrap_or(path), depth)?;
            edited |= staged.is_some();
            if !self.replay {
                continue;
            }
            let Some(value) =
                staged.or_else(|| rebased.map(|path| path.into_os_string().into_vec()))
            else {
                continue;
            };
            if separator.is_some_and(|separator| value.contains(&separator)) {
                return Err(Error::Inspect(unrepresentable_include()));
            }
            let range = paths.subslice_range(field).expect("field from split");
            replacement.replace(range, &value);
        }
        let Some(replacement) = replacement.finish() else {
            return Ok(None);
        };
        if include_base.is_some() && !self.syntax.can_encode(&replacement) {
            return Err(Error::Inspect(unrepresentable_include()));
        }
        Ok(Some((replacement, edited)))
    }

    fn walk_file(&mut self, path: &Path, depth: usize) -> Result<Option<Argument>, Error> {
        if depth > usize::from(self.limits.depth) {
            return Err(Error::Inspect(io::Error::other(
                "options files are nested too deeply",
            )));
        }
        let absolute_path;
        let path = if S::FILE_RELATIVE && path.is_relative() {
            absolute_path = absolute(path).map_err(Error::Inspect)?;
            absolute_path.as_path()
        } else {
            path
        };
        let contents =
            read_regular(path.as_os_str(), &mut self.limits.bytes).map_err(Error::Inspect)?;
        let mut words = self.syntax.decode(&contents).map_err(Error::Inspect)?;
        self.syntax
            .prepare_file(path, &mut words)
            .map_err(Error::Inspect)?;
        let Some(words) = self.walk(Cow::Owned(words), Some(path), depth)? else {
            return Ok(None);
        };
        let contents = self.syntax.encode(&words);
        let path = self
            .files
            .stage(S::STAGED_NAME, &contents)
            .map_err(Error::Edit)?;
        Ok(Some(path))
    }
}

fn unrepresentable_include() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "cannot preserve the path of a response file",
    )
}

#[cfg(test)]
mod tests;
