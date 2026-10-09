use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, DirBuilder, File, Metadata, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, lchown};
use std::path::{Path, PathBuf};

use crate::error::Error;

/// A Linux account as numeric ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Owner {
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

/// Replaces `path` so readers see either the old or the new contents, never a partial file.
pub(crate) fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> Result<(), Error> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut temp_name = OsString::from(".");
    temp_name.push(path.file_name().unwrap_or_default());
    temp_name.push(format!(".tmp-{}", std::process::id()));
    let temp = dir.join(temp_name);
    fs::remove_file(&temp).ok();
    let written = write_new(&temp, contents, mode).and_then(|()| {
        fs::rename(&temp, path).map_err(Error::io(path))?;
        sync_dir(dir)
    });
    if written.is_err() {
        fs::remove_file(&temp).ok();
    }
    written
}

fn write_new(path: &Path, contents: &[u8], mode: u32) -> Result<(), Error> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(Error::io(path))?;
    file.set_permissions(Permissions::from_mode(mode))
        .map_err(Error::io(path))?;
    file.write_all(contents).map_err(Error::io(path))?;
    file.sync_all().map_err(Error::io(path))
}

fn sync_dir(dir: &Path) -> Result<(), Error> {
    File::open(dir)
        .and_then(|file| file.sync_all())
        .map_err(Error::io(dir))
}

/// Reads a text file; a missing file is `None`.
pub(crate) fn read_optional(path: &Path) -> Result<Option<String>, Error> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::io(path)(err)),
    }
}

pub(crate) fn lstat(path: &Path) -> Result<Option<Metadata>, Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::io(path)(err)),
    }
}

pub(crate) fn exists_nofollow(path: &Path) -> Result<bool, Error> {
    Ok(lstat(path)?.is_some())
}

/// Removes a folder tree without following symlinks inside it; a missing path is fine.
pub(crate) fn remove_tree(path: &Path) -> Result<(), Error> {
    match lstat(path)? {
        None => Ok(()),
        Some(metadata) if metadata.is_dir() => fs::remove_dir_all(path).map_err(Error::io(path)),
        Some(_) => fs::remove_file(path).map_err(Error::io(path)),
    }
}

pub(crate) fn remove_file_if_exists(path: &Path) -> Result<(), Error> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(Error::io(path)(err)),
        Ok(()) | Err(_) => Ok(()),
    }
}

pub(crate) fn create_private_dir(path: &Path) -> Result<(), Error> {
    DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(Error::io(path))
}

/// Copies regular files and folders from `src` into the existing folder `dst`.
/// Symlinks and special files are skipped; returns how many were skipped.
pub(crate) fn copy_contents(src: &Path, dst: &Path) -> Result<u64, Error> {
    let mut skipped = 0;
    for entry in fs::read_dir(src).map_err(Error::io(src))? {
        let entry = entry.map_err(Error::io(src))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry.file_type().map_err(Error::io(&from))?;
        if file_type.is_dir() {
            create_private_dir(&to)?;
            skipped += copy_contents(&from, &to)?;
        } else if file_type.is_file() {
            if !copy_regular_file(&from, &to)? {
                skipped += 1;
            }
        } else {
            skipped += 1;
        }
    }
    Ok(skipped)
}

/// Copies `from` only if it is still the regular file that was listed; returns false otherwise.
fn copy_regular_file(from: &Path, to: &Path) -> Result<bool, Error> {
    let Some(before) = lstat(from)? else {
        return Ok(false);
    };
    if !before.is_file() {
        return Ok(false);
    }
    let mut source = File::open(from).map_err(Error::io(from))?;
    let opened = source.metadata().map_err(Error::io(from))?;
    // A swap to a symlink between lstat and open shows up as a different inode.
    if opened.dev() != before.dev() || opened.ino() != before.ino() {
        return Ok(false);
    }
    let mut target = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(to)
        .map_err(Error::io(to))?;
    io::copy(&mut source, &mut target).map_err(Error::io(to))?;
    target
        .set_permissions(Permissions::from_mode(before.mode() & 0o777))
        .map_err(Error::io(to))?;
    Ok(true)
}

