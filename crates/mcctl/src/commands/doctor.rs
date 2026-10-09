use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

use crate::commands::Ctx;
use crate::config::{self, Servers};
use crate::error::Error;
use crate::http;
use crate::names::{Address, Hostname, MAIN_PORT, ServerName};
use crate::output;
use crate::paths::Paths;
use crate::process;
use crate::status_file::{self, DnsStatus};
use crate::systemd::{self, ROUTER_UNIT, ServerState, UnitStatus};

const OUTSIDE_CHECK_URL: &str = "https://api.mcsrvstat.us/3";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub(crate) level: Level,
    pub(crate) text: String,
    pub(crate) fix: Option<String>,
}

impl Finding {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            level: Level::Ok,
            text: text.into(),
            fix: None,
        }
    }

    fn warn(text: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            level: Level::Warn,
            text: text.into(),
            fix: Some(fix.into()),
        }
    }

    fn fail(text: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            level: Level::Fail,
            text: text.into(),
            fix: Some(fix.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JavaFact {
    Present(u8),
    Missing(u8),
    Unreadable,
    NoServer(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerFacts {
    pub(crate) name: ServerName,
    pub(crate) address: Option<Address>,
    pub(crate) unit: Option<UnitStatus>,
    pub(crate) java: JavaFact,
}

/// Everything the checks look at, gathered up front so the checks stay pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Facts {
    pub(crate) router_active: bool,
    pub(crate) config_problems: Vec<String>,
    pub(crate) router_listening: bool,
    pub(crate) public_ip: Result<Ipv4Addr, String>,
    pub(crate) dns_provider: bool,
    pub(crate) dns: Option<DnsStatus>,
    pub(crate) resolved: BTreeMap<Hostname, Result<Vec<Ipv4Addr>, String>>,
    pub(crate) servers: Vec<ServerFacts>,
    pub(crate) outside: BTreeMap<Hostname, Result<bool, String>>,
    pub(crate) public_ports: BTreeSet<u16>,
    pub(crate) ufw_status: Option<String>,
}

pub(crate) fn doctor(ctx: &Ctx<'_>) -> Result<(), Error> {
    let facts = gather(ctx.paths);
    let findings = findings(&facts);
    for finding in &findings {
        output::data(format_finding(finding));
    }
    match findings
        .iter()
        .filter(|finding| finding.level == Level::Fail)
        .count()
    {
        0 => Ok(()),
        count => Err(Error::ChecksFailed { count }),
    }
}

pub(crate) fn format_finding(finding: &Finding) -> String {
    let mut line = format!("{:<4}  {}", finding.level.label(), finding.text);
    if let Some(fix) = &finding.fix {
        write!(line, "\n      fix: {fix}").ok();
    }
    line
}

pub(crate) fn findings(facts: &Facts) -> Vec<Finding> {
    let mut all = vec![check_router(facts)];
    all.extend(check_configs(facts));
    all.push(check_listening(facts));
    all.push(check_public_ip(facts));
    all.extend(check_dns(facts));
    all.extend(facts.servers.iter().flat_map(check_server));
    all.extend(check_outside(facts));
    all.extend(check_firewall(facts));
    all
}

fn check_router(facts: &Facts) -> Finding {
    if facts.router_active {
        Finding::ok("router service is running")
    } else {
        Finding::fail(
            "router service is not running",
            "sudo systemctl enable --now mcctl && sudo journalctl -u mcctl -n 50",
        )
    }
}

fn check_configs(facts: &Facts) -> Vec<Finding> {
    if facts.config_problems.is_empty() {
        return vec![Finding::ok(format!(
            "{} valid",
            status_file::counted(facts.servers.len(), "server config")
        ))];
    }
    facts
        .config_problems
        .iter()
        .map(|problem| Finding::fail(problem.clone(), "fix the file under /etc/mcctl"))
        .collect()
}

fn check_listening(facts: &Facts) -> Finding {
    if facts.router_listening {
        Finding::ok(format!("router answers on port {MAIN_PORT}"))
    } else {
        Finding::fail(
            format!("nothing answers on 127.0.0.1:{MAIN_PORT}"),
            "sudo journalctl -u mcctl -n 50",
        )
    }
}

fn check_public_ip(facts: &Facts) -> Finding {
    match &facts.public_ip {
        Ok(ip) => Finding::ok(format!("public ip is {ip}")),
        Err(reason) => Finding::warn(
            format!("cannot find the public ip: {reason}"),
            "check the internet connection; behind carrier-grade nat players cannot reach this machine",
        ),
    }
}

fn check_dns(facts: &Facts) -> Vec<Finding> {
    let fix_record = |host: &Hostname, ip: &str| {
        if facts.dns_provider {
            "mcctl updates its records every 5 minutes; see: sudo journalctl -u mcctl -g dns"
                .to_owned()
        } else {
            format!("create an A record for {host} pointing to {ip}")
        }
    };
    let expected = facts.public_ip.as_ref().ok();
    let mut found = Vec::new();
    for (host, resolved) in &facts.resolved {
        let shown_ip = expected.map_or_else(
            || "this machine's public ip".to_owned(),
            ToString::to_string,
        );
        found.push(match (resolved, expected) {
            (Err(reason), _) => Finding::fail(
                format!("{host} does not resolve: {reason}"),
                fix_record(host, &shown_ip),
            ),
            (Ok(ips), Some(ip)) if ips.contains(ip) => {
                Finding::ok(format!("{host} points to {ip}"))
            }
            (Ok(ips), Some(ip)) => Finding::fail(
                format!(
                    "dns record for {host} points to {}, expected {ip}",
                    join(ips)
                ),
                fix_record(host, &shown_ip),
            ),
            (Ok(ips), None) => Finding::warn(
                format!(
                    "{host} points to {}; cannot compare without the public ip",
                    join(ips)
                ),
                "rerun mcctl doctor when the internet connection works",
            ),
        });
        if facts
            .dns
            .as_ref()
            .is_some_and(|dns| dns.conflicts.iter().any(|name| name == host.as_str()))
        {
            found.push(Finding::warn(
                format!("{host} has a dns record mcctl did not create, so mcctl leaves it alone"),
                format!(
                    "delete that record at your dns provider, or point it at {shown_ip} yourself"
                ),
            ));
        }
    }
    found
}

fn join(ips: &[Ipv4Addr]) -> String {
    if ips.is_empty() {
        return "nothing".to_owned();
    }
    ips.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn check_server(server: &ServerFacts) -> Vec<Finding> {
    let name = &server.name;
    let mut found = vec![check_unit(name, server.unit.as_ref())];
    match &server.java {
        JavaFact::Present(_) => {}
        JavaFact::Missing(major) => found.push(Finding::fail(
            format!("java {major} for {name} is not installed"),
            format!("sudo mcctl start {name}"),
        )),
        JavaFact::Unreadable => found.push(Finding::warn(
            format!("cannot read the folder of {name}"),
            "sudo mcctl doctor",
        )),
        JavaFact::NoServer(reason) => found.push(Finding::fail(
            format!("{name}: {reason}"),
            format!("check /var/lib/mcctl/servers/{name}, or recreate it with: sudo mcctl new {name} --from <dir>"),
        )),
    }
    if server.address.is_none() {
        found.push(Finding::warn(
            format!("{name} has no address, so only this machine can join"),
            format!("add address = \"<host>\" to /etc/mcctl/servers/{name}.toml"),
        ));
    }
    found
}

fn check_unit(name: &ServerName, unit: Option<&UnitStatus>) -> Finding {
    let Some(unit) = unit else {
        return Finding::warn(
            format!("cannot read the state of {name}"),
            format!("systemctl status mc@{name}"),
        );
    };
    let state = systemd::server_state(unit);
    match (state, unit.enabled()) {
        (ServerState::Failed, _) => {
            Finding::fail(format!("{name} failed"), format!("sudo mcctl logs {name}"))
        }
        (ServerState::Restarting, _) => Finding::fail(
            format!(
                "{name} keeps crashing ({})",
                status_file::counted(unit.restarts as usize, "restart")
            ),
            format!("sudo mcctl logs {name}"),
        ),
        (ServerState::Stopped, true) => Finding::warn(
            format!("{name} is enabled but not running"),
            format!("sudo mcctl start {name}, or keep it off with: sudo mcctl stop {name}"),
        ),
        (ServerState::Running | ServerState::Starting, false) => Finding::warn(
            format!("{name} runs but will not start after a reboot"),
            format!("sudo mcctl start {name}"),
        ),
        (ServerState::Unknown, _) => Finding::warn(
            format!(
                "{name} is in an unknown state ({}/{})",
                unit.active, unit.sub
            ),
            format!("systemctl status mc@{name}"),
        ),
        (
            ServerState::Running
            | ServerState::Starting
            | ServerState::Stopping
            | ServerState::Stopped,
            _,
        ) => Finding::ok(format!("{name} is {state}")),
    }
}

fn check_outside(facts: &Facts) -> Vec<Finding> {
    facts
        .outside
        .iter()
        .map(|(host, reachable)| match reachable {
            Ok(true) => Finding::ok(format!("{host} is reachable from the internet")),
            Ok(false) => Finding::warn(
                format!("{host} is not reachable from the internet"),
                format!("forward TCP {MAIN_PORT} on your router to this machine"),
            ),
            Err(reason) => Finding::warn(
                format!("cannot check {host} from outside: {reason}"),
                "rerun mcctl doctor later",
            ),
        })
        .collect()
}

fn check_firewall(facts: &Facts) -> Vec<Finding> {
    let Some(status) = &facts.ufw_status else {
        return Vec::new();
    };
    if !ufw_active(status) {
        return Vec::new();
    }
    facts
        .public_ports
        .iter()
        .filter(|port| !ufw_allows(status, **port))
        .map(|port| {
            Finding::warn(
                format!("the ufw firewall blocks port {port}"),
                format!("sudo ufw allow {port}/tcp"),
            )
        })
        .collect()
}

pub(crate) fn ufw_active(status: &str) -> bool {
    status
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case("status: active"))
}

/// Whether a rule in `ufw status` output allows TCP `port` from anywhere.
pub(crate) fn ufw_allows(status: &str, port: u16) -> bool {
    status.lines().any(|line| {
        let mut words = line.split_whitespace();
        let Some(target) = words.next() else {
            return false;
        };
        let rest: Vec<&str> = words.collect();
        let allows = |word: &&str| matches!(*word, "ALLOW" | "LIMIT");
        let allow = rest.first().is_some_and(allows) || rest.get(1).is_some_and(allows);
        allow && rest.contains(&"Anywhere") && target_covers(target, port)
    })
}

fn target_covers(target: &str, port: u16) -> bool {
    let ports = match target.split_once('/') {
        None => target,
        Some((ports, "tcp")) => ports,
        Some(_) => return false,
    };
    ports.split(',').any(|part| match part.split_once(':') {
        Some((low, high)) => match (low.parse::<u16>(), high.parse::<u16>()) {
            (Ok(low), Ok(high)) => (low..=high).contains(&port),
            _ => false,
        },
        None => part.parse() == Ok(port),
    })
}

fn gather(paths: &Paths) -> Facts {
    let (servers, mut config_problems) = match config::scan_servers(paths) {
        Ok((servers, problems)) => (servers, problems.iter().map(ToString::to_string).collect()),
        Err(err) => (Servers::new(), vec![err.to_string()]),
    };
    if let Err(err) = config::check_servers(&servers) {
        config_problems.push(err.to_string());
    }
    let global = config::load_global(paths);
    if let Err(err) = &global {
        config_problems.push(err.to_string());
    }
    let published = status_file::read(paths);
    let public_ip = published
        .as_ref()
        .and_then(|published| published.public_ip)
        .map_or_else(
            || mcctl_dns::public_ipv4(&mcctl_dns::agent()).map_err(|err| err.to_string()),
            Ok,
        );
    let hostnames = config::hostnames(&servers);
    let agent = http::agent();
    Facts {
        router_active: systemd::is_active(ROUTER_UNIT),
        config_problems,
        router_listening: TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, MAIN_PORT)),
            CONNECT_TIMEOUT,
        )
        .is_ok(),
        public_ip,
        dns_provider: global.is_ok_and(|global| !global.dns.is_off()),
        dns: published.map(|published| published.dns),
        resolved: hostnames
            .iter()
            .map(|host| (host.clone(), resolve(host)))
            .collect(),
        servers: servers
            .iter()
            .map(|(name, config)| server_facts(paths, name, config.address.clone()))
            .collect(),
        outside: hostnames
            .iter()
            .map(|host| (host.clone(), reachable_from_outside(&agent, host)))
            .collect(),
        public_ports: public_ports(&servers),
        ufw_status: ufw_status(),
    }
}

