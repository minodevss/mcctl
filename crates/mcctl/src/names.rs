use std::fmt;
use std::ops::RangeInclusive;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::Error;

pub(crate) const MAIN_PORT: u16 = 25565;
pub(crate) const INTERNAL_PORTS: RangeInclusive<u16> = 25600..=25999;
pub(crate) const MAX_SERVER_NAME_LEN: usize = 29;
const USER_PREFIX: &str = "mc-";
const MIN_MEMORY_MIB: u32 = 512;
const MAX_MEMORY_MIB: u32 = 1024 * 1024;
const DEFAULT_MEMORY_MIB: u32 = 4096;
const MAX_HOSTNAME_LEN: usize = 253;
const MAX_LABEL_LEN: usize = 63;

/// A server name that is also safe as a file name, a systemd instance and the Linux user `mc-<name>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ServerName(String);

impl ServerName {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn user(&self) -> String {
        format!("{USER_PREFIX}{}", self.0)
    }

    pub(crate) fn unit(&self) -> String {
        format!("mc@{}.service", self.0)
    }
}

impl FromStr for ServerName {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self, Error> {
        match server_name_problem(name) {
            None => Ok(Self(name.to_owned())),
            Some(reason) => Err(Error::InvalidServerName {
                name: name.to_owned(),
                reason,
            }),
        }
    }
}

impl fmt::Display for ServerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn server_name_problem(name: &str) -> Option<&'static str> {
    let Some(first) = name.chars().next() else {
        return Some("it is empty");
    };
    if name.len() > MAX_SERVER_NAME_LEN {
        return Some("use at most 29 characters");
    }
    if !first.is_ascii_lowercase() {
        return Some("start with a lowercase letter");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Some("use only a-z, 0-9 and -");
    }
    if name.ends_with('-') {
        return Some("end with a letter or digit");
    }
    if name.contains("--") {
        return Some("do not use -- inside the name");
    }
    None
}

/// A lowercase DNS name without a trailing dot, with at least two labels.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Hostname(String);

impl Hostname {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Hostname {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Error> {
        let trimmed = input.trim();
        let name = trimmed
            .strip_suffix('.')
            .unwrap_or(trimmed)
            .to_ascii_lowercase();
        match hostname_problem(&name) {
            None => Ok(Self(name)),
            Some(reason) => Err(Error::InvalidHostname {
                input: input.to_owned(),
                reason,
            }),
        }
    }
}

impl fmt::Display for Hostname {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn hostname_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() || name.len() > MAX_HOSTNAME_LEN {
        return Some("use 1 to 253 characters");
    }
    let labels: Vec<&str> = name.split('.').collect();
    if labels.len() < 2 {
        return Some("use a full name like survival.example.com");
    }
    for label in &labels {
        if label.is_empty() || label.len() > MAX_LABEL_LEN {
            return Some("each part between dots needs 1 to 63 characters");
        }
        if !label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Some("use only letters, digits, - and .");
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Some("parts between dots must not start or end with -");
        }
    }
    if labels
        .last()
        .is_some_and(|tld| tld.chars().all(|c| c.is_ascii_digit()))
    {
        return Some("use a hostname, not an ip address");
    }
    None
}

/// Where players reach a server: a hostname routed on the main port, or a dedicated public port.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) enum Address {
    Host(Hostname),
    Port(u16),
}

impl FromStr for Address {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Error> {
        let invalid = |reason: String| Error::InvalidAddress {
            input: input.to_owned(),
            reason,
        };
        let Some(port) = input.trim().strip_prefix(':') else {
            if input.contains(':') {
                return Err(invalid(
                    "use a hostname like survival.example.com or a port like :25566".to_owned(),
                ));
            }
            return input.parse().map(Self::Host);
        };
        let port: u16 = port
            .parse()
            .map_err(|_| invalid("the port must be a number from 1024 to 65535".to_owned()))?;
        dedicated_port_problem(port).map_or(Ok(Self::Port(port)), |reason| Err(invalid(reason)))
    }
}

fn dedicated_port_problem(port: u16) -> Option<String> {
    if port < 1024 {
        return Some("the port must be a number from 1024 to 65535".to_owned());
    }
    if port == MAIN_PORT {
        return Some(format!(
            "port {MAIN_PORT} is the shared hostname port; use a hostname instead"
        ));
    }
    if INTERNAL_PORTS.contains(&port) {
        return Some(format!(
            "ports {}-{} are reserved for servers inside this machine",
            INTERNAL_PORTS.start(),
            INTERNAL_PORTS.end()
        ));
    }
    None
}

impl TryFrom<String> for Address {
    type Error = Error;

    fn try_from(text: String) -> Result<Self, Error> {
        text.parse()
    }
}

impl From<Address> for String {
    fn from(address: Address) -> Self {
        address.to_string()
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host(host) => host.fmt(f),
            Self::Port(port) => write!(f, ":{port}"),
        }
    }
}

/// Java heap size in MiB, written as `8G` or `1536M`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct Memory {
    mib: u32,
}

impl Memory {
    pub(crate) fn mib(self) -> u32 {
        self.mib
    }
}

impl Default for Memory {
    fn default() -> Self {
        Self {
            mib: DEFAULT_MEMORY_MIB,
        }
    }
}

