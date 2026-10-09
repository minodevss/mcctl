use std::cmp::Ordering;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use mcctl_platform::{Detected, Entry, Plan, Platform};

use crate::commands::Ctx;
use crate::commands::lifecycle::{confirm_dropping_players, start_one};
use crate::config;
use crate::error::Error;
use crate::files::{self, Owner};
use crate::http;
use crate::install;
use crate::lock;
use crate::names::ServerName;
use crate::output;
use crate::paths::Paths;
use crate::process;
use crate::status_file;
use crate::systemd;
use crate::users;
use crate::version;

const MOD_FOLDERS: [&str; 2] = ["mods", "plugins"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    UpToDate,
    Install(Change),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    Newer,
    SameMinecraft,
    Older,
}

pub(crate) fn update(
    ctx: &Ctx<'_>,
    name: &ServerName,
    wanted: Option<&str>,
    force: bool,
) -> Result<(), Error> {
    let _lock = lock::acquire(ctx.paths)?;
    config::load_server(ctx.paths, name)?;
    let dir = ctx.paths.server_dir(name);
    let current = mcctl_platform::detect(&dir).map_err(Error::platform(format!(
        "cannot read the software of {name}"
    )))?;
    let Some(loader) = install::loader_for(current.platform) else {
        return Err(Error::ForgeLegacy {
            name: name.clone(),
            minecraft: current.minecraft,
        });
    };
    if !force {
        refuse_mods(&dir, name)?;
    }
    let agent = http::agent();
    let plan = mcctl_platform::resolve(&agent, loader, wanted).map_err(Error::platform(
        format!("cannot find {loader} {}", wanted.unwrap_or("latest")),
    ))?;
    let installed = installed_build(&current);
    let change = match decide(name, &current, installed.as_deref(), &plan, force)? {
        Decision::UpToDate => {
            output::info(format!(
                "{name} is already up to date ({})",
                install::detected_version(&current)
            ));
            return Ok(());
        }
        Decision::Install(change) => change,
    };
    let target_java = mcctl_platform::java_major(&plan.minecraft, None);
    output::info(format!(
        "{name}: {} -> {}",
        install::detected_version(&current),
        install::plan_summary(&plan, target_java)
    ));
    match change {
        Change::Newer => output::warn(format!(
            "world conversion to a newer version cannot be undone; back up {} first",
            dir.display()
        )),
        Change::Older => output::warn("an older version may fail to load or damage this world"),
        Change::SameMinecraft => {}
    }
    if !output::confirm(format!("Update {name}?"), ctx.yes)? {
        return Err(Error::Cancelled);
    }
    install::print_loader_notice(loader);
    let owner =
        users::lookup(&name.user())?.ok_or_else(|| Error::UnknownUser { user: name.user() })?;
    let unit = name.unit();
    let was_running = systemd::is_active(&unit);
    if was_running {
        let published = status_file::read(ctx.paths);
        if !confirm_dropping_players(published.as_ref(), name, "Stop", ctx.yes)? {
            return Err(Error::Cancelled);
        }
        systemd::stop(&unit)?;
    }
    let mut touched = false;
    let result = install_update(
        ctx.paths,
        &agent,
        name,
        owner,
        &current,
        &plan,
        &mut touched,
    );
    match result {
        Ok(detected) => {
            output::info(format!(
                "updated {name} to {}",
                install::detected_version(&detected)
            ));
            if was_running {
                start_one(ctx.paths, &agent, name)?;
                output::info(format!("started {name}"));
            }
            Ok(())
        }
        Err(err) => {
            if was_running && !touched {
                start_one(ctx.paths, &agent, name).unwrap_or_else(output::warn);
            }
            Err(err)
        }
    }
}

fn refuse_mods(dir: &Path, name: &ServerName) -> Result<(), Error> {
    for folder in MOD_FOLDERS {
        if files::has_jar_files(&dir.join(folder))? {
            return Err(Error::ModsPresent {
                name: name.clone(),
                folder,
            });
        }
    }
    Ok(())
}

