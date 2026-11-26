use crate::artifact::Artifact;
use crate::path_prefix_map::PathPrefixMap;
use rustix::fs::{MemfdFlags, Mode, OFlags};
use rustix::io::{FdFlags, fcntl_setfd, retry_on_intr};
use std::env;
use std::ffi::{CStr, OsStr, OsString};
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, IntoRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn descriptor_path(file: &File) -> String {
    format!("/proc/self/fd/{}", file.as_raw_fd())
}

/// Create an empty, seekable file with close-on-exec set. It can be passed
/// to `Command` as standard I/O without inheriting its original descriptor.
pub fn anonymous_file(name: &CStr) -> io::Result<File> {
    Ok(File::from(rustix::fs::memfd_create(
        name,
        MemfdFlags::CLOEXEC,
    )?))
}

/// Create an anonymous input whose descriptor is inherited by all children
/// while the file is open. Use `InputFiles::stage` for command-scoped inheritance.
pub fn anonymous_input(name: &CStr, contents: &[u8]) -> io::Result<File> {
    let mut file = File::from(rustix::fs::memfd_create(name, MemfdFlags::empty())?);
    file.write_all(contents)?;
    Ok(file)
}

/// Owns anonymous files named in arguments. Commands prepared by
/// `Tool::command` inherit them and keep their descriptor paths valid.
#[derive(Clone, Default)]
pub struct InputFiles(Vec<Arc<File>>);

impl InputFiles {
    pub fn stage(&mut self, name: &CStr, contents: &[u8]) -> io::Result<Vec<u8>> {
        let file = anonymous_file(name)?;
        file.write_all_at(contents, 0)?;
        Ok(self.register(file))
    }

    /// Reserve a path now and fill it through the returned handle before launch.
    /// Positioned writes leave the command's file offset at the beginning.
    pub fn reserve(&mut self, name: &CStr) -> io::Result<(Vec<u8>, File)> {
        let file = anonymous_file(name)?;
        Ok((self.register(file.try_clone()?), file))
    }

    fn register(&mut self, file: File) -> Vec<u8> {
        let path = descriptor_path(&file).into_bytes();
        self.0.push(Arc::new(file));
        path
    }

    /// Stage edited contents. Unreadable inputs and inputs with no edits use the original path.
    pub fn stage_rewrite(
        &mut self,
        name: &CStr,
        path: &OsStr,
        rewrite: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
    ) -> io::Result<Option<Vec<u8>>> {
        let mut remaining = usize::MAX;
        let Ok(contents) = read_regular(path, &mut remaining) else {
            return Ok(None);
        };
        rewrite(&contents)
            .map(|contents| self.stage(name, &contents))
            .transpose()
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        self.0.truncate(len);
    }

    pub(crate) fn set_flags(&self, flags: FdFlags) -> io::Result<()> {
        for file in &self.0 {
            fcntl_setfd(&**file, flags)?;
        }
        Ok(())
    }

    pub(crate) fn append(&mut self, mut other: Self) {
        self.0.append(&mut other.0);
    }
}

/// Return an absolute mapped path that names the current working directory.
pub fn mapped_working_directory(paths: &PathPrefixMap) -> Option<PathBuf> {
    let cwd = env::current_dir().ok()?;
    let mapped = PathBuf::from(OsString::from_vec(paths.remap(cwd.as_os_str().as_bytes())?));
    if !mapped.is_absolute() {
        return None;
    }
    same_file(&fs::metadata(".").ok()?, &mapped.metadata().ok()?).then_some(mapped)
}

pub fn same_file(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino()
}

fn same_version(a: &Metadata, b: &Metadata) -> bool {
    same_file(a, b)
        && a.len() == b.len()
        && (a.mtime(), a.mtime_nsec(), a.ctime(), a.ctime_nsec())
            == (b.mtime(), b.mtime_nsec(), b.ctime(), b.ctime_nsec())
}

pub fn special_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|info| !info.is_file())
}

pub(crate) fn open_nonblocking(path: impl AsRef<Path>, flags: OFlags) -> io::Result<File> {
    let flags = flags | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let fd = retry_on_intr(|| rustix::fs::open(path.as_ref(), flags, Mode::empty()))?;
    Ok(File::from(fd))
}

pub fn read_regular(path: &OsStr, remaining: &mut usize) -> io::Result<Vec<u8>> {
    let not_regular = || {
        io::Error::other(format!(
            "input file {} is missing or not a regular file",
            path.to_string_lossy()
        ))
    };
    if !fs::metadata(path).is_ok_and(|info| info.is_file()) {
        return Err(not_regular());
    }
    // A FIFO swapped in after metadata() must not make inspection block.
    let mut file = open_nonblocking(path, OFlags::RDONLY)?;
    let info = file.metadata()?;
    if !info.is_file() {
        return Err(not_regular());
    }
    let size = usize::try_from(info.len())
        .ok()
        .filter(|&size| size <= *remaining)
        .ok_or_else(|| io::Error::other("input exceeds the inspection size limit"))?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).map_err(io::Error::other)?;
    bytes.resize(size, 0);
    file.read_exact(&mut bytes)?;
    *remaining -= size;
    Ok(bytes)
}

/// Observe a direct output path before a tool starts and edit the new version.
/// A relative path requires the working directory to stay the same until editing.
pub struct GeneratedFile {
    path: PathBuf,
    before: Option<Metadata>,
}

impl GeneratedFile {
    pub fn new(path: PathBuf) -> io::Result<Self> {
        let before = fs::metadata(&path).ok();
        if before.as_ref().is_some_and(|info| !info.is_file()) {
            return Err(io::Error::other(format!(
                "{} is not a regular file",
                path.display()
            )));
        }
        Ok(Self { path, before })
    }

    pub fn edit(
        self,
        tool: &str,
        update: impl FnOnce(&mut Artifact<'_>) -> io::Result<()>,
    ) -> io::Result<()> {
        let path = &self.path;
        let after = path
            .metadata()
            .ok()
            .filter(|info| {
                info.is_file()
                    && self
                        .before
                        .as_ref()
                        .is_none_or(|before| !same_version(before, info))
            })
            .ok_or_else(|| io::Error::other(format!("{tool} didn't write {}", path.display())))?;
        let mut verified = false;
        (|| {
            let file = open_nonblocking(path, OFlags::RDWR)?;
            let info = file.metadata()?;
            if !info.is_file() || !same_version(&after, &info) {
                return Err(io::Error::other(format!(
                    "{path:?} changed before it could be opened"
                )));
            }
            verified = true;
            update(&mut Artifact::File {
                file: &file,
                size: after.len(),
            })?;
            // SAFETY: into_raw_fd consumes the sole owner, and Linux closes
            // the descriptor even if close reports EINTR.
            if unsafe { libc::close(file.into_raw_fd()) } < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
            Ok(())
        })()
        .inspect_err(|_| {
            if path.metadata().is_ok_and(|current| {
                if verified {
                    same_file(&after, &current)
                } else {
                    same_version(&after, &current)
                }
            }) {
                let _ = fs::remove_file(path);
            }
        })
    }

    pub fn rewrite_text(
        self,
        tool: &str,
        rewrite: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
    ) -> io::Result<()> {
        self.edit(tool, |output| output.rewrite(|bytes| Ok(rewrite(bytes))))
    }
}