impl FromStr for Memory {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Error> {
        let invalid = |reason| Error::InvalidMemory {
            input: input.to_owned(),
            reason,
        };
        let text = input.trim();
        let (number, factor) = if let Some(gib) = text.strip_suffix(['G', 'g']) {
            (gib, 1024)
        } else if let Some(mib) = text.strip_suffix(['M', 'm']) {
            (mib, 1)
        } else {
            return Err(invalid("use a size like 4G or 1536M"));
        };
        let mib = number
            .parse::<u32>()
            .ok()
            .and_then(|n| n.checked_mul(factor))
            .ok_or_else(|| invalid("use a size like 4G or 1536M"))?;
        if !(MIN_MEMORY_MIB..=MAX_MEMORY_MIB).contains(&mib) {
            return Err(invalid("use between 512M and 1024G"));
        }
        Ok(Self { mib })
    }
}

impl TryFrom<String> for Memory {
    type Error = Error;

    fn try_from(text: String) -> Result<Self, Error> {
        text.parse()
    }
}

impl From<Memory> for String {
    fn from(memory: Memory) -> Self {
        memory.to_string()
    }
}

impl fmt::Display for Memory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mib.is_multiple_of(1024) {
            write!(f, "{}G", self.mib / 1024)
        } else {
            write!(f, "{}M", self.mib)
        }
    }
}

/// Loopback port the server itself listens on; the router forwards to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub(crate) struct InternalPort(u16);

impl InternalPort {
    pub(crate) fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for InternalPort {
    type Error = Error;

    fn try_from(port: u16) -> Result<Self, Error> {
        if INTERNAL_PORTS.contains(&port) {
            Ok(Self(port))
        } else {
            Err(Error::InvalidInternalPort { port })
        }
    }
}

impl From<InternalPort> for u16 {
    fn from(port: InternalPort) -> Self {
        port.0
    }
}

impl fmt::Display for InternalPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_name_accepts_valid_and_rejects_invalid() {
        let cases = [
            ("survival", true),
            ("a", true),
            ("s1", true),
            ("my-server-2", true),
            ("a2345678901234567890123456789", true),
            ("a23456789012345678901234567890", false),
            ("", false),
            ("Survival", false),
            ("1survival", false),
            ("-survival", false),
            ("survival-", false),
            ("my--server", false),
            ("my_server", false),
            ("my.server", false),
            ("my server", false),
            ("../etc", false),
            ("ü", false),
        ];
        for (name, valid) in cases {
            assert_eq!(name.parse::<ServerName>().is_ok(), valid, "{name:?}");
        }
    }

    #[test]
    fn name_max_len_fits_linux_user() {
        let longest: ServerName = "a".repeat(MAX_SERVER_NAME_LEN).parse().unwrap();
        assert!(longest.user().len() <= 32, "{}", longest.user());
        assert_eq!(longest.user().len(), 32);
        assert_eq!(longest.unit(), format!("mc@{longest}.service"));
    }

    #[test]
    fn parses_address_host_and_port() {
        let host = |name: &str| Address::Host(name.parse().unwrap());
        let cases = [
            ("survival.example.com", Some(host("survival.example.com"))),
            ("Survival.Example.COM.", Some(host("survival.example.com"))),
            ("mc.my-domain.co.uk", Some(host("mc.my-domain.co.uk"))),
            (":25566", Some(Address::Port(25566))),
            (":65535", Some(Address::Port(65535))),
            (":25565", None),
            (":25600", None),
            (":25999", None),
            (":80", None),
            (":0", None),
            (":70000", None),
            (":", None),
            ("localhost", None),
            ("203.0.113.5", None),
            ("survival.example.com:25566", None),
            ("-bad.example.com", None),
            ("bad..example.com", None),
            ("survival.example.com..", None),
            ("under_score.example.com", None),
            ("", None),
        ];
        for (input, expected) in cases {
            assert_eq!(input.parse::<Address>().ok(), expected, "{input:?}");
        }
        assert_eq!(
            host("survival.example.com").to_string(),
            "survival.example.com"
        );
        assert_eq!(Address::Port(25566).to_string(), ":25566");
    }

    #[test]
    fn parses_memory_sizes() {
        let cases = [
            ("4G", Some(4096)),
            ("8g", Some(8192)),
            ("1536M", Some(1536)),
            ("512m", Some(512)),
            ("1024G", Some(1024 * 1024)),
            ("511M", None),
            ("1025G", None),
            ("4", None),
            ("4GB", None),
            ("G", None),
            ("-4G", None),
            ("4.5G", None),
            ("99999999999G", None),
            ("4€", None),
            ("€", None),
            ("", None),
        ];
        for (input, expected) in cases {
            assert_eq!(
                input.parse::<Memory>().ok().map(Memory::mib),
                expected,
                "{input:?}"
            );
        }
        assert_eq!(Memory::default().to_string(), "4G");
        assert_eq!("1536M".parse::<Memory>().unwrap().to_string(), "1536M");
        assert_eq!("2048M".parse::<Memory>().unwrap().to_string(), "2G");
    }

    #[test]
    fn internal_port_stays_in_reserved_range() {
        assert!(InternalPort::try_from(25600).is_ok());
        assert!(InternalPort::try_from(25999).is_ok());
        assert!(InternalPort::try_from(25599).is_err());
        assert!(InternalPort::try_from(26000).is_err());
    }
}
