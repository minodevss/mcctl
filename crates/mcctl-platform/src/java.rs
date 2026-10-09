use std::fs::{self, File};
use std::io::BufReader;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use ureq::Agent;

use crate::download::{require_https, save_verified};
use crate::error::{Error, io_error};
use crate::extract::extract_tar_gz;
use crate::http::get_json;
use crate::install::Digest;

const ASSETS_API: &str = "https://api.adoptium.net/v3/assets/latest";
const CURRENT: &str = "current";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Target {
    arch: &'static str,
    os: &'static str,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Asset {
    binary: Binary,
    release_name: String,
    version: AssetVersion,
}

#[derive(Debug, Deserialize)]
struct Binary {
    architecture: String,
    image_type: String,
    os: String,
    package: Package,
}

#[derive(Debug, Deserialize)]
struct Package {
    checksum: String,
    link: String,
    name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(default)]
struct AssetVersion {
    major: u32,
    minor: u32,
    security: u32,
    patch: u32,
    build: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Release {
    name: String,
    link: String,
    sha256: String,
}

/// Path `mcctl run` should exec for `major`; does no I/O and may not exist yet.
pub fn java_path(root: &Path, major: u8) -> PathBuf {
    root.join(major.to_string())
        .join(CURRENT)
        .join("bin")
        .join("java")
}

/// Makes `root/<major>/current` point at the newest Temurin JRE and returns its `bin/java`.
/// Only writes under `root`; does nothing when `current` is already the newest release.
pub fn ensure_java(agent: &Agent, root: &Path, major: u8) -> Result<PathBuf, Error> {
    let target = host_target()?;
    let assets: Vec<Asset> = get_json(agent, &asset_url(major, target))?;
    let release = select_release(&assets, target).ok_or(Error::NoJavaBuild {
        major,
        os: target.os,
        arch: target.arch,
    })?;
    let major_dir = root.join(major.to_string());
    fs::create_dir_all(&major_dir).map_err(io_error(&major_dir))?;
    install_release(agent, &major_dir, &release)?;
    Ok(java_path(root, major))
}

pub(crate) fn adoptium_arch(rust_arch: &str) -> Option<&'static str> {
    match rust_arch {
        "x86_64" => Some("x64"),
        "aarch64" => Some("aarch64"),
        _ => None,
    }
}

pub(crate) fn adoptium_os(rust_os: &str, musl: bool) -> Option<&'static str> {
    match (rust_os, musl) {
        ("linux", true) => Some("alpine-linux"),
        ("linux", false) => Some("linux"),
        _ => None,
    }
}

fn host_target() -> Result<Target, Error> {
    let arch = adoptium_arch(std::env::consts::ARCH);
    let os = adoptium_os(std::env::consts::OS, host_uses_musl());
    match (arch, os) {
        (Some(arch), Some(os)) => Ok(Target { arch, os }),
        _ => Err(Error::UnsupportedPlatform {
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
        }),
    }
}

// mcctl itself is static musl, so ask the host's dynamic loader which libc a JRE must target.
fn host_uses_musl() -> bool {
    fs::read_dir("/lib").is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().starts_with("ld-musl-"))
    })
}

fn asset_url(major: u8, target: Target) -> String {
    format!(
        "{ASSETS_API}/{major}/hotspot?architecture={}&image_type=jre&os={}&vendor=eclipse",
        target.arch, target.os
    )
}

pub(crate) fn select_release(assets: &[Asset], target: Target) -> Option<Release> {
    assets
        .iter()
        .filter(|asset| {
            asset.binary.architecture == target.arch
                && asset.binary.os == target.os
                && asset.binary.image_type == "jre"
                && asset.binary.package.name.ends_with(".tar.gz")
                && is_release_name(&asset.release_name)
                && is_sha256(&asset.binary.package.checksum)
        })
        .max_by_key(|asset| asset.version)
        .map(|asset| Release {
            name: asset.release_name.clone(),
            link: asset.binary.package.link.clone(),
            sha256: asset.binary.package.checksum.to_ascii_lowercase(),
        })
}

fn is_release_name(name: &str) -> bool {
    !name.is_empty()
        && name != CURRENT
        && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0'])
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit())
}

