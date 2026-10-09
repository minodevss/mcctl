use std::cell::RefCell;
use std::collections::BTreeSet;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use mcctl_dns::{Action, Provider, Report};
use tokio::sync::{Notify, watch};

use super::Published;
use super::reload::DnsTarget;
use crate::config::{self, DnsSetting};
use crate::error::Error;
use crate::output::{self, Level};
use crate::paths::Paths;
use crate::status_file::{self, DnsStatus};

const SYNC_INTERVAL: Duration = Duration::from_secs(300);
const FIRST_RETRY: Duration = Duration::from_secs(30);
const MAX_RETRY: Duration = Duration::from_secs(600);

struct CachedProvider {
    setting: DnsSetting,
    token: String,
    provider: Arc<dyn Provider>,
}

/// The result of one round: what to publish, what changed, and what is still wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Round {
    pub(super) ip: Option<Ipv4Addr>,
    pub(super) dns: DnsStatus,
    pub(super) changes: Vec<String>,
    pub(super) problems: Vec<String>,
}

impl Round {
    fn failed(
        setting: &DnsSetting,
        ip: Option<Ipv4Addr>,
        last_sync: Option<u64>,
        problem: String,
    ) -> Self {
        Self {
            ip,
            dns: DnsStatus {
                provider: setting.name().to_owned(),
                ok: false,
                conflicts: Vec::new(),
                failures: vec![problem.clone()],
                last_sync,
            },
            changes: Vec::new(),
            problems: vec![problem],
        }
    }
}

