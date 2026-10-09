use std::cmp::Ordering;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use minisign_verify::{PublicKey, Signature};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::commands::Ctx;
use crate::error::Error;
use crate::files;
use crate::http;
use crate::install;
use crate::lock;
use crate::output;
use crate::paths::Paths;
use crate::status_file;
use crate::systemd::{self, ROUTER_UNIT};
use crate::version;

pub(crate) const RELEASE_PUBLIC_KEY: &str =
    "RWRQWwCQGpxyfYQsg66tP0ow+D77z9iGy/Z002ACeLOapSoviRUBeXlW";
const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/minodevss/mcctl/releases/latest";
const DOWNLOAD_URL: &str = "https://github.com/minodevss/mcctl/releases/download";
const SUMS: &str = "SHA256SUMS";
const SUMS_SIGNATURE: &str = "SHA256SUMS.minisig";
const UNIT_FILES: [&str; 2] = ["mcctl.service", "mc@.service"];
const SMALL_FILE_LIMIT: u64 = 1024 * 1024;
const BINARY_LIMIT: u64 = 256 * 1024 * 1024;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

struct Download {
    name: String,
    bytes: Vec<u8>,
}

pub(crate) fn self_update(ctx: &Ctx<'_>) -> Result<(), Error> {
    let _lock = lock::acquire(ctx.paths)?;
    let arch = std::env::consts::ARCH;
    let target = target_triple(arch).ok_or_else(|| Error::UnsupportedArch {
        arch: arch.to_owned(),
    })?;
    let agent = http::agent();
    let current = env!("CARGO_PKG_VERSION");
    let release: Release = http::get_json(&agent, LATEST_RELEASE_URL)?;
    let tag = release.tag_name;
    let latest = tag.strip_prefix('v').unwrap_or(&tag);
    match version::compare(latest, current) {
        Some(Ordering::Greater) => {
            install_release(ctx, &agent, &tag, target)?;
            output::info(format!("updated mcctl {current} -> {latest}"));
            restart_router(ctx)?;
        }
        Some(Ordering::Equal | Ordering::Less) => {
            output::info(format!("mcctl {current} is up to date"));
        }
        None => return Err(Error::BadTag { tag }),
    }
    refresh_all_java(ctx.paths, &agent)
}

pub(crate) fn target_triple(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("x86_64-unknown-linux-musl"),
        "aarch64" => Some("aarch64-unknown-linux-musl"),
        _ => None,
    }
}

fn install_release(
    ctx: &Ctx<'_>,
    agent: &ureq::Agent,
    tag: &str,
    target: &str,
) -> Result<(), Error> {
    let base = format!("{DOWNLOAD_URL}/{tag}");
    let fetch = |name: &str, limit: u64| {
        http::get_bytes(agent, &format!("{base}/{name}"), limit).map(|bytes| Download {
            name: name.to_owned(),
            bytes,
        })
    };
    output::info(format!("downloading mcctl {tag}"));
    let sums = fetch(SUMS, SMALL_FILE_LIMIT)?;
    let signature = fetch(SUMS_SIGNATURE, SMALL_FILE_LIMIT)?;
    verify_signature(
        RELEASE_PUBLIC_KEY,
        &sums.bytes,
        &String::from_utf8_lossy(&signature.bytes),
    )?;
    let sums_text = String::from_utf8_lossy(&sums.bytes).into_owned();
    let binary = fetch(&format!("mcctl-{target}"), BINARY_LIMIT)?;
    let units = UNIT_FILES
        .iter()
        .map(|name| fetch(name, SMALL_FILE_LIMIT))
        .collect::<Result<Vec<_>, _>>()?;
    for download in std::iter::once(&binary).chain(&units) {
        verify_checksum(&sums_text, download)?;
    }
    files::write_atomic(&ctx.paths.binary(), &binary.bytes, 0o755)?;
    let unit_dir = ctx.paths.unit_dir();
    fs::create_dir_all(&unit_dir).map_err(Error::io(&unit_dir))?;
    for unit in &units {
        files::write_atomic(&unit_dir.join(&unit.name), &unit.bytes, 0o644)?;
    }
    systemd::daemon_reload()
}

fn restart_router(ctx: &Ctx<'_>) -> Result<(), Error> {
    if !systemd::is_active(ROUTER_UNIT) {
        return Ok(());
    }
    let connections =
        status_file::read(ctx.paths).map_or(0, |published| published.total_connections());
    let go_ahead = connections == 0
        || output::confirm(
            format!(
                "Restarting the router drops {}. Continue?",
                status_file::counted(connections, "connection")
            ),
            ctx.yes,
        )?;
    if go_ahead {
        systemd::restart(ROUTER_UNIT)?;
        output::info("restarted the router");
    } else {
        output::info("restart the router later: sudo systemctl restart mcctl");
    }
    Ok(())
}

