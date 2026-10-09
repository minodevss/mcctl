//! Keeps DNS A records for server addresses pointed at the public IPv4.
//! Never touches records it did not create; synchronous, no scheduling, no printing.

mod cloudflare;
mod duckdns;
mod error;
mod exec;
mod http;
mod ip;
mod names;
mod porkbun;
mod reconcile;
mod remembered;

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use ureq::config::IpFamily;

pub use error::Error;
pub use ip::public_ipv4;
pub use reconcile::{Action, Desired, Listing, Owned, Provider, Report, plan, remove, sync};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub enum Config {
    Cloudflare {
        token: String,
    },
    Porkbun {
        api_key: String,
        secret_key: String,
    },
    DuckDns {
        token: String,
    },
    Exec {
        command: PathBuf,
        token: Option<String>,
    },
    None,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cloudflare { .. } => f.write_str("Cloudflare"),
            Self::Porkbun { .. } => f.write_str("Porkbun"),
            Self::DuckDns { .. } => f.write_str("DuckDns"),
            Self::Exec { command, .. } => f
                .debug_struct("Exec")
                .field("command", command)
                .finish_non_exhaustive(),
            Self::None => f.write_str("None"),
        }
    }
}

/// HTTPS-only agent that connects over IPv4, gives up after 10 s, and keeps 4xx/5xx bodies readable.
pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .ip_family(IpFamily::Ipv4Only)
        .https_only(true)
        .http_status_as_error(false)
        .timeout_global(Some(REQUEST_TIMEOUT))
        .user_agent(concat!("mcctl/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

/// Returns `None` for `Config::None`.
pub fn provider(config: &Config, agent: ureq::Agent) -> Option<Box<dyn Provider>> {
    match config {
        Config::Cloudflare { token } => Some(Box::new(cloudflare::Cloudflare::new(agent, token))),
        Config::Porkbun {
            api_key,
            secret_key,
        } => Some(Box::new(porkbun::Porkbun::new(agent, api_key, secret_key))),
        Config::DuckDns { token } => Some(Box::new(remembered::Remembered::new(
            duckdns::DuckDns::new(agent, token),
        ))),
        Config::Exec { command, token } => Some(Box::new(remembered::Remembered::new(
            exec::Exec::new(command, token.as_deref()),
        ))),
        Config::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_debug_hides_secrets() {
        let configs = [
            Config::Cloudflare {
                token: "cf-secret".into(),
            },
            Config::Porkbun {
                api_key: "pk1_secret".into(),
                secret_key: "sk1_secret".into(),
            },
            Config::DuckDns {
                token: "duck-secret".into(),
            },
            Config::Exec {
                command: "/usr/local/bin/dns-hook".into(),
                token: Some("hook-secret".into()),
            },
        ];
        for config in configs {
            let shown = format!("{config:?}");
            assert!(!shown.contains("secret"), "{shown}");
        }
    }
}
