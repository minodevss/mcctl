use crate::commands::Ctx;
use crate::config;
use crate::error::Error;
use crate::install;
use crate::names::{Address, Memory, ServerName};
use crate::output;
use crate::status_file::{self, StatusFile};
use crate::systemd::{self, ROUTER_UNIT, ServerState};

const HEADERS: [&str; 6] = [
    "NAME",
    "STATE",
    "ADDRESS",
    "VERSION",
    "MEMORY",
    "CONNECTIONS",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerRow {
    pub(crate) name: ServerName,
    pub(crate) state: ServerState,
    pub(crate) address: Option<Address>,
    pub(crate) version: Option<String>,
    pub(crate) memory: Memory,
    pub(crate) connections: Option<usize>,
}

pub(crate) fn status(ctx: &Ctx<'_>) -> Result<(), Error> {
    let published = status_file::read(ctx.paths);
    let router_running = systemd::is_active(ROUTER_UNIT);
    output::data(header_line(published.as_ref(), router_running));
    let (servers, problems) = config::scan_servers(ctx.paths)?;
    for problem in &problems {
        output::warn(problem);
    }
    if servers.is_empty() {
        output::data("no servers yet; create one: sudo mcctl new <name> fabric --address <host>");
        return Ok(());
    }
    let rows: Vec<ServerRow> = servers
        .into_iter()
        .map(|(name, config)| {
            let state = systemd::show(&name.unit())
                .map_or(ServerState::Unknown, |unit| systemd::server_state(&unit));
            let version = mcctl_platform::detect(&ctx.paths.server_dir(&name))
                .ok()
                .map(|detected| install::detected_version(&detected));
            let connections = published
                .as_ref()
                .map(|published| published.connections_to(name.as_str()));
            ServerRow {
                name,
                state,
                address: config.address,
                version,
                memory: config.memory,
                connections,
            }
        })
        .collect();
    output::data("");
    for line in format_table(&rows) {
        output::data(line);
    }
    Ok(())
}

pub(crate) fn header_line(published: Option<&StatusFile>, router_running: bool) -> String {
    let ip = published
        .and_then(|published| published.public_ip)
        .map_or_else(|| "unknown".to_owned(), |ip| ip.to_string());
    let dns = published.map_or_else(|| "unknown".to_owned(), |published| published.dns.summary());
    let router = if router_running { "running" } else { "stopped" };
    format!("public ip {ip} · dns {dns} · router {router}")
}

pub(crate) fn format_table(rows: &[ServerRow]) -> Vec<String> {
    let cells: Vec<[String; 6]> = rows
        .iter()
        .map(|row| {
            [
                row.name.to_string(),
                row.state.to_string(),
                row.address
                    .as_ref()
                    .map_or_else(|| "-".to_owned(), ToString::to_string),
                row.version.clone().unwrap_or_else(|| "-".to_owned()),
                row.memory.to_string(),
                row.connections
                    .map_or_else(|| "-".to_owned(), |count| count.to_string()),
            ]
        })
        .collect();
    let mut widths = HEADERS.map(str::len);
    for row in &cells {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let header = HEADERS.map(str::to_owned);
    std::iter::once(&header)
        .chain(&cells)
        .map(|row| join_padded(row, &widths))
        .collect()
}

fn join_padded(row: &[String; 6], widths: &[usize; 6]) -> String {
    let line: Vec<String> = row
        .iter()
        .zip(widths)
        .map(|(cell, width)| format!("{cell:<width$}"))
        .collect();
    line.join("  ").trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::net::Ipv4Addr;

    use super::*;
    use crate::status_file::DnsStatus;

    #[test]
    fn status_table_formats_columns() {
        let rows = [
            ServerRow {
                name: "survival".parse().unwrap(),
                state: ServerState::Running,
                address: Some("survival.example.com".parse().unwrap()),
                version: Some("fabric 26.3".to_owned()),
                memory: "8G".parse().unwrap(),
                connections: Some(3),
            },
            ServerRow {
                name: "lab".parse().unwrap(),
                state: ServerState::Stopped,
                address: None,
                version: None,
                memory: "1536M".parse().unwrap(),
                connections: None,
            },
        ];
        assert_eq!(
            format_table(&rows),
            [
                "NAME      STATE    ADDRESS               VERSION      MEMORY  CONNECTIONS",
                "survival  running  survival.example.com  fabric 26.3  8G      3",
                "lab       stopped  -                     -            1536M   -",
            ]
        );
    }

    #[test]
    fn header_shows_ip_dns_and_router() {
        assert_eq!(
            header_line(None, false),
            "public ip unknown · dns unknown · router stopped"
        );
        let published = StatusFile {
            updated: 1,
            public_ip: Some(Ipv4Addr::new(203, 0, 113, 7)),
            dns: DnsStatus {
                provider: "cloudflare".to_owned(),
                ok: true,
                conflicts: Vec::new(),
                failures: Vec::new(),
                last_sync: Some(1),
            },
            connections: BTreeMap::new(),
        };
        assert_eq!(
            header_line(Some(&published), true),
            "public ip 203.0.113.7 · dns ok · router running"
        );
    }
}
