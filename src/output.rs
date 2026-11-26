//! Publish a newly generated artifact through a same-filesystem rename.

use crate::artifact::Artifact;
use crate::files::{open_nonblocking, same_file};
use rustix::fs::OFlags;
use std::fs::{self, Permissions};
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf, absolute};
use tempfile::{Builder, TempDir};

/// A private path for a producer that writes a fresh, single-file output.
///
/// Publishing replaces the destination entry. Keep its parent directory
/// stable during the operation. The staged path has the requested basename
/// in a new sibling directory. Tools can record that directory in outputs.
pub struct PendingOutput {
    destination: PathBuf,
    staged: PathBuf,
    _directory: TempDir,
}

impl PendingOutput {
    pub fn new(destination: &Path) -> io::Result<Self> {
        let destination = absolute(destination)?;
        let Some((parent, basename)) = destination.parent().zip(destination.file_name()) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid output path",
            ));
        };
        check_destination(&destination)?;
        let directory = Builder::new()
            .prefix(".tool-wrapper-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir_in(parent)?;
        let staged = directory.path().join(basename);
        Ok(Self {
            destination,
            staged,
            _directory: directory,
        })
    }

    /// Pass this path to the producer in place of the destination.
    pub fn path(&self) -> &Path {
        &self.staged
    }

    /// Edit, sync, and rename the produced file. Readers see a complete file
    /// at the destination. Concurrent successful publishers replace it in
    /// rename order. This does not sync the destination directory.
    pub fn publish(self, edit: impl FnOnce(&mut Artifact<'_>) -> io::Result<()>) -> io::Result<()> {
        let file = open_nonblocking(&self.staged, OFlags::RDWR | OFlags::NOFOLLOW)?;
        let info = file.metadata()?;
        if !info.is_file() || info.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected a new regular output file",
            ));
        }
        edit(&mut Artifact::File {
            file: &file,
            size: info.len(),
        })?;
        file.sync_all()?;
        let current = self.staged.symlink_metadata()?;
        if !current.is_file() || current.nlink() != 1 || !same_file(&info, &current) {
            return Err(io::Error::other("staged output changed before publication"));
        }
        check_destination(&self.destination)?;
        fs::rename(&self.staged, &self.destination)
    }
}

fn check_destination(path: &Path) -> io::Result<()> {
    match path.symlink_metadata() {
        Ok(info) if info.is_file() => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "output path must name a regular file",
        )),
    }
}

#[cfg(test)]
mod tests;
