mod fabric;
mod forge;
mod neoforge;
mod paper;
mod vanilla;

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use ureq::Agent;

use crate::download::parse_checksum_file;
use crate::error::Error;
use crate::http::get_bytes;
use crate::minecraft::is_safe_version_id;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loader {
    Vanilla,
    Fabric,
    Paper,
    Forge,
    NeoForge,
}

impl Loader {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Fabric => "fabric",
            Self::Paper => "paper",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
        }
    }
}

impl fmt::Display for Loader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Loader {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        [
            Self::Vanilla,
            Self::Fabric,
            Self::Paper,
            Self::Forge,
            Self::NeoForge,
        ]
        .into_iter()
        .find(|loader| loader.as_str().eq_ignore_ascii_case(name))
        .ok_or_else(|| Error::UnknownLoader {
            name: name.to_owned(),
        })
    }
}

/// What to fetch for one server; `loader_version` is the loader build picked, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub loader: Loader,
    pub minecraft: String,
    pub loader_version: Option<String>,
    pub artifacts: Vec<Artifact>,
    pub installer: Option<InstallerStep>,
}

/// One file to fetch; `dest` is relative to the staging folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub url: String,
    pub digest: Option<Digest>,
    pub dest: PathBuf,
}

/// Expected checksum as lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Digest {
    Sha1(String),
    Sha256(String),
}

impl Digest {
    pub fn hex(&self) -> &str {
        match self {
            Self::Sha1(hex) | Self::Sha256(hex) => hex,
        }
    }
}

/// An installer jar (already among the artifacts) to run once from the staging folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallerStep {
    pub jar: PathBuf,
    pub args: Vec<String>,
}

/// Picks server software for `loader`; `None` means the newest Minecraft the loader ships stable builds for.
pub fn resolve(agent: &Agent, loader: Loader, minecraft: Option<&str>) -> Result<Plan, Error> {
    if let Some(minecraft) = minecraft
        && !is_safe_version_id(minecraft)
    {
        return Err(Error::UnsupportedVersion {
            loader,
            minecraft: minecraft.to_owned(),
        });
    }
    match loader {
        Loader::Vanilla => vanilla::plan(agent, minecraft),
        Loader::Fabric => fabric::plan(agent, minecraft),
        Loader::Paper => paper::plan(agent, minecraft),
        Loader::Forge => forge::plan(agent, minecraft),
        Loader::NeoForge => neoforge::plan(agent, minecraft),
    }
}

/// Argv for the installer step; run it as the server user with the staging folder as working directory.
pub fn installer_command(step: &InstallerStep, java: &Path) -> Vec<OsString> {
    let mut argv = vec![
        java.as_os_str().to_owned(),
        OsString::from("-jar"),
        step.jar.clone().into_os_string(),
    ];
    argv.extend(step.args.iter().map(OsString::from));
    argv
}

fn install_server_step(jar: &Path) -> InstallerStep {
    InstallerStep {
        jar: jar.to_path_buf(),
        args: vec!["--installServer".to_owned(), ".".to_owned()],
    }
}

/// Maven publishes `<file>.sha256` holding just the hex digest.
fn maven_sha256(agent: &Agent, url: &str) -> Result<String, Error> {
    let checksum_url = format!("{url}.sha256");
    let text = String::from_utf8_lossy(&get_bytes(agent, &checksum_url)?).into_owned();
    parse_checksum_file(&text, 64).ok_or(Error::BadChecksumFile { url: checksum_url })
}

fn unsupported(loader: Loader, minecraft: Option<&str>) -> Error {
    Error::UnsupportedVersion {
        loader,
        minecraft: minecraft.unwrap_or("latest").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_loader_names() {
        let cases = [
            ("vanilla", Some(Loader::Vanilla)),
            ("fabric", Some(Loader::Fabric)),
            ("paper", Some(Loader::Paper)),
            ("forge", Some(Loader::Forge)),
            ("neoforge", Some(Loader::NeoForge)),
            ("NeoForge", Some(Loader::NeoForge)),
            ("quilt", None),
            ("", None),
        ];
        for (name, loader) in cases {
            assert_eq!(name.parse::<Loader>().ok(), loader, "{name}");
        }
    }

    #[test]
    fn installer_runs_install_server_in_working_dir() {
        let step = install_server_step(Path::new("neoforge-21.1.256-installer.jar"));
        let argv = installer_command(&step, Path::new("/opt/java/bin/java"));
        assert_eq!(
            argv,
            [
                "/opt/java/bin/java",
                "-jar",
                "neoforge-21.1.256-installer.jar",
                "--installServer",
                "."
            ]
        );
    }
}