/// The loader build an installer-based server runs, read from its libraries folder name.
pub(crate) fn installed_build(detected: &Detected) -> Option<String> {
    let Entry::ArgFiles { unix, .. } = &detected.entry else {
        return None;
    };
    let folder = unix.parent()?.file_name()?.to_str()?;
    match detected.platform {
        Platform::Forge => folder
            .strip_prefix(&format!("{}-", detected.minecraft))
            .map(str::to_owned),
        Platform::NeoForge => Some(folder.to_owned()),
        Platform::Vanilla | Platform::Fabric | Platform::Paper | Platform::ForgeLegacy => None,
    }
}

pub(crate) fn decide(
    name: &ServerName,
    current: &Detected,
    installed_build: Option<&str>,
    plan: &Plan,
    force: bool,
) -> Result<Decision, Error> {
    let order = if plan.minecraft == current.minecraft {
        Some(Ordering::Equal)
    } else {
        version::compare(&plan.minecraft, &current.minecraft)
    };
    match order {
        Some(Ordering::Equal) if installed_build == plan.loader_version.as_deref() => {
            Ok(Decision::UpToDate)
        }
        Some(Ordering::Equal) => Ok(Decision::Install(Change::SameMinecraft)),
        Some(Ordering::Greater) => Ok(Decision::Install(Change::Newer)),
        Some(Ordering::Less) if force => Ok(Decision::Install(Change::Older)),
        Some(Ordering::Less) => Err(Error::Downgrade {
            name: name.clone(),
            current: current.minecraft.clone(),
            target: plan.minecraft.clone(),
        }),
        None if force => Ok(Decision::Install(Change::Newer)),
        None => Err(Error::UnknownOrder {
            current: current.minecraft.clone(),
            target: plan.minecraft.clone(),
        }),
    }
}

fn install_update(
    paths: &Paths,
    agent: &ureq::Agent,
    name: &ServerName,
    owner: Owner,
    current: &Detected,
    plan: &Plan,
    touched: &mut bool,
) -> Result<Detected, Error> {
    let dir = paths.server_dir(name);
    let staging = install::make_staging(paths, name)?;
    let mut apply = || {
        install::download(agent, plan, &staging)?;
        files::hand_over(&staging, owner)?;
        *touched = true;
        copy_as(owner, &staging, &dir)?;
        if let Some(old) = replaced_entry(current, plan) {
            retire_as(owner, &dir, &old)?;
        }
        if let Some(step) = &plan.installer {
            let major = mcctl_platform::java_major(&plan.minecraft, None);
            let java = install::ensure_java(agent, paths, major)?;
            install::run_installer(&dir, &name.user(), owner, step, &java)?;
        }
        let detected = mcctl_platform::detect(&dir).map_err(Error::platform(format!(
            "cannot read the software of {name}"
        )))?;
        install::ensure_java(agent, paths, detected.java)?;
        Ok(detected)
    };
    let result = apply();
    if let Err(err) = files::remove_tree(&staging) {
        output::warn(format!("could not clean up: {err}"));
    }
    result
}

/// A server jar the new files do not overwrite would leave two servers in the folder.
pub(crate) fn replaced_entry(current: &Detected, plan: &Plan) -> Option<PathBuf> {
    match &current.entry {
        Entry::Jar(jar) if !plan.artifacts.iter().any(|artifact| &artifact.dest == jar) => {
            Some(jar.clone())
        }
        Entry::Jar(_) | Entry::ArgFiles { .. } => None,
    }
}

fn retire_as(owner: Owner, dir: &Path, jar: &Path) -> Result<(), Error> {
    let from = dir.join(jar);
    let mut to = from.clone().into_os_string();
    to.push(".old");
    output::info(format!(
        "renaming the old server jar to {}",
        Path::new(&to).display()
    ));
    let mut command = Command::new("mv");
    command.args(["-f", "-T", "--"]).arg(&from).arg(&to);
    process::run(process::as_owner(&mut command, owner)).map(drop)
}

fn copy_as(owner: Owner, staging: &Path, dir: &Path) -> Result<(), Error> {
    let mut contents = OsString::from(staging);
    contents.push("/.");
    let mut command = Command::new("cp");
    command.args(["-a", "--"]).arg(contents).arg(dir);
    process::run(process::as_owner(&mut command, owner)).map(drop)
}

