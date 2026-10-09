use std::fs::{self, File, OpenOptions, TryLockError};
use std::os::unix::fs::OpenOptionsExt;

use crate::error::Error;
use crate::paths::Paths;

/// Held for the whole of a mutating command; the kernel drops it when the process exits.
#[derive(Debug)]
pub(crate) struct Lock {
    _file: File,
}

pub(crate) fn acquire(paths: &Paths) -> Result<Lock, Error> {
    let dir = paths.config_dir();
    fs::create_dir_all(&dir).map_err(Error::io(&dir))?;
    let path = paths.lock_file();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .map_err(Error::io(&path))?;
    match file.try_lock() {
        Ok(()) => Ok(Lock { _file: file }),
        Err(TryLockError::WouldBlock) => Err(Error::Locked { path }),
        Err(TryLockError::Error(err)) => Err(Error::io(&path)(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_is_refused_until_the_first_is_dropped() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::under(root.path());
        let first = acquire(&paths).unwrap();
        assert!(matches!(acquire(&paths), Err(Error::Locked { .. })));
        drop(first);
        assert!(acquire(&paths).is_ok());
    }
}