fn install_release(agent: &Agent, major_dir: &Path, release: &Release) -> Result<(), Error> {
    let release_dir = major_dir.join(&release.name);
    let current = fs::read_link(major_dir.join(CURRENT)).ok();
    if current.as_deref() == Some(Path::new(&release.name)) && is_runtime(&release_dir) {
        return Ok(());
    }
    if !is_runtime(&release_dir) {
        let archive = scratch_path(major_dir, &release.name, "tar.gz");
        let result = require_https(&release.link)
            .and_then(|()| {
                let digest = Digest::Sha256(release.sha256.clone());
                save_verified(agent, &release.link, Some(&digest), &archive)
            })
            .and_then(|()| unpack_release(&archive, major_dir, &release.name));
        fs::remove_file(&archive).ok();
        result?;
    }
    switch_current(major_dir, &release.name)
}

fn scratch_path(major_dir: &Path, name: &str, suffix: &str) -> PathBuf {
    major_dir.join(format!(".{name}.{}.{suffix}", std::process::id()))
}

fn is_runtime(dir: &Path) -> bool {
    fs::symlink_metadata(dir.join("bin").join("java")).is_ok_and(|metadata| metadata.is_file())
}

/// Extracts `archive` beside the final folder, then renames its single top folder into place.
pub(crate) fn unpack_release(archive: &Path, major_dir: &Path, name: &str) -> Result<(), Error> {
    let partial = scratch_path(major_dir, name, "partial");
    let result = unpack_into(archive, &partial, &major_dir.join(name));
    fs::remove_dir_all(&partial).ok();
    result
}

fn unpack_into(archive: &Path, partial: &Path, release_dir: &Path) -> Result<(), Error> {
    fs::create_dir(partial).map_err(io_error(partial))?;
    let file = File::open(archive).map_err(io_error(archive))?;
    extract_tar_gz(BufReader::new(file), partial, archive)?;
    let runtime = single_runtime_dir(partial, archive)?;
    match fs::rename(&runtime, release_dir) {
        Ok(()) => Ok(()),
        Err(_) if is_runtime(release_dir) => Ok(()),
        Err(source) => Err(io_error(release_dir)(source)),
    }
}

fn single_runtime_dir(partial: &Path, archive: &Path) -> Result<PathBuf, Error> {
    let layout_error = || Error::ArchiveLayout {
        archive: archive.to_path_buf(),
    };
    let mut entries = fs::read_dir(partial).map_err(io_error(partial))?;
    let only = entries
        .next()
        .ok_or_else(layout_error)?
        .map_err(io_error(partial))?;
    if entries.next().is_some() {
        return Err(layout_error());
    }
    let dir = only.path();
    let is_dir = only.file_type().is_ok_and(|kind| kind.is_dir());
    if is_dir && is_runtime(&dir) {
        Ok(dir)
    } else {
        Err(layout_error())
    }
}

