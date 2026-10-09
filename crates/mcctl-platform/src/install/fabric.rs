use std::path::PathBuf;

use serde::Deserialize;
use ureq::Agent;

use super::vanilla::{server_artifact, server_jar};
use super::{Artifact, Loader, Plan, unsupported};
use crate::error::Error;
use crate::http::get_json;
use crate::minecraft::is_safe_version_id;

const META: &str = "https://meta.fabricmc.net/v2/versions";
const LAUNCHER_JAR: &str = "fabric-server-launch.jar";

#[derive(Debug, Deserialize)]
pub(super) struct LoaderEntry {
    loader: Component,
}

#[derive(Debug, Deserialize)]
pub(super) struct Component {
    version: String,
    stable: bool,
}

pub(super) fn newest_stable_loader(entries: &[LoaderEntry]) -> Option<&str> {
    first_stable(entries.iter().map(|entry| &entry.loader))
}

/// Fabric meta lists newest first.
pub(super) fn first_stable<'a>(
    components: impl IntoIterator<Item = &'a Component>,
) -> Option<&'a str> {
    components
        .into_iter()
        .find(|component| component.stable && is_safe_version_id(&component.version))
        .map(|component| component.version.as_str())
}

/// Where the launcher looks for the vanilla jar before downloading it itself.
pub(super) fn game_jar_dest(minecraft: &str) -> PathBuf {
    PathBuf::from(format!(".fabric/server/{minecraft}-server.jar"))
}

pub(super) fn launcher_url(minecraft: &str, loader: &str, installer: &str) -> String {
    format!("{META}/loader/{minecraft}/{loader}/{installer}/server/jar")
}

pub(super) fn plan(agent: &Agent, minecraft: Option<&str>) -> Result<Plan, Error> {
    let (minecraft, server) = server_jar(agent, minecraft, Loader::Fabric)?;
    let loaders: Vec<LoaderEntry> = get_json(agent, &format!("{META}/loader/{minecraft}"))?;
    let loader = newest_stable_loader(&loaders)
        .ok_or_else(|| unsupported(Loader::Fabric, Some(&minecraft)))?;
    let installers: Vec<Component> = get_json(agent, &format!("{META}/installer"))?;
    let installer =
        first_stable(&installers).ok_or_else(|| unsupported(Loader::Fabric, Some(&minecraft)))?;
    let launcher = Artifact {
        url: launcher_url(&minecraft, loader, installer),
        digest: None,
        dest: PathBuf::from(LAUNCHER_JAR),
    };
    Ok(Plan {
        loader: Loader::Fabric,
        loader_version: Some(loader.to_owned()),
        artifacts: vec![launcher, server_artifact(server, game_jar_dest(&minecraft))],
        minecraft,
        installer: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOADERS: &str = include_str!("../../tests/fixtures/fabric_loader_1.21.1.json");
    const INSTALLERS: &str = include_str!("../../tests/fixtures/fabric_installer.json");

    #[test]
    fn picks_newest_stable_fabric_loader_and_installer() {
        let loaders: Vec<LoaderEntry> = serde_json::from_str(LOADERS).unwrap();
        assert_eq!(newest_stable_loader(&loaders), Some("0.19.5"));
        let installers: Vec<Component> = serde_json::from_str(INSTALLERS).unwrap();
        assert_eq!(first_stable(&installers), Some("1.1.2"));
    }

    #[test]
    fn skips_unstable_loaders() {
        let loaders: Vec<LoaderEntry> = serde_json::from_str(
            r#"[{"loader":{"version":"0.20.0-beta.1","stable":false}},{"loader":{"version":"0.19.5","stable":true}}]"#,
        )
        .unwrap();
        assert_eq!(newest_stable_loader(&loaders), Some("0.19.5"));
    }

    #[test]
    fn launcher_and_game_jar_paths_match_fabric_layout() {
        assert_eq!(
            launcher_url("1.21.1", "0.19.5", "1.1.2"),
            "https://meta.fabricmc.net/v2/versions/loader/1.21.1/0.19.5/1.1.2/server/jar"
        );
        assert_eq!(
            game_jar_dest("1.21.1"),
            PathBuf::from(".fabric/server/1.21.1-server.jar")
        );
    }
}
