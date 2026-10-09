use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;
use ureq::Agent;

use super::{Artifact, Digest, Loader, Plan, install_server_step, maven_sha256, unsupported};
use crate::error::Error;
use crate::http::{get_json, is_not_found};
use crate::minecraft::{is_plain_release, is_safe_version_id, parse_release};

const PROMOTIONS_URL: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
const MAVEN: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";

#[derive(Debug, Deserialize)]
pub(super) struct Promotions {
    promos: HashMap<String, String>,
}

/// Forge build for `minecraft`: recommended, else latest. `None` takes the newest Minecraft
/// with a recommended build, falling back to the newest with any build.
pub(super) fn pick(promotions: &Promotions, minecraft: Option<&str>) -> Option<(String, String)> {
    let (minecraft, forge) = match minecraft {
        Some(minecraft) => {
            let forge = promotions
                .promos
                .get(&format!("{minecraft}-recommended"))
                .or_else(|| promotions.promos.get(&format!("{minecraft}-latest")))?;
            (minecraft.to_owned(), forge.clone())
        }
        None => newest_promotion(promotions, "-recommended")
            .or_else(|| newest_promotion(promotions, "-latest"))?,
    };
    is_safe_version_id(&forge).then_some((minecraft, forge))
}

fn newest_promotion(promotions: &Promotions, suffix: &str) -> Option<(String, String)> {
    promotions
        .promos
        .iter()
        .filter_map(|(key, forge)| {
            let minecraft = key.strip_suffix(suffix)?;
            is_plain_release(minecraft).then(|| (parse_release(minecraft), minecraft, forge))
        })
        .max_by_key(|(release, ..)| *release)
        .map(|(_, minecraft, forge)| (minecraft.to_owned(), forge.clone()))
}

/// Maven coordinates to try: `<mc>-<forge>`, then the `<mc>-<forge>-<mc>` form used around 1.7.10.
pub(super) fn coordinates(minecraft: &str, forge: &str) -> [String; 2] {
    [
        format!("{minecraft}-{forge}"),
        format!("{minecraft}-{forge}-{minecraft}"),
    ]
}

pub(super) fn installer_url(coordinate: &str) -> String {
    format!("{MAVEN}/{coordinate}/forge-{coordinate}-installer.jar")
}

pub(super) fn plan(agent: &Agent, minecraft: Option<&str>) -> Result<Plan, Error> {
    let promotions: Promotions = get_json(agent, PROMOTIONS_URL)?;
    let (minecraft, forge) =
        pick(&promotions, minecraft).ok_or_else(|| unsupported(Loader::Forge, minecraft))?;
    let [standard, legacy] = coordinates(&minecraft, &forge);
    let (coordinate, sha256) = match maven_sha256(agent, &installer_url(&standard)) {
        Ok(sha256) => (standard, sha256),
        Err(error) if is_not_found(&error) => {
            let sha256 = maven_sha256(agent, &installer_url(&legacy))?;
            (legacy, sha256)
        }
        Err(error) => return Err(error),
    };
    let jar = PathBuf::from(format!("forge-{coordinate}-installer.jar"));
    Ok(Plan {
        loader: Loader::Forge,
        minecraft,
        loader_version: Some(forge),
        installer: Some(install_server_step(&jar)),
        artifacts: vec![Artifact {
            url: installer_url(&coordinate),
            digest: Some(Digest::Sha256(sha256)),
            dest: jar,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROMOTIONS: &str = include_str!("../../tests/fixtures/forge_promotions_slim.json");

    #[test]
    fn forge_prefers_recommended_then_latest() {
        let promotions: Promotions = serde_json::from_str(PROMOTIONS).unwrap();
        let cases = [
            (Some("1.20.1"), Some(("1.20.1", "47.4.10"))),
            (Some("26.3"), Some(("26.3", "66.0.9"))),
            (Some("1.12.2"), Some(("1.12.2", "14.23.5.2859"))),
            (Some("1.99"), None),
            (None, Some(("26.2", "65.1.0"))),
        ];
        for (minecraft, expected) in cases {
            let picked = pick(&promotions, minecraft);
            assert_eq!(
                picked.as_ref().map(|(m, f)| (m.as_str(), f.as_str())),
                expected,
                "{minecraft:?}"
            );
        }
    }

    #[test]
    fn forge_installer_urls_cover_old_naming() {
        let [standard, legacy] = coordinates("1.7.10", "10.13.4.1614");
        assert_eq!(
            installer_url(&standard),
            "https://maven.minecraftforge.net/net/minecraftforge/forge/1.7.10-10.13.4.1614/forge-1.7.10-10.13.4.1614-installer.jar"
        );
        assert_eq!(legacy, "1.7.10-10.13.4.1614-1.7.10");
    }
}
