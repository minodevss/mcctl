use std::path::PathBuf;

use serde::Deserialize;
use ureq::Agent;

use super::{Artifact, Digest, Loader, Plan, install_server_step, maven_sha256, unsupported};
use crate::error::Error;
use crate::http::get_json;
use crate::minecraft::{is_safe_version_id, leading_numbers, neoforge_minecraft};

const VERSIONS_URL: &str =
    "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";
const MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";

#[derive(Debug, Deserialize)]
pub(super) struct Versions {
    versions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Channel {
    Beta,
    Stable,
}

fn channel(version: &str) -> Option<Channel> {
    if version.contains('+') {
        return None;
    }
    match version.split_once('-') {
        None => Some(Channel::Stable),
        Some((_, "beta")) => Some(Channel::Beta),
        Some(_) => None,
    }
}

/// Newest `NeoForge` for `minecraft` (or overall), preferring stable over beta; alphas are skipped.
pub(super) fn pick(versions: &Versions, minecraft: Option<&str>) -> Option<(String, String)> {
    versions
        .versions
        .iter()
        .filter(|version| is_safe_version_id(version))
        .filter_map(|version| {
            let target = neoforge_minecraft(version)?;
            let rank = (channel(version)?, leading_numbers(version)?);
            minecraft
                .is_none_or(|wanted| wanted == target)
                .then_some((rank, version, target))
        })
        .max_by(|(left, ..), (right, ..)| left.cmp(right))
        .map(|(_, version, target)| (version.clone(), target))
}

pub(super) fn installer_url(version: &str) -> String {
    format!("{MAVEN}/{version}/neoforge-{version}-installer.jar")
}

pub(super) fn plan(agent: &Agent, minecraft: Option<&str>) -> Result<Plan, Error> {
    let versions: Versions = get_json(agent, VERSIONS_URL)?;
    let (version, minecraft) =
        pick(&versions, minecraft).ok_or_else(|| unsupported(Loader::NeoForge, minecraft))?;
    let url = installer_url(&version);
    let sha256 = maven_sha256(agent, &url)?;
    let jar = PathBuf::from(format!("neoforge-{version}-installer.jar"));
    Ok(Plan {
        loader: Loader::NeoForge,
        minecraft,
        loader_version: Some(version),
        installer: Some(install_server_step(&jar)),
        artifacts: vec![Artifact {
            url,
            digest: Some(Digest::Sha256(sha256)),
            dest: jar,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSIONS: &str = include_str!("../../tests/fixtures/neoforge_versions.json");

    fn versions() -> Versions {
        serde_json::from_str(VERSIONS).unwrap()
    }

    #[test]
    fn parses_neoforge_versions_for_mc() {
        let cases = [
            (Some("1.21.1"), Some(("21.1.256", "1.21.1"))),
            (Some("1.21"), Some(("21.0.167", "1.21"))),
            (Some("1.20.4"), Some(("20.4.251", "1.20.4"))),
            (Some("26.1"), Some(("26.1.0.19-beta", "26.1"))),
            (Some("26.3"), Some(("26.3.0.58-beta", "26.3"))),
            (Some("1.21.6"), Some(("21.6.20-beta", "1.21.6"))),
            (Some("1.12.2"), None),
            (None, Some(("26.2.0.88", "26.2"))),
        ];
        for (minecraft, expected) in cases {
            let picked = pick(&versions(), minecraft);
            assert_eq!(
                picked.as_ref().map(|(v, m)| (v.as_str(), m.as_str())),
                expected,
                "{minecraft:?}"
            );
        }
    }

    #[test]
    fn neoforge_installer_url_follows_maven_layout() {
        assert_eq!(
            installer_url("21.1.256"),
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.256/neoforge-21.1.256-installer.jar"
        );
    }
}
