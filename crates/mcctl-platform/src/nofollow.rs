use std::fs::{self, File, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

use crate::error::{Error, io_error};

/// True for a non-empty path made only of plain names: no root, `.`, `..` or prefix.
pub(crate) fn is_plain_relative(path: &Path) -> bool {
    path.components().next().is_some()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn lstat(path: &Path) -> Result<Option<Metadata>, Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if is_absent(&error) => Ok(None),
        Err(source) => Err(io_error(path)(source)),
    }
}

fn is_absent(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

/// True when `base/rel` and every folder on the way are real directories, not symlinks.
pub(crate) fn is_real_dir(base: &Path, rel: &Path) -> Result<bool, Error> {
    let mut current = base.to_path_buf();
    for component in rel.components() {
        current.push(component);
        match lstat(&current)? {
            Some(metadata) if metadata.file_type().is_dir() => {}
            Some(_) | None => return Ok(false),
        }
    }
    Ok(true)
}

/// Opens `base/rel` only if it is a regular file reached without any symlink; otherwise `None`.
pub(crate) fn open_regular(base: &Path, rel: &Path) -> Result<Option<File>, Error> {
    if !is_plain_relative(rel) {
        return Ok(None);
    }
    if let Some(parent) = rel.parent()
        && !is_real_dir(base, parent)?
    {
        return Ok(None);
    }
    let path = base.join(rel);
    let Some(before) = lstat(&path)? else {
        return Ok(None);
    };
    if !before.file_type().is_file() {
        return Ok(None);
    }
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if is_absent(&error) => return Ok(None),
        Err(source) => return Err(io_error(&path)(source)),
    };
    let after = file.metadata().map_err(io_error(&path))?;
    // A swap to a symlink between lstat and open shows up as a different inode.
    Ok((before.dev() == after.dev() && before.ino() == after.ino()).then_some(file))
}

/// Creates `base/rel` one level at a time, refusing to pass through anything but real directories.
pub(crate) fn create_real_dirs(base: &Path, rel: &Path) -> Result<(), Error> {
    let mut current = base.to_path_buf();
    for component in rel.components() {
        current.push(component);
        match lstat(&current)? {
            Some(metadata) if metadata.file_type().is_dir() => {}
            Some(_) => return Err(Error::UnsafePath { path: current }),
            None => fs::create_dir(&current).map_err(io_error(&current))?,
        }
    }
    Ok(())
}

pub(crate) fn exists_nofollow(path: &Path) -> Result<bool, Error> {
    Ok(lstat(path)?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_plain_relative_paths() {
        let cases = [
            ("server.jar", true),
            (".fabric/server/1.21.1-server.jar", true),
            ("", false),
            ("/etc/passwd", false),
            ("../server.jar", false),
            ("a/../../b", false),
            ("./server.jar", false),
        ];
        for (path, plain) in cases {
            assert_eq!(is_plain_relative(Path::new(path)), plain, "{path}");
        }
    }
}