/// Points `current` at `name` by renaming a fresh symlink over it, so readers never see it missing.
pub(crate) fn switch_current(major_dir: &Path, name: &str) -> Result<(), Error> {
    let temp = scratch_path(major_dir, CURRENT, "link");
    fs::remove_file(&temp).ok();
    symlink(name, &temp).map_err(io_error(&temp))?;
    let current = major_dir.join(CURRENT);
    fs::rename(&temp, &current).map_err(|source| {
        fs::remove_file(&temp).ok();
        io_error(&current)(source)
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::*;

    const ASSETS: &str = include_str!("../tests/fixtures/adoptium_21_x64_linux.json");
    const X64_LINUX: Target = Target {
        arch: "x64",
        os: "linux",
    };

    #[test]
    fn parses_adoptium_asset() {
        let assets: Vec<Asset> = serde_json::from_str(ASSETS).unwrap();
        assert_eq!(
            select_release(&assets, X64_LINUX),
            Some(Release {
                name: "jdk-21.0.12.1+1".to_owned(),
                link: "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jre_x64_linux_hotspot_21.0.12.1_1.tar.gz".to_owned(),
                sha256: "2413149700df0f7d440500a84a8f764c535f21e5a5e87d38328b64eec2c5b500".to_owned(),
            })
        );
        let alpine = Target {
            arch: "x64",
            os: "alpine-linux",
        };
        assert_eq!(select_release(&assets, alpine), None);
    }

    #[test]
    fn selects_newest_release_with_safe_name() {
        let asset = |name: &str, security: u32| {
            format!(
                r#"{{"release_name":"{name}","version":{{"major":21,"minor":0,"security":{security},"build":1}},
                "binary":{{"architecture":"x64","image_type":"jre","os":"linux",
                "package":{{"checksum":"{sum}","link":"https://x/{name}.tar.gz","name":"{name}.tar.gz"}}}}}}"#,
                sum = "a".repeat(64)
            )
        };
        let json = format!(
            "[{},{},{},{}]",
            asset("jdk-21.0.11+1", 11),
            asset("jdk-21.0.12+1", 12),
            asset("..", 13),
            asset("current", 14)
        );
        let assets: Vec<Asset> = serde_json::from_str(&json).unwrap();
        assert_eq!(
            select_release(&assets, X64_LINUX).unwrap().name,
            "jdk-21.0.12+1"
        );
    }

    #[test]
    fn maps_host_to_adoptium_names() {
        assert_eq!(adoptium_arch("x86_64"), Some("x64"));
        assert_eq!(adoptium_arch("aarch64"), Some("aarch64"));
        assert_eq!(adoptium_arch("riscv64"), None);
        assert_eq!(adoptium_os("linux", false), Some("linux"));
        assert_eq!(adoptium_os("linux", true), Some("alpine-linux"));
        assert_eq!(adoptium_os("macos", false), None);
    }

    #[test]
    fn java_path_goes_through_current_link() {
        assert_eq!(
            java_path(Path::new("/var/lib/mcctl/java"), 21),
            PathBuf::from("/var/lib/mcctl/java/21/current/bin/java")
        );
    }

    fn runtime_archive(dir: &Path, top: &str) -> PathBuf {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o755);
        builder
            .append_data(&mut header, format!("{top}/bin/java"), &b"java\n"[..])
            .unwrap();
        let mut gz = builder.into_inner().unwrap();
        gz.flush().unwrap();
        let path = dir.join("jre.tar.gz");
        fs::write(&path, gz.finish().unwrap()).unwrap();
        path
    }

    #[test]
    fn unpacks_runtime_and_switches_current() {
        let root = tempfile::tempdir().unwrap();
        let archive = runtime_archive(root.path(), "jdk-21.0.12.1+1-jre");
        let major_dir = root.path().join("21");
        fs::create_dir(&major_dir).unwrap();

        unpack_release(&archive, &major_dir, "jdk-21.0.12.1+1").unwrap();
        switch_current(&major_dir, "jdk-21.0.12.1+1").unwrap();

        let java = java_path(root.path(), 21);
        assert_eq!(fs::read(&java).unwrap(), b"java\n");
        let names: Vec<_> = fs::read_dir(&major_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert_eq!(names, ["current", "jdk-21.0.12.1+1"]);
    }

    #[test]
    fn switching_current_replaces_old_link() {
        let root = tempfile::tempdir().unwrap();
        switch_current(root.path(), "jdk-21.0.11+1").unwrap();
        switch_current(root.path(), "jdk-21.0.12+1").unwrap();
        assert_eq!(
            fs::read_link(root.path().join("current")).unwrap(),
            PathBuf::from("jdk-21.0.12+1")
        );
    }

    #[test]
    fn archive_without_single_runtime_folder_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        builder
            .append_data(&mut header, "README", &b"x"[..])
            .unwrap();
        let archive = root.path().join("bad.tar.gz");
        fs::write(&archive, builder.into_inner().unwrap().finish().unwrap()).unwrap();
        let major_dir = root.path().join("21");
        fs::create_dir(&major_dir).unwrap();

        let result = unpack_release(&archive, &major_dir, "jdk-21");
        assert!(matches!(result, Err(Error::ArchiveLayout { .. })));
        assert_eq!(fs::read_dir(&major_dir).unwrap().count(), 0);
    }
}
