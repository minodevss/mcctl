use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use sha1::Digest as _;
use ureq::Agent;

use crate::error::{Error, io_error};
use crate::http::{get_stream, http_error, is_transient};
use crate::install::{Artifact, Digest, Plan};
use crate::nofollow::{create_real_dirs, exists_nofollow, is_plain_relative};

const ATTEMPTS: u64 = 3;

/// Saves every artifact under `staging`, which must be a real directory the caller just created.
/// Refuses destinations that exist or leave `staging`, and verifies each published checksum.
pub fn download(agent: &Agent, plan: &Plan, staging: &Path) -> Result<(), Error> {
    let staging_type = fs::symlink_metadata(staging)
        .map_err(io_error(staging))?
        .file_type();
    if !staging_type.is_dir() {
        return Err(Error::UnsafePath {
            path: staging.to_path_buf(),
        });
    }
    for artifact in &plan.artifacts {
        check_artifact(artifact)?;
    }
    for artifact in &plan.artifacts {
        save_artifact(agent, artifact, staging)?;
    }
    Ok(())
}

fn check_artifact(artifact: &Artifact) -> Result<(), Error> {
    if !is_plain_relative(&artifact.dest) {
        return Err(Error::UnsafePath {
            path: artifact.dest.clone(),
        });
    }
    require_https(&artifact.url)
}

pub(crate) fn require_https(url: &str) -> Result<(), Error> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(Error::InsecureUrl {
            url: url.to_owned(),
        })
    }
}

fn save_artifact(agent: &Agent, artifact: &Artifact, staging: &Path) -> Result<(), Error> {
    if let Some(parent) = artifact.dest.parent() {
        create_real_dirs(staging, parent)?;
    }
    let target = staging.join(&artifact.dest);
    if exists_nofollow(&target)? {
        return Err(Error::AlreadyExists { path: target });
    }
    let partial = partial_path(&target);
    save_verified(agent, &artifact.url, artifact.digest.as_ref(), &partial)?;
    fs::rename(&partial, &target).map_err(io_error(&target))
}

fn partial_path(target: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(target.file_name().unwrap_or_default());
    name.push(".part");
    target.with_file_name(name)
}

/// Streams `url` into a new file at `path`, retrying dropped transfers, and removes the file
/// again if the download or checksum fails.
pub(crate) fn save_verified(
    agent: &Agent,
    url: &str,
    digest: Option<&Digest>,
    path: &Path,
) -> Result<(), Error> {
    let mut attempt = 1;
    loop {
        match save_once(agent, url, digest, path) {
            Err(error) if attempt < ATTEMPTS && is_transient(&error) => {
                thread::sleep(Duration::from_secs(attempt));
                attempt += 1;
            }
            result => return result,
        }
    }
}

fn save_once(agent: &Agent, url: &str, digest: Option<&Digest>, path: &Path) -> Result<(), Error> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error(path))?;
    let result = copy_verified(agent, url, digest, &mut file, path)
        .and_then(|()| file.sync_all().map_err(io_error(path)));
    drop(file);
    if result.is_err() {
        fs::remove_file(path).ok();
    }
    result
}

fn copy_verified(
    agent: &Agent,
    url: &str,
    digest: Option<&Digest>,
    out: &mut impl Write,
    out_path: &Path,
) -> Result<(), Error> {
    let mut body = get_stream(agent, url)?;
    let mut hasher = digest.map(Hasher::for_digest);
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = match body.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(http_error(url, ureq::Error::Io(error))),
        };
        let chunk = buffer.get(..read).unwrap_or_default();
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(chunk);
        }
        out.write_all(chunk).map_err(io_error(out_path))?;
    }
    match (digest, hasher) {
        (Some(expected), Some(hasher)) => check_digest(url, expected, &hasher.finish_hex()),
        _ => Ok(()),
    }
}

pub(crate) fn check_digest(url: &str, expected: &Digest, actual: &str) -> Result<(), Error> {
    let expected = expected.hex();
    if expected.eq_ignore_ascii_case(actual) {
        Ok(())
    } else {
        Err(Error::ChecksumMismatch {
            url: url.to_owned(),
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        })
    }
}

enum Hasher {
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
}

impl Hasher {
    fn for_digest(digest: &Digest) -> Self {
        match digest {
            Digest::Sha1(_) => Self::Sha1(sha1::Sha1::new()),
            Digest::Sha256(_) => Self::Sha256(sha2::Sha256::new()),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            Self::Sha1(hasher) => hasher.update(data),
            Self::Sha256(hasher) => hasher.update(data),
        }
    }

    fn finish_hex(self) -> String {
        match self {
            Self::Sha1(hasher) => hex(&hasher.finalize()),
            Self::Sha256(hasher) => hex(&hasher.finalize()),
        }
    }
}

pub(crate) fn sha1_hex(bytes: &[u8]) -> String {
    hex(&sha1::Sha1::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            write!(out, "{byte:02x}").ok();
            out
        })
}

/// Reads a published checksum file: the first word, if it is hex of the expected length.
pub(crate) fn parse_checksum_file(text: &str, hex_len: usize) -> Option<String> {
    let word = text.split_whitespace().next()?;
    (word.len() == hex_len && word.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| word.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_to_lowercase_hex() {
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn reads_checksum_files() {
        let sha256 = "1912760b4cb6b803d8a826de603c9076b1da71ec2765e9a1f8c1ca78f65278e3";
        let cases = [
            (sha256.to_owned(), Some(sha256)),
            (
                format!("{}  forge-installer.jar\n", sha256.to_uppercase()),
                Some(sha256),
            ),
            ("not-a-checksum".to_owned(), None),
            (String::new(), None),
            ("9d85f6e652996e83f05ead32120317e1ef056590".to_owned(), None),
        ];
        for (text, expected) in cases {
            assert_eq!(
                parse_checksum_file(&text, 64).as_deref(),
                expected,
                "{text}"
            );
        }
    }

    #[test]
    fn digest_mismatch_names_both_values() {
        let expected = Digest::Sha1("aa".to_owned());
        assert!(check_digest("https://x", &expected, "AA").is_ok());
        assert!(matches!(
            check_digest("https://x", &expected, "bb"),
            Err(Error::ChecksumMismatch { .. })
        ));
    }
}
