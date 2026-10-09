use std::path::PathBuf;

use serde::Deserialize;
use ureq::Agent;

use super::{Artifact, Digest, Loader, Plan, unsupported};
use crate::download::{check_digest, sha1_hex};
use crate::error::Error;
use crate::http::{get_bytes, get_json, parse_json};

const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, Deserialize)]
pub(super) struct Manifest {
    latest: Latest,
    versions: Vec<ManifestVersion>,
}

#[derive(Debug, Deserialize)]
struct Latest {
    release: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(super) struct ManifestVersion {
    pub(super) id: String,
    pub(super) url: String,
    pub(super) sha1: String,
}

#[derive(Debug, Deserialize)]
struct VersionDetails {
    downloads: Downloads,
}

#[derive(Debug, Deserialize)]
struct Downloads {
    server: Option<FileRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(super) struct FileRef {
    pub(super) url: String,
    pub(super) sha1: String,
}

impl Manifest {
    /// The entry for `minecraft`, or for the latest release when `None`.
    pub(super) fn find(&self, minecraft: Option<&str>) -> Option<&ManifestVersion> {
        let wanted = minecraft.unwrap_or(&self.latest.release);
        self.versions.iter().find(|version| version.id == wanted)
    }
}

pub(super) fn plan(agent: &Agent, minecraft: Option<&str>) -> Result<Plan, Error> {
    let (minecraft, server) = server_jar(agent, minecraft, Loader::Vanilla)?;
    Ok(Plan {
        loader: Loader::Vanilla,
        artifacts: vec![server_artifact(server, PathBuf::from("server.jar"))],
        minecraft,
        loader_version: None,
        installer: None,
    })
}

pub(super) fn server_artifact(server: FileRef, dest: PathBuf) -> Artifact {
    Artifact {
        url: server.url,
        digest: Some(Digest::Sha1(server.sha1.to_ascii_lowercase())),
        dest,
    }
}

/// Resolves the vanilla server jar, checking the version file against the manifest's sha1.
pub(super) fn server_jar(
    agent: &Agent,
    minecraft: Option<&str>,
    loader: Loader,
) -> Result<(String, FileRef), Error> {
    let manifest: Manifest = get_json(agent, MANIFEST_URL)?;
    let version = manifest
        .find(minecraft)
        .ok_or_else(|| unsupported(loader, minecraft))?;
    let bytes = get_bytes(agent, &version.url)?;
    check_digest(
        &version.url,
        &Digest::Sha1(version.sha1.clone()),
        &sha1_hex(&bytes),
    )?;
    let details: VersionDetails = parse_json(&version.url, &bytes)?;
    let server = details
        .downloads
        .server
        .ok_or_else(|| unsupported(loader, Some(&version.id)))?;
    Ok((version.id.clone(), server))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = include_str!("../../tests/fixtures/version_manifest_v2.json");
    const VERSION: &str = include_str!("../../tests/fixtures/version_26.3.json");

    #[test]
    fn parses_version_manifest() {
        let manifest: Manifest = serde_json::from_str(MANIFEST).unwrap();
        let latest = manifest.find(None).unwrap();
        assert_eq!(latest.id, "26.3");
        assert_eq!(latest.sha1, "702fe59163c6ee6578607daa85811d9bc9c7cc40");
        assert_eq!(
            manifest.find(Some("1.21.1")).unwrap().url,
            "https://piston-meta.mojang.com/v1/packages/cedfc3b6dcbca34e2b478d498bf1d56a8fa2f404/1.21.1.json"
        );
        assert!(manifest.find(Some("1.99")).is_none());
    }

    #[test]
    fn reads_server_download_from_version_file() {
        let details: VersionDetails = serde_json::from_str(VERSION).unwrap();
        let server = details.downloads.server.unwrap();
        let artifact = server_artifact(server, PathBuf::from("server.jar"));
        assert_eq!(
            artifact.digest,
            Some(Digest::Sha1(
                "33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c".to_owned()
            ))
        );
        assert_eq!(
            artifact.url,
            "https://piston-data.mojang.com/v1/objects/33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c/server.jar"
        );
    }
}
