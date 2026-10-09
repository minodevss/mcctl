use std::io;
use std::path::PathBuf;

use crate::install::Loader;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("request to {url} failed: {source}")]
    Http {
        url: String,
        #[source]
        source: Box<ureq::Error>,
    },
    #[error("unexpected response from {url}: {source}")]
    Json {
        url: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("no minecraft server found in {dir}")]
    NotAServer { dir: PathBuf },
    #[error("more than one server in {}: {}", dir.display(), candidates.join(", "))]
    Ambiguous {
        dir: PathBuf,
        candidates: Vec<String>,
    },
    #[error("cannot tell the minecraft version of {path}")]
    UnknownVersion { path: PathBuf },
    #[error("unknown loader {name}, expected vanilla, fabric, paper, forge or neoforge")]
    UnknownLoader { name: String },
    #[error("{loader} has no build for minecraft {minecraft}")]
    UnsupportedVersion { loader: Loader, minecraft: String },
    #[error("paper has no stable build for minecraft {minecraft}")]
    NoStableBuild { minecraft: String },
    #[error("checksum mismatch for {url}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        url: String,
        expected: String,
        actual: String,
    },
    #[error("no checksum found at {url}")]
    BadChecksumFile { url: String },
    #[error("refusing to download over plain http: {url}")]
    InsecureUrl { url: String },
    #[error("refusing to write outside the staging folder: {path}")]
    UnsafePath { path: PathBuf },
    #[error("{path} already exists")]
    AlreadyExists { path: PathBuf },
    #[error("refusing unsafe entry {entry} in {archive}")]
    UnsafeArchive { archive: PathBuf, entry: String },
    #[error("{archive} does not hold a single java runtime folder")]
    ArchiveLayout { archive: PathBuf },
    #[error("temurin has no builds for {os} on {arch}")]
    UnsupportedPlatform { os: String, arch: String },
    #[error("temurin has no java {major} runtime for {os} on {arch}")]
    NoJavaBuild {
        major: u8,
        os: &'static str,
        arch: &'static str,
    },
}

pub(crate) fn io_error(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Error {
    let path = path.into();
    move |source| Error::Io { path, source }
}
