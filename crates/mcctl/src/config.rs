use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use mcctl_router::{Backend, RouteTable};
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::files;
use crate::names::{Address, Hostname, InternalPort, MAIN_PORT, Memory, ServerName};
use crate::paths::Paths;

const TOKEN_FILE: &str = "dns-token";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct ServerConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) address: Option<Address>,
    #[serde(default)]
    pub(crate) memory: Memory,
    pub(crate) internal_port: InternalPort,
}

pub(crate) type Servers = BTreeMap<ServerName, ServerConfig>;

/// A `*.toml` file in the servers folder and the server name its stem parses to.
pub(crate) type ServerFile = (Result<ServerName, Error>, PathBuf);

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct GlobalConfig {
    #[serde(default)]
    pub(crate) dns: DnsSetting,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawDns")]
pub(crate) enum DnsSetting {
    #[default]
    Off,
    Cloudflare,
    DuckDns,
    Porkbun,
    Exec {
        command: PathBuf,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawDns {
    provider: ProviderName,
    #[serde(default)]
    command: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ProviderName {
    Cloudflare,
    Duckdns,
    Porkbun,
    Exec,
    None,
}

impl TryFrom<RawDns> for DnsSetting {
    type Error = Error;

    fn try_from(raw: RawDns) -> Result<Self, Error> {
        match (raw.provider, raw.command) {
            (ProviderName::Exec, Some(command)) => Ok(Self::Exec { command }),
            (ProviderName::Exec, None) => Err(Error::ExecWithoutCommand),
            (
                ProviderName::Cloudflare
                | ProviderName::Duckdns
                | ProviderName::Porkbun
                | ProviderName::None,
                Some(_),
            ) => Err(Error::CommandWithoutExec),
            (ProviderName::Cloudflare, None) => Ok(Self::Cloudflare),
            (ProviderName::Duckdns, None) => Ok(Self::DuckDns),
            (ProviderName::Porkbun, None) => Ok(Self::Porkbun),
            (ProviderName::None, None) => Ok(Self::Off),
        }
    }
}

impl DnsSetting {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Off => "none",
            Self::Cloudflare => "cloudflare",
            Self::DuckDns => "duckdns",
            Self::Porkbun => "porkbun",
            Self::Exec { .. } => "exec",
        }
    }

    pub(crate) fn is_off(&self) -> bool {
        *self == Self::Off
    }

    /// Builds the provider config; `token` is already trimmed and never appears in errors.
    pub(crate) fn with_token(&self, token: &str) -> Result<mcctl_dns::Config, Error> {
        let required = || {
            if token.is_empty() {
                Err(Error::DnsTokenMissing {
                    provider: self.name(),
                })
            } else {
                Ok(token.to_owned())
            }
        };
        match self {
            Self::Off => Ok(mcctl_dns::Config::None),
            Self::Cloudflare => Ok(mcctl_dns::Config::Cloudflare { token: required()? }),
            Self::DuckDns => Ok(mcctl_dns::Config::DuckDns { token: required()? }),
            Self::Porkbun => {
                let token = required()?;
                match token.split_once(':') {
                    Some((api_key, secret_key))
                        if !api_key.is_empty() && !secret_key.is_empty() =>
                    {
                        Ok(mcctl_dns::Config::Porkbun {
                            api_key: api_key.to_owned(),
                            secret_key: secret_key.to_owned(),
                        })
                    }
                    Some(_) | None => Err(Error::PorkbunToken),
                }
            }
            Self::Exec { command } => Ok(mcctl_dns::Config::Exec {
                command: command.clone(),
                token: (!token.is_empty()).then(|| token.to_owned()),
            }),
        }
    }
}

/// The DNS token from the systemd credential, else from /etc/mcctl/dns-token; empty when absent.
pub(crate) fn read_token(paths: &Paths) -> Result<String, Error> {
    let credential = std::env::var_os("CREDENTIALS_DIRECTORY")
        .map(|dir| PathBuf::from(dir).join(TOKEN_FILE))
        .filter(|path| path.exists());
    let path = credential.unwrap_or_else(|| paths.dns_token());
    Ok(files::read_optional(&path)?
        .map(|token| token.trim().to_owned())
        .unwrap_or_default())
}

pub(crate) fn parse_global(text: &str, path: &Path) -> Result<GlobalConfig, Error> {
    toml::from_str(text).map_err(|err| syntax(path, &err))
}

pub(crate) fn parse_server(text: &str, path: &Path) -> Result<ServerConfig, Error> {
    toml::from_str(text).map_err(|err| syntax(path, &err))
}

fn syntax(path: &Path, err: &toml::de::Error) -> Error {
    Error::ConfigSyntax {
        path: path.to_path_buf(),
        message: err.message().to_owned(),
    }
}

pub(crate) fn format_server(name: &ServerName, config: &ServerConfig) -> Result<String, Error> {
    toml::to_string(config).map_err(|err| Error::ConfigFormat {
        server: name.clone(),
        message: err.to_string(),
    })
}

/// A missing `mcctl.toml` means no DNS provider.
pub(crate) fn load_global(paths: &Paths) -> Result<GlobalConfig, Error> {
    let path = paths.global_config();
    files::read_optional(&path)?.map_or_else(
        || Ok(GlobalConfig::default()),
        |text| parse_global(&text, &path),
    )
}

pub(crate) fn load_server(paths: &Paths, name: &ServerName) -> Result<ServerConfig, Error> {
    find_server(paths, name)?.ok_or_else(|| Error::NoSuchServer { name: name.clone() })
}

pub(crate) fn find_server(paths: &Paths, name: &ServerName) -> Result<Option<ServerConfig>, Error> {
    let path = paths.server_config(name);
    files::read_optional(&path)?
        .map(|text| parse_server(&text, &path))
        .transpose()
}

pub(crate) fn save_server(
    paths: &Paths,
    name: &ServerName,
    config: &ServerConfig,
) -> Result<(), Error> {
    let dir = paths.servers_config_dir();
    fs::create_dir_all(&dir).map_err(Error::io(&dir))?;
    let text = format_server(name, config)?;
    files::write_atomic(&paths.server_config(name), text.as_bytes(), 0o644)
}

/// Server config files in the servers folder; dot files and other extensions are ignored.
pub(crate) fn server_files(paths: &Paths) -> Result<Vec<ServerFile>, Error> {
    let dir = paths.servers_config_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(Error::io(&dir)(err)),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(Error::io(&dir))?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if file_name.starts_with('.') {
            continue;
        }
        if let Some(stem) = file_name.strip_suffix(".toml") {
            found.push((stem.parse(), entry.path()));
        }
    }
    found.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(found)
}

/// Every server config; the first broken file or conflict is an error.
pub(crate) fn load_servers(paths: &Paths) -> Result<Servers, Error> {
    let (servers, mut problems) = scan_servers(paths)?;
    if problems.is_empty() {
        check_servers(&servers)?;
        Ok(servers)
    } else {
        Err(problems.remove(0))
    }
}

/// Every server config that parses, plus one error per file that does not.
pub(crate) fn scan_servers(paths: &Paths) -> Result<(Servers, Vec<Error>), Error> {
    let mut servers = Servers::new();
    let mut problems = Vec::new();
    for (name, path) in server_files(paths)? {
        let loaded = name.and_then(|name| {
            let text = fs::read_to_string(&path).map_err(Error::io(&path))?;
            Ok((name, parse_server(&text, &path)?))
        });
        match loaded {
            Ok((name, config)) => {
                servers.insert(name, config);
            }
            Err(err) => problems.push(err),
        }
    }
    Ok((servers, problems))
}

/// Errors when two servers share a hostname, a public port or an internal port.
pub(crate) fn check_servers(servers: &Servers) -> Result<(), Error> {
    route_table(servers).map(drop)
}

pub(crate) fn route_table(servers: &Servers) -> Result<RouteTable, Error> {
    let mut table = RouteTable::new(MAIN_PORT);
    let mut internal: HashMap<InternalPort, &ServerName> = HashMap::new();
    for (name, config) in servers {
        if let Some(first) = internal.insert(config.internal_port, name) {
            return Err(Error::DuplicateInternalPort {
                port: config.internal_port.get(),
                first: first.clone(),
                second: name.clone(),
            });
        }
        let backend = Backend {
            server: name.to_string(),
            addr: SocketAddr::from(([127, 0, 0, 1], config.internal_port.get())),
        };
        let routed = match &config.address {
            Some(Address::Host(host)) => table.add_host(host.as_str(), backend),
            Some(Address::Port(port)) => table.add_port(*port, backend),
            None => Ok(()),
        };
        routed.map_err(|source| Error::RouteConflict {
            server: name.clone(),
            source,
        })?;
    }
    Ok(table)
}

pub(crate) fn hostnames(servers: &Servers) -> BTreeSet<Hostname> {
    servers
        .values()
        .filter_map(|config| match &config.address {
            Some(Address::Host(host)) => Some(host.clone()),
            Some(Address::Port(_)) | None => None,
        })
        .collect()
}

/// The other server already using `address`, if any.
pub(crate) fn address_owner<'a>(servers: &'a Servers, address: &Address) -> Option<&'a ServerName> {
    servers
        .iter()
        .find(|(_, config)| config.address.as_ref() == Some(address))
        .map(|(name, _)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(address: Option<&str>, port: u16) -> ServerConfig {
        ServerConfig {
            address: address.map(|address| address.parse().unwrap()),
            memory: Memory::default(),
            internal_port: port.try_into().unwrap(),
        }
    }

    fn name(name: &str) -> ServerName {
        name.parse().unwrap()
    }

    #[test]
    fn reads_and_writes_server_config() {
        let text = "address = \"Survival.Example.com\"\nmemory = \"8G\"\ninternal-port = 25600\n";
        let config = parse_server(text, Path::new("survival.toml")).unwrap();
        assert_eq!(
            config.address,
            Some(Address::Host("survival.example.com".parse().unwrap()))
        );
        assert_eq!(config.memory.mib(), 8192);
        assert_eq!(config.internal_port.get(), 25600);
        assert_eq!(
            format_server(&name("survival"), &config).unwrap(),
            "address = \"survival.example.com\"\nmemory = \"8G\"\ninternal-port = 25600\n"
        );
        let minimal = parse_server("internal-port = 25601\n", Path::new("x.toml")).unwrap();
        assert_eq!(minimal, server(None, 25601));
        assert_eq!(
            format_server(&name("x"), &minimal).unwrap(),
            "memory = \"4G\"\ninternal-port = 25601\n"
        );
    }

    #[test]
    fn rejects_unknown_config_keys() {
        let server_cases = [
            "internal-port = 25600\nport = 1\n",
            "internal-port = 25600\ninternal_port = 25601\n",
            "internal-port = 25600\n[extra]\n",
            "internal-port = 25500\n",
            "memory = \"8G\"\n",
            "address = \"localhost\"\ninternal-port = 25600\n",
        ];
        for text in server_cases {
            assert!(parse_server(text, Path::new("s.toml")).is_err(), "{text:?}");
        }
        let global_cases = [
            "[dns]\nprovider = \"cloudflare\"\ntoken = \"secret\"\n",
            "[dns]\nprovider = \"route53\"\n",
            "[dns]\nprovider = \"exec\"\n",
            "[dns]\nprovider = \"cloudflare\"\ncommand = \"/bin/true\"\n",
            "[router]\n",
        ];
        for text in global_cases {
            assert!(
                parse_global(text, Path::new("mcctl.toml")).is_err(),
                "{text:?}"
            );
        }
        let exec = parse_global(
            "[dns]\nprovider = \"exec\"\ncommand = \"/usr/local/bin/dns-hook\"\n",
            Path::new("mcctl.toml"),
        )
        .unwrap();
        assert_eq!(
            exec.dns,
            DnsSetting::Exec {
                command: PathBuf::from("/usr/local/bin/dns-hook")
            }
        );
        assert_eq!(
            parse_global("", Path::new("mcctl.toml")).unwrap().dns,
            DnsSetting::Off
        );
    }

    #[test]
    fn maps_dns_setting_and_token_to_provider_config() {
        assert!(matches!(
            DnsSetting::Cloudflare.with_token(""),
            Err(Error::DnsTokenMissing {
                provider: "cloudflare"
            })
        ));
        assert!(matches!(
            DnsSetting::Porkbun.with_token("pk1_abc:sk1_def"),
            Ok(mcctl_dns::Config::Porkbun { api_key, secret_key }) if api_key == "pk1_abc" && secret_key == "sk1_def"
        ));
        for bad in ["pk1_abc", "pk1_abc:", ":sk1_def"] {
            assert!(matches!(
                DnsSetting::Porkbun.with_token(bad),
                Err(Error::PorkbunToken)
            ));
        }
        let exec = DnsSetting::Exec {
            command: PathBuf::from("/hook"),
        };
        assert!(matches!(
            exec.with_token(""),
            Ok(mcctl_dns::Config::Exec { token: None, .. })
        ));
        assert!(matches!(
            DnsSetting::Off.with_token("x"),
            Ok(mcctl_dns::Config::None)
        ));
    }

    #[test]
    fn builds_route_table_from_configs() {
        let servers = Servers::from([
            (
                name("survival"),
                server(Some("survival.example.com"), 25600),
            ),
            (name("modded"), server(Some(":25570"), 25601)),
            (name("private"), server(None, 25602)),
        ]);
        let table = route_table(&servers).unwrap();
        assert_eq!(table.main_port(), MAIN_PORT);
        assert_eq!(table.listen_ports(), BTreeSet::from([MAIN_PORT, 25570]));
        let survival = table.route(MAIN_PORT, "Survival.Example.com.").unwrap();
        assert_eq!(survival.server, "survival");
        assert_eq!(survival.addr, SocketAddr::from(([127, 0, 0, 1], 25600)));
        let modded = table.route(25570, "anything").unwrap();
        assert_eq!(modded.server, "modded");
        assert_eq!(modded.addr, SocketAddr::from(([127, 0, 0, 1], 25601)));
        assert!(table.route(MAIN_PORT, "private").is_none());
        assert_eq!(
            hostnames(&servers),
            BTreeSet::from(["survival.example.com".parse().unwrap()])
        );
    }

    #[test]
    fn rejects_duplicate_addresses() {
        let same_host = Servers::from([
            (name("a"), server(Some("play.example.com"), 25600)),
            (name("b"), server(Some("PLAY.example.com"), 25601)),
        ]);
        assert!(matches!(
            check_servers(&same_host),
            Err(Error::RouteConflict { server, .. }) if server.as_str() == "b"
        ));
        let same_port = Servers::from([
            (name("a"), server(Some(":25570"), 25600)),
            (name("b"), server(Some(":25570"), 25601)),
        ]);
        assert!(matches!(
            check_servers(&same_port),
            Err(Error::RouteConflict { .. })
        ));
        let same_internal = Servers::from([
            (name("a"), server(None, 25600)),
            (name("b"), server(None, 25600)),
        ]);
        assert!(matches!(
            check_servers(&same_internal),
            Err(Error::DuplicateInternalPort { port: 25600, .. })
        ));
        let address = "play.example.com".parse().unwrap();
        assert_eq!(
            address_owner(&same_host, &address).map(ServerName::as_str),
            Some("a")
        );
    }

    #[test]
    fn loads_server_files_and_reports_broken_ones() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::under(root.path());
        save_server(
            &paths,
            &name("survival"),
            &server(Some("a.example.com"), 25600),
        )
        .unwrap();
        let dir = paths.servers_config_dir();
        fs::write(dir.join("Bad Name.toml"), "internal-port = 25601\n").unwrap();
        fs::write(dir.join("broken.toml"), "internal-port = \n").unwrap();
        fs::write(dir.join(".survival.toml.tmp-1"), "junk").unwrap();
        fs::write(dir.join("notes.txt"), "junk").unwrap();
        let (servers, problems) = scan_servers(&paths).unwrap();
        assert_eq!(
            servers.keys().map(ServerName::as_str).collect::<Vec<_>>(),
            ["survival"]
        );
        assert_eq!(problems.len(), 2);
        assert!(load_servers(&paths).is_err());
        assert_eq!(
            load_server(&paths, &name("survival")).unwrap(),
            server(Some("a.example.com"), 25600)
        );
        assert!(matches!(
            load_server(&paths, &name("missing")),
            Err(Error::NoSuchServer { .. })
        ));
        assert_eq!(load_global(&paths).unwrap(), GlobalConfig::default());
    }
}
