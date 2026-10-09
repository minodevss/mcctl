use std::io::Read;
use std::path::{Component, Path};

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};

use crate::error::{Error, io_error};

/// Unpacks a `.tar.gz` into the empty directory `dest`. Every entry is checked first: only
/// plain relative names, regular files, directories and links that stay inside `dest`.
pub(crate) fn extract_tar_gz(reader: impl Read, dest: &Path, archive: &Path) -> Result<(), Error> {
    let mut tar = Archive::new(GzDecoder::new(reader));
    tar.set_overwrite(false);
    tar.set_preserve_permissions(false);
    tar.set_preserve_ownerships(false);
    tar.set_unpack_xattrs(false);
    for entry in tar.entries().map_err(io_error(archive))? {
        let mut entry = entry.map_err(io_error(archive))?;
        let path = entry.path().map_err(io_error(archive))?.into_owned();
        let link = entry
            .link_name()
            .map_err(io_error(archive))?
            .map(std::borrow::Cow::into_owned);
        let unsafe_entry = || Error::UnsafeArchive {
            archive: archive.to_path_buf(),
            entry: path.display().to_string(),
        };
        if !entry_is_safe(entry.header().entry_type(), &path, link.as_deref()) {
            return Err(unsafe_entry());
        }
        if !entry.unpack_in(dest).map_err(io_error(dest))? {
            return Err(unsafe_entry());
        }
    }
    Ok(())
}

pub(crate) fn entry_is_safe(kind: EntryType, path: &Path, link: Option<&Path>) -> bool {
    if !stays_relative(path) {
        return false;
    }
    match kind {
        EntryType::Regular
        | EntryType::Continuous
        | EntryType::Directory
        | EntryType::XGlobalHeader
        | EntryType::XHeader => true,
        EntryType::Symlink => link.is_some_and(|target| symlink_stays_inside(path, target)),
        EntryType::Link => link.is_some_and(|target| stays_relative(target) && names(target) > 0),
        _ => false,
    }
}

fn stays_relative(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn names(path: &Path) -> usize {
    path.components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .count()
}

/// True when `target`, resolved from the folder holding `link`, never climbs above the archive root.
pub(crate) fn symlink_stays_inside(link: &Path, target: &Path) -> bool {
    let mut depth = names(link).saturating_sub(1);
    for component in target.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => match depth.checked_sub(1) {
                Some(up) => depth = up,
                None => return false,
            },
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    target.components().next().is_some()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::{Builder, Header};

    use super::*;

    fn header(kind: EntryType, size: u64) -> Header {
        let mut header = Header::new_gnu();
        header.set_entry_type(kind);
        header.set_size(size);
        header.set_mode(0o755);
        header
    }

    fn raw_name(header: &mut Header, name: &str) {
        let field = &mut header.as_old_mut().name;
        field.fill(0);
        field[..name.len()].copy_from_slice(name.as_bytes());
    }

    fn raw_link(header: &mut Header, target: &str) {
        let field = &mut header.as_old_mut().linkname;
        field.fill(0);
        field[..target.len()].copy_from_slice(target.as_bytes());
    }

    fn tar_gz(entries: &[(EntryType, &str, Option<&str>, &[u8])]) -> Vec<u8> {
        let mut builder = Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for (kind, name, link, data) in entries {
            let mut header = header(*kind, data.len() as u64);
            raw_name(&mut header, name);
            if let Some(link) = link {
                raw_link(&mut header, link);
            }
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        let mut gz = builder.into_inner().unwrap();
        gz.flush().unwrap();
        gz.finish().unwrap()
    }

    fn extract(bytes: &[u8]) -> (tempfile::TempDir, Result<(), Error>) {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("out");
        fs::create_dir(&dest).unwrap();
        let result = extract_tar_gz(bytes, &dest, Path::new("jre.tar.gz"));
        (root, result)
    }

    #[test]
    fn extracts_runtime_with_inner_symlinks() {
        let bytes = tar_gz(&[
            (EntryType::Directory, "jdk-21-jre/", None, b""),
            (EntryType::Directory, "jdk-21-jre/bin/", None, b""),
            (EntryType::Regular, "jdk-21-jre/bin/java", None, b"#!java"),
            (
                EntryType::Directory,
                "jdk-21-jre/legal/java.base/",
                None,
                b"",
            ),
            (
                EntryType::Regular,
                "jdk-21-jre/legal/java.base/LICENSE",
                None,
                b"gpl",
            ),
            (
                EntryType::Directory,
                "jdk-21-jre/legal/java.xml/",
                None,
                b"",
            ),
            (
                EntryType::Symlink,
                "jdk-21-jre/legal/java.xml/LICENSE",
                Some("../java.base/LICENSE"),
                b"",
            ),
        ]);
        let (root, result) = extract(&bytes);
        result.unwrap();
        let out = root.path().join("out/jdk-21-jre");
        assert_eq!(fs::read(out.join("bin/java")).unwrap(), b"#!java");
        assert_eq!(
            fs::read(out.join("legal/java.xml/LICENSE")).unwrap(),
            b"gpl"
        );
    }

    #[test]
    fn extract_rejects_parent_dir_entries() {
        let bytes = tar_gz(&[(EntryType::Regular, "jdk/../../evil", None, b"x")]);
        let (root, result) = extract(&bytes);
        assert!(matches!(result, Err(Error::UnsafeArchive { .. })));
        assert!(!root.path().join("evil").exists());
    }

    #[test]
    fn extract_rejects_absolute_entries() {
        let bytes = tar_gz(&[(EntryType::Regular, "/tmp/evil", None, b"x")]);
        let (_root, result) = extract(&bytes);
        assert!(matches!(result, Err(Error::UnsafeArchive { .. })));
    }

    #[test]
    fn extract_rejects_links_out_of_the_archive() {
        let cases = [
            (EntryType::Symlink, "/etc/passwd"),
            (EntryType::Symlink, "../../outside"),
            (EntryType::Link, "../outside"),
        ];
        for (kind, target) in cases {
            let bytes = tar_gz(&[(kind, "jdk/link", Some(target), b"")]);
            let (_root, result) = extract(&bytes);
            assert!(
                matches!(result, Err(Error::UnsafeArchive { .. })),
                "{target}"
            );
        }
    }

    #[test]
    fn extract_rejects_device_files() {
        let bytes = tar_gz(&[(EntryType::Fifo, "jdk/pipe", None, b"")]);
        let (_root, result) = extract(&bytes);
        assert!(matches!(result, Err(Error::UnsafeArchive { .. })));
    }

    #[test]
    fn symlink_targets_are_resolved_from_the_link_folder() {
        let cases = [
            ("a/b/link", "../c", true),
            ("a/b/link", "../../c", true),
            ("a/b/link", "../../../c", false),
            ("link", "c", true),
            ("link", "../c", false),
            ("a/link", "/c", false),
            ("a/link", "", false),
            ("./a/b/link", "../../c", true),
        ];
        for (link, target, inside) in cases {
            assert_eq!(
                symlink_stays_inside(Path::new(link), Path::new(target)),
                inside,
                "{link} -> {target}"
            );
        }
    }
}