fn public_ports(servers: &Servers) -> BTreeSet<u16> {
    std::iter::once(MAIN_PORT)
        .chain(servers.values().filter_map(|config| match config.address {
            Some(Address::Port(port)) => Some(port),
            Some(Address::Host(_)) | None => None,
        }))
        .collect()
}

fn resolve(host: &Hostname) -> Result<Vec<Ipv4Addr>, String> {
    let addrs = (host.as_str(), MAIN_PORT)
        .to_socket_addrs()
        .map_err(|err| err.to_string())?;
    Ok(addrs
        .filter_map(|addr| match addr {
            SocketAddr::V4(v4) => Some(*v4.ip()),
            SocketAddr::V6(_) => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

fn server_facts(paths: &Paths, name: &ServerName, address: Option<Address>) -> ServerFacts {
    let java = match mcctl_platform::detect(&paths.server_dir(name)) {
        Ok(detected) => {
            if mcctl_platform::java_path(&paths.java_root(), detected.java).exists() {
                JavaFact::Present(detected.java)
            } else {
                JavaFact::Missing(detected.java)
            }
        }
        Err(mcctl_platform::Error::Io { source, .. })
            if source.kind() == io::ErrorKind::PermissionDenied =>
        {
            JavaFact::Unreadable
        }
        Err(err) => JavaFact::NoServer(err.to_string()),
    };
    ServerFacts {
        name: name.clone(),
        address,
        unit: systemd::show(&name.unit()).ok(),
        java,
    }
}

#[derive(Deserialize)]
struct OutsideStatus {
    online: bool,
}

fn reachable_from_outside(agent: &ureq::Agent, host: &Hostname) -> Result<bool, String> {
    http::get_json::<OutsideStatus>(agent, &format!("{OUTSIDE_CHECK_URL}/{host}"))
        .map(|status| status.online)
        .map_err(|err| err.to_string())
}

/// `None` when ufw is missing or its status cannot be read, as happens without root.
fn ufw_status() -> Option<String> {
    match process::probe(Command::new("ufw").arg("status")) {
        Ok((Some(0), stdout)) => Some(stdout),
        Ok(_) | Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 7);

    fn host(name: &str) -> Hostname {
        name.parse().unwrap()
    }

    fn healthy() -> Facts {
        Facts {
            router_active: true,
            config_problems: Vec::new(),
            router_listening: true,
            public_ip: Ok(PUBLIC),
            dns_provider: true,
            dns: None,
            resolved: BTreeMap::from([(host("survival.example.com"), Ok(vec![PUBLIC]))]),
            servers: vec![ServerFacts {
                name: "survival".parse().unwrap(),
                address: Some(Address::Host(host("survival.example.com"))),
                unit: Some(UnitStatus {
                    active: "active".to_owned(),
                    sub: "running".to_owned(),
                    file_state: "enabled".to_owned(),
                    restarts: 0,
                }),
                java: JavaFact::Present(25),
            }],
            outside: BTreeMap::from([(host("survival.example.com"), Ok(true))]),
            public_ports: BTreeSet::from([MAIN_PORT]),
            ufw_status: None,
        }
    }

    fn not_ok(facts: &Facts) -> Vec<Finding> {
        findings(facts)
            .into_iter()
            .filter(|finding| finding.level != Level::Ok)
            .collect()
    }

    #[test]
    fn healthy_setup_has_only_ok_lines() {
        assert_eq!(not_ok(&healthy()), []);
    }

    #[test]
    fn doctor_flags_dns_mismatch() {
        let mut facts = healthy();
        facts.resolved.insert(
            host("survival.example.com"),
            Ok(vec![Ipv4Addr::new(198, 51, 100, 9)]),
        );
        let problems = not_ok(&facts);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].level, Level::Fail);
        assert_eq!(
            problems[0].text,
            "dns record for survival.example.com points to 198.51.100.9, expected 203.0.113.7"
        );
        facts.dns_provider = false;
        assert_eq!(
            not_ok(&facts)[0].fix.as_deref(),
            Some("create an A record for survival.example.com pointing to 203.0.113.7")
        );
    }

    #[test]
    fn doctor_flags_unreachable_outside() {
        let mut facts = healthy();
        facts
            .outside
            .insert(host("survival.example.com"), Ok(false));
        let problems = not_ok(&facts);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].level, Level::Warn);
        assert_eq!(
            format_finding(&problems[0]),
            "warn  survival.example.com is not reachable from the internet\n      fix: forward TCP 25565 on your router to this machine"
        );
    }

    #[test]
    fn doctor_flags_stopped_router_and_missing_java() {
        let mut facts = healthy();
        facts.router_active = false;
        facts.router_listening = false;
        facts.servers[0].java = JavaFact::Missing(25);
        let failed: Vec<String> = not_ok(&facts)
            .into_iter()
            .filter(|finding| finding.level == Level::Fail)
            .map(|finding| finding.text)
            .collect();
        assert_eq!(
            failed,
            [
                "router service is not running",
                "nothing answers on 127.0.0.1:25565",
                "java 25 for survival is not installed",
            ]
        );
    }

    #[test]
    fn doctor_compares_unit_state_with_enablement() {
        let unit = |active: &str, sub: &str, file_state: &str| UnitStatus {
            active: active.to_owned(),
            sub: sub.to_owned(),
            file_state: file_state.to_owned(),
            restarts: 2,
        };
        let name: ServerName = "survival".parse().unwrap();
        let cases = [
            (unit("active", "running", "enabled"), Level::Ok),
            (unit("inactive", "dead", "disabled"), Level::Ok),
            (unit("inactive", "dead", "enabled"), Level::Warn),
            (unit("active", "running", "disabled"), Level::Warn),
            (unit("failed", "failed", "enabled"), Level::Fail),
            (unit("activating", "auto-restart", "enabled"), Level::Fail),
        ];
        for (status, expected) in cases {
            assert_eq!(
                check_unit(&name, Some(&status)).level,
                expected,
                "{status:?}"
            );
        }
    }

    #[test]
    fn reads_ufw_rules() {
        let status = "Status: active\n\nTo                         Action      From\n--                         ------      ----\n22/tcp                     ALLOW       Anywhere\n25565/tcp                  ALLOW       Anywhere\n25570:25580/tcp            ALLOW       Anywhere\n25590/udp                  ALLOW       Anywhere\n25591                      ALLOW IN    Anywhere\n25592/tcp                  DENY        Anywhere\n25594/tcp                  LIMIT       Anywhere\n25593/tcp                  ALLOW       192.168.1.0/24\n";
        assert!(ufw_active(status));
        assert!(!ufw_active("Status: inactive\n"));
        let cases = [
            (25565, true),
            (25575, true),
            (25590, false),
            (25591, true),
            (25592, false),
            (25593, false),
            (25594, true),
            (25566, false),
        ];
        for (port, allowed) in cases {
            assert_eq!(ufw_allows(status, port), allowed, "{port}");
        }
        let mut facts = healthy();
        facts.ufw_status = Some("Status: active\n".to_owned());
        assert_eq!(
            not_ok(&facts)[0].fix.as_deref(),
            Some("sudo ufw allow 25565/tcp")
        );
    }
}
