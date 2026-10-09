use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use mcctl_router::{Event, RejectReason};
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};

use crate::output::{self, Level};

const SUMMARY_INTERVAL: Duration = Duration::from_secs(300);
const OFFLINE_QUIET: Duration = Duration::from_secs(60);
const SAMPLE_HOSTS: usize = 3;
const SAMPLE_HOST_CHARS: usize = 64;

pub(super) async fn log_events(mut events: mpsc::Receiver<Event>) {
    let mut rejections = Rejections::default();
    let mut offline_logged: HashMap<String, Instant> = HashMap::new();
    let mut ticker = tokio::time::interval_at(Instant::now() + SUMMARY_INTERVAL, SUMMARY_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            event = events.recv() => match event {
                None => break,
                Some(Event::Rejected { reason, .. }) => rejections.add(&reason),
                Some(Event::Offline { server }) => {
                    let now = Instant::now();
                    let quiet = offline_logged
                        .get(&server)
                        .is_some_and(|logged| now.duration_since(*logged) < OFFLINE_QUIET);
                    if !quiet {
                        output::info(format!("{server} is offline; players see the offline message"));
                        offline_logged.insert(server, now);
                    }
                }
                Some(
                    event @ (Event::Listening { .. }
                    | Event::StoppedListening { .. }
                    | Event::BindFailed { .. }
                    | Event::AcceptFailed { .. }),
                ) => {
                    if let Some((level, line)) = describe(&event) {
                        output::log(level, line);
                    }
                }
            },
            _ = ticker.tick() => {
                if let Some(line) = rejections.take_summary() {
                    output::info(line);
                }
            }
        }
    }
    if let Some(line) = rejections.take_summary() {
        output::info(line);
    }
}

/// One log line per event; rejections are summed up separately.
pub(super) fn describe(event: &Event) -> Option<(Level, String)> {
    match event {
        Event::Listening { port } => Some((Level::Info, format!("listening on port {port}"))),
        Event::StoppedListening { port } => {
            Some((Level::Info, format!("stopped listening on port {port}")))
        }
        Event::BindFailed { port, error } => Some((
            Level::Warn,
            format!("cannot listen on port {port}: {error}"),
        )),
        Event::AcceptFailed { port, error } => Some((
            Level::Warn,
            format!("cannot accept connections on port {port}: {error}"),
        )),
        Event::Offline { server } => Some((Level::Info, format!("{server} is offline"))),
        Event::Rejected { .. } => None,
    }
}

#[derive(Debug, Default)]
pub(super) struct Rejections {
    counts: BTreeMap<&'static str, u64>,
    hosts: BTreeSet<String>,
}

impl Rejections {
    pub(super) fn add(&mut self, reason: &RejectReason) {
        let key = match reason {
            RejectReason::TooManyFromIp => "too many from one address",
            RejectReason::TooManyConnections => "router full",
            RejectReason::HandshakeTimeout => "no handshake in time",
            RejectReason::BadHandshake(_) => "bad handshake",
            RejectReason::UnknownHost { host } => {
                if self.hosts.len() < SAMPLE_HOSTS {
                    let short: String = host.chars().take(SAMPLE_HOST_CHARS).collect();
                    self.hosts.insert(format!("{short:?}"));
                }
                "unknown host"
            }
        };
        *self.counts.entry(key).or_default() += 1;
    }

    /// The line for everything since the last summary, then starts counting afresh.
    pub(super) fn take_summary(&mut self) -> Option<String> {
        let total: u64 = self.counts.values().sum();
        if total == 0 {
            return None;
        }
        let parts: Vec<String> = self
            .counts
            .iter()
            .map(|(reason, count)| {
                if *reason == "unknown host" && !self.hosts.is_empty() {
                    let hosts: Vec<&str> = self.hosts.iter().map(String::as_str).collect();
                    format!("{count} {reason} ({})", hosts.join(", "))
                } else {
                    format!("{count} {reason}")
                }
            })
            .collect();
        *self = Self::default();
        Some(format!(
            "rejected {total} connections: {}",
            parts.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    #[test]
    fn sums_up_rejections_by_reason() {
        let mut rejections = Rejections::default();
        assert_eq!(rejections.take_summary(), None);
        for host in [
            "a.example.com",
            "b.example.com",
            "a.example.com",
            "evil\n\u{1b}[31m",
        ] {
            rejections.add(&RejectReason::UnknownHost {
                host: host.to_owned(),
            });
        }
        rejections.add(&RejectReason::HandshakeTimeout);
        assert_eq!(
            rejections.take_summary().as_deref(),
            Some(
                "rejected 5 connections: 1 no handshake in time, 4 unknown host (\"a.example.com\", \"b.example.com\", \"evil\\n\\u{1b}[31m\")"
            )
        );
        assert_eq!(rejections.take_summary(), None);
    }

    #[test]
    fn describes_router_events() {
        let cases = [
            (
                Event::Listening { port: 25565 },
                Some((Level::Info, "listening on port 25565".to_owned())),
            ),
            (
                Event::BindFailed {
                    port: 25566,
                    error: "address in use".to_owned(),
                },
                Some((
                    Level::Warn,
                    "cannot listen on port 25566: address in use".to_owned(),
                )),
            ),
            (
                Event::Rejected {
                    ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                    reason: RejectReason::TooManyConnections,
                },
                None,
            ),
        ];
        for (event, expected) in cases {
            assert_eq!(describe(&event), expected);
        }
    }
}