#[cfg(test)]
mod tests {
    use mcctl_platform::{Artifact, Loader};

    use super::*;

    fn detected(platform: Platform, minecraft: &str, entry: Entry) -> Detected {
        Detected {
            platform,
            minecraft: minecraft.to_owned(),
            java: 21,
            entry,
        }
    }

    fn jar(path: &str) -> Entry {
        Entry::Jar(PathBuf::from(path))
    }

    fn plan(loader: Loader, minecraft: &str, build: Option<&str>, dest: &str) -> Plan {
        Plan {
            loader,
            minecraft: minecraft.to_owned(),
            loader_version: build.map(str::to_owned),
            artifacts: vec![Artifact {
                url: "https://example.com/server.jar".to_owned(),
                digest: None,
                dest: PathBuf::from(dest),
            }],
            installer: None,
        }
    }

    fn name() -> ServerName {
        "survival".parse().unwrap()
    }

    #[test]
    fn refuses_downgrade_without_force() {
        let current = detected(Platform::Paper, "26.3", jar("paper.jar"));
        let older = plan(Loader::Paper, "1.21.10", Some("100"), "paper.jar");
        assert!(matches!(
            decide(&name(), &current, None, &older, false),
            Err(Error::Downgrade { .. })
        ));
        assert_eq!(
            decide(&name(), &current, None, &older, true).unwrap(),
            Decision::Install(Change::Older)
        );
        let snapshot = plan(Loader::Vanilla, "26w14a", None, "server.jar");
        assert!(matches!(
            decide(&name(), &current, None, &snapshot, false),
            Err(Error::UnknownOrder { .. })
        ));
    }

    #[test]
    fn decides_between_up_to_date_and_install() {
        let vanilla = detected(Platform::Vanilla, "1.21.1", jar("server.jar"));
        let cases = [
            (
                plan(Loader::Vanilla, "1.21.1", None, "server.jar"),
                None,
                Decision::UpToDate,
            ),
            (
                plan(Loader::Vanilla, "26.3", None, "server.jar"),
                None,
                Decision::Install(Change::Newer),
            ),
            (
                plan(Loader::Vanilla, "1.21.1", Some("7"), "server.jar"),
                None,
                Decision::Install(Change::SameMinecraft),
            ),
            (
                plan(Loader::Vanilla, "1.21.1", Some("7"), "server.jar"),
                Some("7"),
                Decision::UpToDate,
            ),
        ];
        for (plan, build, expected) in cases {
            assert_eq!(
                decide(&name(), &vanilla, build, &plan, false).unwrap(),
                expected,
                "{plan:?}"
            );
        }
    }

    #[test]
    fn reads_installed_loader_build_from_libraries() {
        let forge = detected(
            Platform::Forge,
            "26.3",
            Entry::ArgFiles {
                user: None,
                unix: PathBuf::from("libraries/net/minecraftforge/forge/26.3-66.0.9/unix_args.txt"),
            },
        );
        assert_eq!(installed_build(&forge).as_deref(), Some("66.0.9"));
        let neoforge = detected(
            Platform::NeoForge,
            "1.21.1",
            Entry::ArgFiles {
                user: Some(PathBuf::from("user_jvm_args.txt")),
                unix: PathBuf::from("libraries/net/neoforged/neoforge/21.1.256/unix_args.txt"),
            },
        );
        assert_eq!(installed_build(&neoforge).as_deref(), Some("21.1.256"));
        let paper = detected(Platform::Paper, "1.21.1", jar("paper.jar"));
        assert_eq!(installed_build(&paper), None);
    }

    #[test]
    fn retires_server_jar_the_update_does_not_overwrite() {
        let imported = detected(Platform::Paper, "1.21.1", jar("paper-1.21.1-133.jar"));
        let same_name = detected(Platform::Paper, "1.21.1", jar("paper.jar"));
        let next = plan(Loader::Paper, "1.21.4", Some("200"), "paper.jar");
        assert_eq!(
            replaced_entry(&imported, &next),
            Some(PathBuf::from("paper-1.21.1-133.jar"))
        );
        assert_eq!(replaced_entry(&same_name, &next), None);
    }
}
