use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use mcctl_platform::Loader;

use crate::names::{Address, Memory, ServerName};

const EXAMPLES: &str = "\
Examples:
  sudo mcctl new survival fabric 26.3 --address survival.example.com --memory 8G
  sudo mcctl start survival
  sudo mcctl console survival say hello";

#[derive(Debug, Parser)]
#[command(
    name = "mcctl",
    version,
    about = "Run Minecraft Java servers on this machine",
    after_help = EXAMPLES,
    disable_help_subcommand = true
)]
pub(crate) struct Cli {
    /// Answer yes to every confirmation
    #[arg(short = 'y', long, global = true)]
    pub(crate) yes: bool,
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Show servers, the router and DNS (default)
    Status,
    /// Create a server
    New(NewArgs),
    /// Delete a server and its world
    Delete {
        /// Server name
        server: ServerName,
    },
    /// Start servers and keep them running across reboots
    Start {
        /// Server names
        #[arg(required = true)]
        servers: Vec<ServerName>,
    },
    /// Stop servers and keep them stopped across reboots
    Stop {
        /// Server names
        #[arg(required = true)]
        servers: Vec<ServerName>,
    },
    /// Restart servers
    Restart {
        /// Server names
        #[arg(required = true)]
        servers: Vec<ServerName>,
    },
    /// Update a server's software, or mcctl itself when no server is given
    Update {
        /// Server name; leave out to update mcctl
        server: Option<ServerName>,
        /// Minecraft version; the newest when left out
        version: Option<String>,
        /// Update even with mods or plugins, or to an older version
        #[arg(long)]
        force: bool,
    },
    /// Send a command to a server, or open its console
    Console {
        /// Server name
        server: ServerName,
        /// Command to send, like: say hello
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Show a server's log
    Logs {
        /// Server name
        server: ServerName,
        /// Keep printing new lines
        #[arg(short, long)]
        follow: bool,
    },
    /// Check the setup and explain how to fix problems
    Doctor,
    /// Remove mcctl from this machine
    Uninstall {
        /// Also delete all servers, worlds and settings
        #[arg(long)]
        purge: bool,
    },
    #[command(hide = true)]
    Serve,
    #[command(hide = true)]
    Run { server: ServerName },
}

#[derive(Debug, Args)]
pub(crate) struct NewArgs {
    /// Server name: a-z, 0-9 and -, up to 29 characters
    pub(crate) server: ServerName,
    /// vanilla, fabric, paper, forge or neoforge
    #[arg(conflicts_with = "from")]
    pub(crate) loader: Option<Loader>,
    /// Minecraft version; the newest when left out
    #[arg(conflicts_with = "from")]
    pub(crate) version: Option<String>,
    /// Copy an existing server folder instead of downloading
    #[arg(long, value_name = "DIR")]
    pub(crate) from: Option<PathBuf>,
    /// survival.example.com, or :25566 for a dedicated port
    #[arg(long, value_name = "ADDR")]
    pub(crate) address: Option<Address>,
    /// Java heap, like 8G or 1536M [default: 4G]
    #[arg(long, value_name = "SIZE")]
    pub(crate) memory: Option<Memory>,
}

impl Command {
    pub(crate) fn needs_root(&self) -> bool {
        match self {
            Self::New(_)
            | Self::Delete { .. }
            | Self::Start { .. }
            | Self::Stop { .. }
            | Self::Restart { .. }
            | Self::Update { .. }
            | Self::Console { .. }
            | Self::Uninstall { .. } => true,
            Self::Status | Self::Logs { .. } | Self::Doctor | Self::Serve | Self::Run { .. } => {
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn new_from_conflicts_with_loader() {
        let parsed = Cli::try_parse_from(["mcctl", "new", "s", "fabric", "--from", "/srv/old"]);
        assert!(parsed.is_err());
        let parsed =
            Cli::try_parse_from(["mcctl", "new", "s", "--from", "/srv/old", "-y"]).unwrap();
        assert!(parsed.yes);
        let Some(Command::New(args)) = parsed.command else {
            panic!("expected new");
        };
        assert_eq!(args.from, Some(PathBuf::from("/srv/old")));
        assert!(args.loader.is_none());
    }

    #[test]
    fn console_takes_the_rest_as_the_command() {
        let parsed =
            Cli::try_parse_from(["mcctl", "console", "survival", "say", "-hello", "world"])
                .unwrap();
        let Some(Command::Console { command, .. }) = parsed.command else {
            panic!("expected console");
        };
        assert_eq!(command, ["say", "-hello", "world"]);
    }

    #[test]
    fn rejects_invalid_names_at_parse_time() {
        assert!(Cli::try_parse_from(["mcctl", "start", "Bad"]).is_err());
        assert!(Cli::try_parse_from(["mcctl", "new", "s", "quilt"]).is_err());
        assert!(Cli::try_parse_from(["mcctl", "new", "s", "--memory", "4"]).is_err());
        assert!(Cli::try_parse_from(["mcctl"]).unwrap().command.is_none());
    }
}
