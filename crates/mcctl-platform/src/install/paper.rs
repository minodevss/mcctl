use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;
use ureq::Agent;

use super::{Artifact, Digest, Loader, Plan, unsupported};
use crate::error::Error;
use crate::http::get_json;
use crate::minecraft::is_plain_release;

const PROJECT: &str = "https://fill.papermc.io/v3/projects/paper";
const SERVER_DOWNLOAD: &str = "server:default";
const STABLE: &str = "STABLE";
const LATEST_LOOKBACK: usize = 5;

#[derive(Debug, Deserialize)]
pub(super) struct Versions {
    versions: Vec<VersionEntry>,
}

#[derive(Debug, Deserialize)]
struct VersionEntry {
    version: VersionInfo,
}

#[derive(Debug, Deserialize)]
struct VersionInfo {
    id: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct Build {
    id: u32,
    channel: String,
    downloads: HashMap<String, BuildDownload>,
}

#[derive(Debug, Deserialize)]
struct BuildDownload {
    url: String,
    checksums: Checksums,
}

#[derive(Debug, Deserialize)]
struct Checksums {
    sha256: String,
}

impl Versions {
    fn contains(&self, minecraft: &str) -> bool {
        self.versions
            .iter()
            .any(|entry| entry.version.id == minecraft)
    }

    /// Release ids in the API's newest-first order, skipping pre-releases and release candidates.
    fn releases(&self) -> impl Iterator<Item = &str> {
        self.versions
            .iter()
            .map(|entry| entry.version.id.as_str())
            .filter(|id| is_plain_release(id))
    }
}

pub(super) fn newest_stable(builds: &[Build]) -> Option<(u32, Artifact)> {
    builds
        .iter()
        .filter(|build| build.channel == STABLE)
        .filter_map(|build| Some((build.id, build.downloads.get(SERVER_DOWNLOAD)?)))
        .max_by_key(|(id, _)| *id)
        .map(|(id, download)| {
            let artifact = Artifact {
                url: download.url.clone(),
                digest: Some(Digest::Sha256(
                    download.checksums.sha256.to_ascii_lowercase(),
                )),
                dest: PathBuf::from("paper.jar"),
            };
            (id, artifact)
        })
}

pub(super) fn plan(agent: &Agent, minecraft: Option<&str>) -> Result<Plan, Error> {
    let versions: Versions = get_json(agent, &format!("{PROJECT}/versions"))?;
    if let Some(minecraft) = minecraft {
        if !versions.contains(minecraft) {
            return Err(unsupported(Loader::Paper, Some(minecraft)));
        }
        return stable_plan(agent, minecraft)?.ok_or_else(|| Error::NoStableBuild {
            minecraft: minecraft.to_owned(),
        });
    }
    for release in versions.releases().take(LATEST_LOOKBACK) {
        if let Some(plan) = stable_plan(agent, release)? {
            return Ok(plan);
        }
    }
    Err(Error::NoStableBuild {
        minecraft: versions.releases().next().unwrap_or("latest").to_owned(),
    })
}

fn stable_plan(agent: &Agent, minecraft: &str) -> Result<Option<Plan>, Error> {
    let builds: Vec<Build> = get_json(agent, &format!("{PROJECT}/versions/{minecraft}/builds"))?;
    Ok(newest_stable(&builds).map(|(build, artifact)| Plan {
        loader: Loader::Paper,
        minecraft: minecraft.to_owned(),
        loader_version: Some(build.to_string()),
        artifacts: vec![artifact],
        installer: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSIONS: &str = include_str!("../../tests/fixtures/paper_versions.json");
    const BUILDS: &str = include_str!("../../tests/fixtures/paper_builds_26.2.json");
    const BETA_ONLY: &str = include_str!("../../tests/fixtures/paper_builds_26.3.json");

    #[test]
    fn picks_newest_stable_paper_build() {
        let builds: Vec<Build> = serde_json::from_str(BUILDS).unwrap();
        let (build, artifact) = newest_stable(&builds).unwrap();
        assert_eq!(build, 132);
        assert_eq!(
            artifact,
            Artifact {
                url: "https://fill-data.papermc.io/v1/objects/5ab560a769c1ab413cb7f637dd0dc697974571f2db0a667cfbac511422e51b26/paper-26.2-132.jar".to_owned(),
                digest: Some(Digest::Sha256(
                    "5ab560a769c1ab413cb7f637dd0dc697974571f2db0a667cfbac511422e51b26".to_owned()
                )),
                dest: PathBuf::from("paper.jar"),
            }
        );
    }

    #[test]
    fn beta_only_paper_version_has_no_stable_build() {
        let builds: Vec<Build> = serde_json::from_str(BETA_ONLY).unwrap();
        assert!(newest_stable(&builds).is_none());
    }

    #[test]
    fn latest_paper_candidates_skip_prereleases() {
        let versions: Versions = serde_json::from_str(VERSIONS).unwrap();
        assert_eq!(
            versions.releases().take(4).collect::<Vec<_>>(),
            ["26.3", "26.2", "26.1.2", "26.1.1"]
        );
        assert!(versions.contains("1.21.11"));
        assert!(!versions.contains("1.99"));
    }
}
