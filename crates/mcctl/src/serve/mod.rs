mod dns;
mod events;
mod reload;

use std::cell::RefCell;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use mcctl_router::{Settings, Stats};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{Notify, mpsc, watch};

use crate::error::Error;
use crate::output;
use crate::paths::Paths;
use crate::status_file::{self, DnsStatus, StatusFile};

const EVENT_QUEUE: usize = 1024;
const STATUS_INTERVAL: Duration = Duration::from_secs(5);
const SHUTDOWN_WAIT: Duration = Duration::from_secs(1);

/// What the DNS loop learned, for the status file; never borrowed across an await.
#[derive(Debug, Default)]
struct Published {
    public_ip: Option<Ipv4Addr>,
    dns: DnsStatus,
}

/// Runs the router, config reloads, DNS updates and the status file until SIGTERM or SIGINT.
pub(crate) fn serve(paths: &Paths) -> Result<(), Error> {
    output::use_service_style();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::Runtime)?;
    let served = runtime.block_on(serve_until_stopped(paths));
    runtime.shutdown_timeout(SHUTDOWN_WAIT);
    served
}

async fn serve_until_stopped(paths: &Paths) -> Result<(), Error> {
    let mut terminate = signal(SignalKind::terminate()).map_err(Error::Runtime)?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(Error::Runtime)?;
    let shutdown = async move {
        tokio::select! {
            _ = terminate.recv() => {}
            _ = interrupt.recv() => {}
        }
        output::info("shutting down");
    };
    output::info(format!("mcctl {} starting", env!("CARGO_PKG_VERSION")));
    let mut watcher = reload::Watcher::new(paths.clone());
    let initial = watcher.load_or_empty();
    let (routes_tx, routes_rx) = watch::channel(Arc::new(initial.routes));
    let (dns_tx, dns_rx) = watch::channel(initial.dns.clone());
    let stats = Stats::default();
    let (events_tx, events_rx) = mpsc::channel(EVENT_QUEUE);
    let published = RefCell::new(Published {
        public_ip: None,
        dns: DnsStatus {
            provider: initial.dns.setting.name().to_owned(),
            ..DnsStatus::default()
        },
    });
    let dns_ran = Notify::new();
    let router = mcctl_router::run(
        routes_rx,
        Settings::default(),
        stats.clone(),
        events_tx,
        shutdown,
    );
    let background = async {
        tokio::join!(
            reload::watch(watcher, routes_tx, dns_tx),
            events::log_events(events_rx),
            dns::keep_records(paths, dns_rx, &published, &dns_ran),
            write_status(paths, &stats, &published, &dns_ran),
        )
    };
    tokio::select! {
        () = router => {}
        _ = background => {}
    }
    Ok(())
}

async fn write_status(
    paths: &Paths,
    stats: &Stats,
    published: &RefCell<Published>,
    dns_ran: &Notify,
) {
    let mut ticker = tokio::time::interval(STATUS_INTERVAL);
    let mut failing = false;
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            () = dns_ran.notified() => {}
        }
        let snapshot = {
            let published = published.borrow();
            StatusFile {
                updated: status_file::unix_now(),
                public_ip: published.public_ip,
                dns: published.dns.clone(),
                connections: stats.snapshot(),
            }
        };
        match status_file::write(paths, &snapshot) {
            Ok(()) => failing = false,
            Err(err) if !failing => {
                output::warn(format!("cannot write the status file: {err}"));
                failing = true;
            }
            Err(_) => {}
        }
    }
}
