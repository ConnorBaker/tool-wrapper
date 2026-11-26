use super::*;
use std::os::unix::fs::symlink;

#[test]
fn preserves_previous_output_on_edit_error() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let destination = directory.path().join("archive.a");
    fs::write(&destination, b"old")?;

    let pending = PendingOutput::new(&destination)?;
    let staged = pending.path().to_owned();
    assert_eq!(staged.file_name(), destination.file_name());
    fs::write(&staged, b"new")?;
    let error = pending.publish(|output| {
        output.write(0, b"bad")?;
        output.rewrite(|_| Err(io::Error::other("edit failed")))
    });
    assert_eq!(error.unwrap_err().to_string(), "edit failed");
    assert_eq!(fs::read(&destination)?, b"old");
    assert!(!staged.exists());

    let pending = PendingOutput::new(&destination)?;
    fs::write(pending.path(), b"new")?;
    pending.publish(|output| {
        output.rewrite(|bytes| {
            assert_eq!(bytes, b"new");
            Ok(Some(b"longer".to_vec()))
        })?;
        assert_eq!(output.size(), 6);
        assert_eq!(output.read(0, 6)?, b"longer");
        output.rewrite(|_| Ok(Some(b"ok".to_vec())))?;
        assert_eq!(output.size(), 2);
        assert!(output.read(0, 3).is_err());
        Ok(())
    })?;
    assert_eq!(fs::read(&destination)?, b"ok");
    Ok(())
}

#[test]
fn refuses_symlinks_and_hardlinked_outputs() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let destination = directory.path().join("archive.a");
    fs::write(&destination, b"old")?;

    let pending = PendingOutput::new(&destination)?;
    fs::hard_link(&destination, pending.path())?;
    assert!(
        pending
            .publish(|_| panic!("edited an existing inode"))
            .is_err()
    );
    assert_eq!(fs::read(&destination)?, b"old");
    assert_eq!(destination.metadata()?.nlink(), 1);

    let pending = PendingOutput::new(&destination)?;
    symlink(&destination, pending.path())?;
    assert!(pending.publish(|_| panic!("followed a symlink")).is_err());
    assert_eq!(fs::read(&destination)?, b"old");

    let alias = directory.path().join("alias.a");
    symlink(&destination, &alias)?;
    assert!(PendingOutput::new(&alias).is_err());
    Ok(())
}