/// Gives every entry under `root` to `owner` and copies owner permissions to the group;
/// folders get setgid so new files keep the server group. Never follows symlinks.
pub(crate) fn hand_over(root: &Path, owner: Owner) -> Result<(), Error> {
    let Some(metadata) = lstat(root)? else {
        return Ok(());
    };
    lchown(root, Some(owner.uid), Some(owner.gid)).map_err(Error::io(root))?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    let mode = group_mode(metadata.mode(), metadata.is_dir());
    fs::set_permissions(root, Permissions::from_mode(mode)).map_err(Error::io(root))?;
    if metadata.is_dir() {
        for entry in fs::read_dir(root).map_err(Error::io(root))? {
            let entry = entry.map_err(Error::io(root))?;
            hand_over(&entry.path(), owner)?;
        }
    }
    Ok(())
}

/// Owner bits copied to the group, nothing for others, setgid on folders.
pub(crate) fn group_mode(mode: u32, is_dir: bool) -> u32 {
    let user = mode & 0o700;
    let shared = user | (user >> 3);
    if is_dir { shared | 0o2000 } else { shared }
}

/// Names of regular `*.log` files directly inside `dir`.
pub(crate) fn root_log_files(dir: &Path) -> Result<BTreeSet<PathBuf>, Error> {
    let mut logs = BTreeSet::new();
    for entry in fs::read_dir(dir).map_err(Error::io(dir))? {
        let entry = entry.map_err(Error::io(dir))?;
        let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
        if is_file && entry.file_name().to_string_lossy().ends_with(".log") {
            logs.insert(PathBuf::from(entry.file_name()));
        }
    }
    Ok(logs)
}

/// True if `dir` directly holds a regular `*.jar` file.
pub(crate) fn has_jar_files(dir: &Path) -> Result<bool, Error> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(Error::io(dir)(err)),
    };
    for entry in entries {
        let entry = entry.map_err(Error::io(dir))?;
        let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
        if is_file
            && entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".jar")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn writes_atomically_with_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("survival.toml");
        write_atomic(&path, b"one", 0o644).unwrap();
        write_atomic(&path, b"two", 0o644).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o644);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn copies_files_and_folders_but_not_symlinks() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::create_dir(src.path().join("world")).unwrap();
        fs::write(src.path().join("world/level.dat"), "level").unwrap();
        fs::write(src.path().join("server.jar"), "jar").unwrap();
        symlink("/etc/passwd", src.path().join("passwd")).unwrap();
        symlink("/etc", src.path().join("world/etc")).unwrap();
        let skipped = copy_contents(src.path(), dst.path()).unwrap();
        assert_eq!(skipped, 2);
        assert_eq!(
            fs::read_to_string(dst.path().join("world/level.dat")).unwrap(),
            "level"
        );
        assert!(dst.path().join("server.jar").is_file());
        assert!(!exists_nofollow(&dst.path().join("passwd")).unwrap());
        assert!(!exists_nofollow(&dst.path().join("world/etc")).unwrap());
    }

    #[test]
    fn group_gets_owner_permissions() {
        let cases = [
            (0o644, false, 0o660),
            (0o755, false, 0o770),
            (0o600, false, 0o660),
            (0o700, true, 0o2770),
            (0o755, true, 0o2770),
            (0o4755, false, 0o770),
        ];
        for (mode, is_dir, expected) in cases {
            assert_eq!(group_mode(mode, is_dir), expected, "{mode:o}");
        }
    }

    #[test]
    fn finds_jars_and_logs() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("mods")).unwrap();
        assert!(!has_jar_files(&dir.path().join("mods")).unwrap());
        assert!(!has_jar_files(&dir.path().join("plugins")).unwrap());
        fs::write(dir.path().join("mods/readme.txt"), "").unwrap();
        assert!(!has_jar_files(&dir.path().join("mods")).unwrap());
        fs::write(dir.path().join("mods/fabric-api.JAR"), "").unwrap();
        assert!(has_jar_files(&dir.path().join("mods")).unwrap());
        fs::write(dir.path().join("installer.log"), "").unwrap();
        assert_eq!(
            root_log_files(dir.path()).unwrap(),
            BTreeSet::from([PathBuf::from("installer.log")])
        );
    }
}
