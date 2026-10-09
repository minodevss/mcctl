use std::fs;
use std::path::{Path, PathBuf};

use mcctl_platform::{Detected, Loader, Plan};

use crate::cli::NewArgs;
use crate::commands::Ctx;
use crate::config::{self, ServerConfig, Servers};
use crate::error::Error;
use crate::files;
use crate::http;
use crate::install;
use crate::lock;
use crate::names::{Address, Memory, ServerName};
use crate::output;
use crate::paths::Paths;
use crate::ports;
use crate::users;

enum Software {
    Download(Plan),
    Import { dir: PathBuf, detected: Detected },
}

impl Software {
    fn summary(&self) -> String {
        match self {
            Self::Download(plan) => {
                install::plan_summary(plan, mcctl_platform::java_major(&plan.minecraft, None))
            }
            Self::Import { dir, detected } => format!(
                "{}, java {} (copied from {})",
                install::detected_version(detected),
                detected.java,
                dir.display()
            ),
        }
    }
}

/// What this run created, so a failure can take it back.
#[derive(Default)]
struct Created {
    user: Option<String>,
    staging: Option<PathBuf>,
    server_dir: Option<PathBuf>,
}

impl Created {
    fn undo(self) {
        for dir in [self.staging, self.server_dir].into_iter().flatten() {
            if let Err(err) = files::remove_tree(&dir) {
                output::warn(format!("could not clean up: {err}"));
            }
        }
        if let Some(user) = self.user
            && let Err(err) = users::delete_user(&user)
        {
            output::warn(format!("could not remove user {user}: {err}"));
        }
    }
}

pub(crate) fn new(ctx: &Ctx<'_>, args: NewArgs) -> Result<(), Error> {
    let _lock = lock::acquire(ctx.paths)?;
    let name = args.server;
    let servers = config::load_servers(ctx.paths)?;
    refuse_existing(ctx.paths, &name)?;
    if let Some(address) = &args.address
        && let Some(owner) = config::address_owner(&servers, address)
    {
        return Err(Error::AddressTaken {
            address: address.to_string(),
            server: owner.clone(),
        });
    }
    let agent = http::agent();
    let software = choose_software(&agent, args.from, args.loader, args.version.as_deref())?;
    output::info(format!("{name}: {}", software.summary()));
    if let Software::Download(plan) = &software {
        install::print_loader_notice(plan.loader);
    }
    install::print_eula_notice();
    if !output::confirm("Accept the Minecraft EULA?", ctx.yes)? {
        return Err(Error::EulaDeclined);
    }
    let wanted = Wanted {
        name: &name,
        address: args.address,
        memory: args.memory.unwrap_or_default(),
    };
    let mut created = Created::default();
    let made = create(
        ctx.paths,
        &agent,
        &wanted,
        &software,
        &servers,
        &mut created,
    );
    let (config, detected) = match made {
        Ok(made) => made,
        Err(err) => {
            created.undo();
            return Err(err);
        }
    };
    if let Some(owner) = users::sudo_user() {
        match users::add_to_group(&owner, &name.user()) {
            Ok(()) => output::info(format!(
                "{owner} can now open {} (after logging in again)",
                ctx.paths.server_dir(&name).display()
            )),
            Err(err) => output::warn(format!("could not give {owner} access: {err}")),
        }
    }
    print_created(ctx.paths, &name, &config, &detected);
    Ok(())
}

struct Wanted<'a> {
    name: &'a ServerName,
    address: Option<Address>,
    memory: Memory,
}

fn refuse_existing(paths: &Paths, name: &ServerName) -> Result<(), Error> {
    for path in [paths.server_config(name), paths.server_dir(name)] {
        if files::exists_nofollow(&path)? {
            return Err(Error::ServerExists {
                name: name.clone(),
                what: path,
            });
        }
    }
    if users::lookup(&name.user())?.is_some() {
        return Err(Error::UserExists { name: name.clone() });
    }
    Ok(())
}

fn choose_software(
    agent: &ureq::Agent,
    from: Option<PathBuf>,
    loader: Option<Loader>,
    version: Option<&str>,
) -> Result<Software, Error> {
    if let Some(dir) = from {
        let detected = mcctl_platform::detect(&dir)
            .map_err(Error::platform(format!("cannot use {}", dir.display())))?;
        return Ok(Software::Import { dir, detected });
    }
    let loader = loader.unwrap_or(Loader::Vanilla);
    mcctl_platform::resolve(agent, loader, version)
        .map(Software::Download)
        .map_err(Error::platform(format!(
            "cannot find {loader} {}",
            version.unwrap_or("latest")
        )))
}

fn create(
    paths: &Paths,
    agent: &ureq::Agent,
    wanted: &Wanted<'_>,
    software: &Software,
    servers: &Servers,
    created: &mut Created,
) -> Result<(ServerConfig, Detected), Error> {
    let name = wanted.name;
    let user = name.user();
    let server_dir = paths.server_dir(name);
    let owner = users::create_server_user(&user, &server_dir)?;
    created.user = Some(user.clone());
    let staging = install::make_staging(paths, name)?;
    created.staging = Some(staging.clone());
    fill_staging(agent, software, &staging)?;
    fs::write(staging.join("eula.txt"), "eula=true\n")
        .map_err(Error::io(staging.join("eula.txt")))?;
    files::hand_over(&staging, owner)?;
    if let Software::Download(plan) = software
        && let Some(step) = &plan.installer
    {
        let java = install::ensure_java(
            agent,
            paths,
            mcctl_platform::java_major(&plan.minecraft, None),
        )?;
        install::run_installer(&staging, &user, owner, step, &java)?;
    }
    fs::rename(&staging, &server_dir).map_err(Error::io(&server_dir))?;
    created.staging = None;
    created.server_dir = Some(server_dir.clone());
    let detected = mcctl_platform::detect(&server_dir)
        .map_err(Error::platform(format!("cannot start {name}")))?;
    install::ensure_java(agent, paths, detected.java)?;
    let config = ServerConfig {
        address: wanted.address.clone(),
        memory: wanted.memory,
        internal_port: ports::allocate(servers)?,
    };
    config::save_server(paths, name, &config)?;
    Ok((config, detected))
}

fn fill_staging(agent: &ureq::Agent, software: &Software, staging: &Path) -> Result<(), Error> {
    match software {
        Software::Download(plan) => install::download(agent, plan, staging),
        Software::Import { dir, .. } => {
            output::info(format!("copying {}", dir.display()));
            let skipped = files::copy_contents(dir, staging)?;
            if skipped > 0 {
                output::warn(format!("skipped {skipped} symlinks or special files"));
            }
            Ok(())
        }
    }
}

fn print_created(paths: &Paths, name: &ServerName, config: &ServerConfig, detected: &Detected) {
    output::info(format!(
        "created {name}: {}, java {} in {}",
        install::detected_version(detected),
        detected.java,
        paths.server_dir(name).display()
    ));
    match &config.address {
        Some(Address::Host(host)) => output::info(format!("players join at {host}")),
        Some(Address::Port(port)) => output::info(format!("players join at <public ip>:{port}")),
        None => output::info(format!(
            "no address: only this machine can join; add address = \"<host>\" to {}",
            paths.server_config(name).display()
        )),
    }
    output::data(format!("start with: sudo mcctl start {name}"));
}
