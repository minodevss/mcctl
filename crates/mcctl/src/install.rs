use std::path::{Path, PathBuf};
use std::process::Command;

use mcctl_platform::{Detected, InstallerStep, Loader, Plan, Platform};

use crate::error::Error;
use crate::files::{self, Owner};
use crate::names::ServerName;
use crate::output;
use crate::paths::Paths;
use crate::process;
use crate::systemd;

const EULA_URL: &str = "https://aka.ms/MinecraftEULA";
const FORGE_SUPPORT: &str =
    "Forge asks automated installers to support it: https://www.patreon.com/LexManos";

/// A root-only folder next to the server folders, so a later rename stays on one filesystem.
pub(crate) fn make_staging(paths: &Paths, name: &ServerName) -> Result<PathBuf, Error> {
    let parent = paths.servers_dir();
    std::fs::create_dir_all(&parent).map_err(Error::io(&parent))?;
    let staging = paths.staging_dir(name);
    files::remove_tree(&staging)?;
    files::create_private_dir(&staging)?;
    Ok(staging)
}

pub(crate) fn download(agent: &ureq::Agent, plan: &Plan, staging: &Path) -> Result<(), Error> {
    output::info(format!(
        "downloading {}",
        plan_summary(plan, mcctl_platform::java_major(&plan.minecraft, None))
    ));
    mcctl_platform::download(agent, plan, staging)
        .map_err(Error::platform("cannot download the server software"))
}

pub(crate) fn ensure_java(agent: &ureq::Agent, paths: &Paths, major: u8) -> Result<PathBuf, Error> {
    let root = paths.java_root();
    std::fs::create_dir_all(&root).map_err(Error::io(&root))?;
    mcctl_platform::ensure_java(agent, &root, major)
        .map_err(Error::platform(format!("cannot install java {major}")))
}

/// Keeps an installed Java when the refresh fails, so a server can start offline.
pub(crate) fn refresh_java(agent: &ureq::Agent, paths: &Paths, major: u8) -> Result<(), Error> {
    let installed = mcctl_platform::java_path(&paths.java_root(), major).exists();
    match ensure_java(agent, paths, major) {
        Ok(_) => Ok(()),
        Err(err) if installed => {
            output::warn(format!("{err}; keeping the installed java {major}"));
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// Runs the installer jar in `dir` as `user`, then removes the jar and the logs it wrote, as that user.
pub(crate) fn run_installer(
    dir: &Path,
    user: &str,
    owner: Owner,
    step: &InstallerStep,
    java: &Path,
) -> Result<(), Error> {
    output::info("running the installer; this can take a few minutes");
    let logs_before = files::root_log_files(dir)?;
    let argv = mcctl_platform::installer_command(step, java);
    systemd::run_sandboxed(user, dir, &argv)?;
    let new_logs = files::root_log_files(dir)?
        .into_iter()
        .filter(|log| !logs_before.contains(log));
    let leftovers: Vec<PathBuf> = std::iter::once(step.jar.clone())
        .chain(new_logs)
        .map(|rel| dir.join(rel))
        .collect();
    remove_as(owner, &leftovers)
}

pub(crate) fn remove_as(owner: Owner, paths: &[PathBuf]) -> Result<(), Error> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut command = Command::new("rm");
    command.args(["-f", "--"]).args(paths);
    process::run(process::as_owner(&mut command, owner)).map(drop)
}

pub(crate) fn print_eula_notice() {
    output::info(format!("Minecraft EULA: {EULA_URL}"));
}

pub(crate) fn print_loader_notice(loader: Loader) {
    match loader {
        Loader::Forge => output::info(FORGE_SUPPORT),
        Loader::Vanilla | Loader::Fabric | Loader::Paper | Loader::NeoForge => {}
    }
}

/// "fabric 26.3 (loader 0.19.5), java 25"
pub(crate) fn plan_summary(plan: &Plan, java: u8) -> String {
    let build = plan
        .loader_version
        .as_ref()
        .map(|version| format!(" ({} {version})", build_label(plan.loader)))
        .unwrap_or_default();
    format!("{} {}{build}, java {java}", plan.loader, plan.minecraft)
}

fn build_label(loader: Loader) -> &'static str {
    match loader {
        Loader::Paper => "build",
        Loader::Vanilla | Loader::Fabric | Loader::Forge | Loader::NeoForge => "loader",
    }
}

/// "fabric 26.3"
pub(crate) fn detected_version(detected: &Detected) -> String {
    format!("{} {}", detected.platform, detected.minecraft)
}

pub(crate) fn loader_for(platform: Platform) -> Option<Loader> {
    match platform {
        Platform::Vanilla => Some(Loader::Vanilla),
        Platform::Fabric => Some(Loader::Fabric),
        Platform::Paper => Some(Loader::Paper),
        Platform::Forge => Some(Loader::Forge),
        Platform::NeoForge => Some(Loader::NeoForge),
        Platform::ForgeLegacy => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(loader: Loader, minecraft: &str, build: Option<&str>) -> Plan {
        Plan {
            loader,
            minecraft: minecraft.to_owned(),
            loader_version: build.map(str::to_owned),
            artifacts: Vec::new(),
            installer: None,
        }
    }

    #[test]
    fn summarizes_plans_in_one_line() {
        let cases = [
            (
                plan(Loader::Fabric, "26.3", Some("0.19.5")),
                25,
                "fabric 26.3 (loader 0.19.5), java 25",
            ),
            (
                plan(Loader::Vanilla, "1.21.1", None),
                21,
                "vanilla 1.21.1, java 21",
            ),
            (
                plan(Loader::Paper, "1.21.1", Some("133")),
                21,
                "paper 1.21.1 (build 133), java 21",
            ),
        ];
        for (plan, java, expected) in cases {
            assert_eq!(plan_summary(&plan, java), expected);
        }
    }
}