/// Every 5 minutes and on each config change: find the public IP, then sync records.
pub(super) async fn keep_records(
    paths: &Paths,
    mut targets: watch::Receiver<DnsTarget>,
    published: &RefCell<Published>,
    ran: &Notify,
) {
    let mut cache: Option<CachedProvider> = None;
    let mut retry: Option<Duration> = None;
    let mut last_problems: Vec<String> = Vec::new();
    loop {
        let target = targets.borrow_and_update().clone();
        let (previous_ip, last_sync) = {
            let published = published.borrow();
            (published.public_ip, published.dns.last_sync)
        };
        let round = sync_once(paths, &target, &mut cache, last_sync).await;
        log_round(&round, previous_ip, &last_problems);
        let failed = !round.problems.is_empty();
        last_problems.clone_from(&round.problems);
        {
            let mut published = published.borrow_mut();
            published.public_ip = round.ip.or(previous_ip);
            published.dns = round.dns;
        }
        ran.notify_one();
        let delay = if failed {
            let next = next_retry(retry);
            retry = Some(next);
            next
        } else {
            retry = None;
            SYNC_INTERVAL
        };
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            changed = targets.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

/// 30 s, then doubling up to 10 min.
pub(super) fn next_retry(previous: Option<Duration>) -> Duration {
    previous.map_or(FIRST_RETRY, |previous| (previous * 2).min(MAX_RETRY))
}

async fn sync_once(
    paths: &Paths,
    target: &DnsTarget,
    cache: &mut Option<CachedProvider>,
    last_sync: Option<u64>,
) -> Round {
    let setting = &target.setting;
    let agent = mcctl_dns::agent();
    let ip = match blocking(move || mcctl_dns::public_ipv4(&agent)).await {
        Ok(Ok(ip)) => ip,
        Ok(Err(err)) => {
            return Round::failed(
                setting,
                None,
                last_sync,
                format!("cannot find the public ip: {err}"),
            );
        }
        Err(problem) => return Round::failed(setting, None, last_sync, problem),
    };
    if setting.is_off() {
        return Round {
            ip: Some(ip),
            dns: DnsStatus::default(),
            changes: Vec::new(),
            problems: Vec::new(),
        };
    }
    let provider = match provider_for(paths, setting, cache) {
        Ok(provider) => provider,
        Err(err) => return Round::failed(setting, Some(ip), last_sync, err.to_string()),
    };
    let names = target.hostnames.clone();
    match blocking(move || mcctl_dns::sync(provider.as_ref(), &names, ip)).await {
        Ok(Ok(report)) => round_from_report(setting, ip, &report, status_file::unix_now()),
        Ok(Err(err)) => Round::failed(
            setting,
            Some(ip),
            last_sync,
            format!("cannot list dns records: {err}"),
        ),
        Err(problem) => Round::failed(setting, Some(ip), last_sync, problem),
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|err| format!("dns task stopped: {err}"))
}

fn provider_for(
    paths: &Paths,
    setting: &DnsSetting,
    cache: &mut Option<CachedProvider>,
) -> Result<Arc<dyn Provider>, Error> {
    let token = config::read_token(paths)?;
    if let Some(cached) = cache.as_ref()
        && cached.setting == *setting
        && cached.token == token
    {
        return Ok(Arc::clone(&cached.provider));
    }
    let dns_config = setting.with_token(&token)?;
    let provider: Arc<dyn Provider> = mcctl_dns::provider(&dns_config, mcctl_dns::agent())
        .map(Arc::from)
        .ok_or(Error::DnsTokenMissing {
            provider: setting.name(),
        })?;
    *cache = Some(CachedProvider {
        setting: setting.clone(),
        token,
        provider: Arc::clone(&provider),
    });
    Ok(provider)
}

pub(super) fn round_from_report(
    setting: &DnsSetting,
    ip: Ipv4Addr,
    report: &Report,
    now: u64,
) -> Round {
    let failures: Vec<String> = report
        .failures
        .iter()
        .map(|(action, err)| format!("{}: {err}", describe_action(action)))
        .collect();
    let problems = report
        .conflicts
        .iter()
        .map(|name| format!("{name} has a dns record mcctl did not create; leaving it alone"))
        .chain(
            failures
                .iter()
                .map(|failure| format!("failed to apply {failure}")),
        )
        .collect();
    Round {
        ip: Some(ip),
        dns: DnsStatus {
            provider: setting.name().to_owned(),
            ok: report.failures.is_empty() && report.conflicts.is_empty(),
            conflicts: report.conflicts.clone(),
            failures,
            last_sync: Some(now),
        },
        changes: report.applied.iter().map(describe_action).collect(),
        problems,
    }
}

fn describe_action(action: &Action) -> String {
    match action {
        Action::Create(record) => format!("create {} -> {}", record.name, record.ip),
        Action::Update(record, _) => format!("update {} -> {}", record.name, record.ip),
        Action::Delete(owned) => format!("delete {}", owned.name),
        Action::Conflict(name) => format!("skip {name}"),
    }
}

/// Changes always print; problems print only when they differ from the last round.
fn log_round(round: &Round, previous_ip: Option<Ipv4Addr>, last_problems: &[String]) {
    if let Some(ip) = round.ip
        && previous_ip != Some(ip)
    {
        output::info(format!("public ip is {ip}"));
    }
    for change in &round.changes {
        output::info(format!("dns: {change}"));
    }
    if round.problems == last_problems {
        return;
    }
    let before: BTreeSet<&String> = last_problems.iter().collect();
    for problem in round
        .problems
        .iter()
        .filter(|problem| !before.contains(problem))
    {
        output::log(Level::Warn, format_args!("dns: {problem}"));
    }
    if round.problems.is_empty() {
        output::info("dns: records are in sync");
    }
}

#[cfg(test)]
mod tests {
    use mcctl_dns::{Desired, Owned};

    use super::*;

    const IP: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 7);

    #[test]
    fn backs_off_from_30_seconds_to_10_minutes() {
        let mut delay = None;
        let mut seen = Vec::new();
        for _ in 0..7 {
            let next = next_retry(delay);
            seen.push(next.as_secs());
            delay = Some(next);
        }
        assert_eq!(seen, [30, 60, 120, 240, 480, 600, 600]);
    }

    #[test]
    fn turns_sync_report_into_status() {
        let report = Report {
            applied: vec![
                Action::Create(Desired {
                    name: "a.example.com".to_owned(),
                    ip: IP,
                }),
                Action::Delete(Owned {
                    name: "old.example.com".to_owned(),
                    ip: IP,
                    id: None,
                }),
            ],
            conflicts: vec!["taken.example.com".to_owned()],
            failures: vec![(
                Action::Create(Desired {
                    name: "b.example.com".to_owned(),
                    ip: IP,
                }),
                mcctl_dns::Error::NoZone {
                    name: "b.example.com".to_owned(),
                },
            )],
        };
        let round = round_from_report(&DnsSetting::Cloudflare, IP, &report, 42);
        assert_eq!(round.ip, Some(IP));
        assert_eq!(
            round.changes,
            [
                "create a.example.com -> 203.0.113.7",
                "delete old.example.com"
            ]
        );
        assert_eq!(
            round.dns,
            DnsStatus {
                provider: "cloudflare".to_owned(),
                ok: false,
                conflicts: vec!["taken.example.com".to_owned()],
                failures: vec![
                    "create b.example.com -> 203.0.113.7: no dns zone in the account covers b.example.com"
                        .to_owned()
                ],
                last_sync: Some(42),
            }
        );
        assert_eq!(round.problems.len(), 2);
        let clean = round_from_report(&DnsSetting::Cloudflare, IP, &Report::default(), 43);
        assert!(clean.dns.ok);
        assert_eq!(clean.problems, Vec::<String>::new());
    }
}
