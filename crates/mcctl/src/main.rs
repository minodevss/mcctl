//! The mcctl binary: CLI, router service and server launcher for Minecraft Java servers on one host.
//! Never touches worlds, mods, plugins, whitelist, server.properties or backups.

mod cli;
mod commands;
mod config;
mod error;
mod files;
mod http;
mod install;
mod lock;
mod names;
mod output;
mod paths;
mod ports;
mod process;
mod run;
mod serve;
mod status_file;
mod systemd;
mod users;
mod version;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = match cli::Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return output::clap_exit(&err),
    };
    match commands::dispatch(cli, &paths::Paths::system()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            output::error(&err);
            ExitCode::FAILURE
        }
    }
}