/// Running servers keep their current Java until they restart.
fn refresh_all_java(paths: &Paths, agent: &ureq::Agent) -> Result<(), Error> {
    for major in installed_java_majors(&paths.java_root())? {
        install::refresh_java(agent, paths, major)?;
    }
    Ok(())
}

fn installed_java_majors(root: &Path) -> Result<Vec<u8>, Error> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(Error::io(root)(err)),
    };
    let mut majors = Vec::new();
    for entry in entries {
        let entry = entry.map_err(Error::io(root))?;
        if let Some(major) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        {
            majors.push(major);
        }
    }
    majors.sort_unstable();
    Ok(majors)
}

pub(crate) fn verify_signature(
    public_key: &str,
    data: &[u8],
    signature: &str,
) -> Result<(), Error> {
    let fail = |reason: minisign_verify::Error| Error::Signature {
        file: SUMS.to_owned(),
        reason: reason.to_string(),
    };
    let key = PublicKey::from_base64(public_key).map_err(fail)?;
    let signature = Signature::decode(signature).map_err(fail)?;
    key.verify(data, &signature, false).map_err(fail)
}

fn verify_checksum(sums: &str, download: &Download) -> Result<(), Error> {
    let expected = sha256_for(sums, &download.name).ok_or_else(|| Error::ChecksumMissing {
        file: download.name.clone(),
    })?;
    if sha256_hex(&download.bytes) == expected {
        Ok(())
    } else {
        Err(Error::ChecksumMismatch {
            file: download.name.clone(),
        })
    }
}

/// The lowercase hex digest `sha256sum` printed for `file`, in text or binary mode.
pub(crate) fn sha256_for(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (digest, name) = line.trim_end().split_once(' ')?;
        let name = name.strip_prefix([' ', '*']).unwrap_or(name);
        (name == file && digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| digest.to_ascii_lowercase())
    })
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            write!(hex, "{byte:02x}").ok();
            hex
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const FIXTURE_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const FIXTURE_SIGNATURE: &str = include_str!("../../tests/fixtures/test.minisig");

    #[test]
    fn self_update_target_triple() {
        assert_eq!(target_triple("x86_64"), Some("x86_64-unknown-linux-musl"));
        assert_eq!(target_triple("aarch64"), Some("aarch64-unknown-linux-musl"));
        assert_eq!(target_triple("riscv64"), None);
    }

    #[test]
    fn sha256sums_lookup() {
        let sums = format!(
            "{EMPTY_SHA256}  mcctl-x86_64-unknown-linux-musl\n\
             {}  mc@.service\n\
             {} *mcctl.service\n",
            "A".repeat(64),
            "b".repeat(64)
        );
        assert_eq!(
            sha256_for(&sums, "mcctl-x86_64-unknown-linux-musl").as_deref(),
            Some(EMPTY_SHA256)
        );
        assert_eq!(sha256_for(&sums, "mc@.service"), Some("a".repeat(64)));
        assert_eq!(sha256_for(&sums, "mcctl.service"), Some("b".repeat(64)));
        assert_eq!(sha256_for(&sums, "mcctl-aarch64-unknown-linux-musl"), None);
        assert_eq!(sha256_for(&sums, "service"), None);
        assert_eq!(sha256_for("short  file\n", "file"), None);
        assert_eq!(sha256_hex(b""), EMPTY_SHA256);
        let download = Download {
            name: "mcctl-x86_64-unknown-linux-musl".to_owned(),
            bytes: Vec::new(),
        };
        assert!(verify_checksum(&sums, &download).is_ok());
        let tampered = Download {
            bytes: b"x".to_vec(),
            ..download
        };
        assert!(matches!(
            verify_checksum(&sums, &tampered),
            Err(Error::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn verifies_minisign_signature() {
        assert!(PublicKey::from_base64(RELEASE_PUBLIC_KEY).is_ok());
        assert!(verify_signature(FIXTURE_KEY, b"test", FIXTURE_SIGNATURE).is_ok());
        assert!(verify_signature(FIXTURE_KEY, b"tampered", FIXTURE_SIGNATURE).is_err());
        let corrupted = FIXTURE_SIGNATURE.replacen("y/rU", "y/rV", 1);
        assert!(verify_signature(FIXTURE_KEY, b"test", &corrupted).is_err());
        assert!(verify_signature(RELEASE_PUBLIC_KEY, b"test", FIXTURE_SIGNATURE).is_err());
        assert!(verify_signature(FIXTURE_KEY, b"test", "not a signature").is_err());
    }
}
