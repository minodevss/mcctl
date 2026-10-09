use std::collections::BTreeSet;
use std::pin::pin;
use std::sync::Arc;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::context::Context;
use crate::event::Events;
use crate::listeners::Listeners;
use crate::{Event, RouteTable, Settings, Stats, conn};

const ACCEPT_QUEUE: usize = 64;

/// Routes connections until `shutdown` resolves, then gives live sessions `shutdown_grace` before closing them.
/// Listeners follow every table sent on `routes`; routed sessions outlive reloads.
/// Bind and accept failures are reported as events, never returned.
pub async fn run(
    mut routes: watch::Receiver<Arc<RouteTable>>,
    settings: Settings,
    stats: Stats,
    events: mpsc::Sender<Event>,
    shutdown: impl Future<Output = ()>,
) {
    let grace = settings.shutdown_grace;
    let ctx = Arc::new(Context::new(
        settings,
        stats,
        Events::new(events),
        routes.clone(),
    ));
    let (accepted_tx, mut accepted_rx) = mpsc::channel(ACCEPT_QUEUE);
    let mut listeners = Listeners::new(Arc::clone(&ctx), accepted_tx);
    let mut sessions = JoinSet::new();
    let mut shutdown = pin!(shutdown);
    let mut watching = true;
    listeners.sync(&wanted_ports(&mut routes)).await;
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            changed = routes.changed(), if watching => match changed {
                Ok(()) => listeners.sync(&wanted_ports(&mut routes)).await,
                Err(_) => watching = false,
            },
            Some(accepted) = accepted_rx.recv() => {
                sessions.spawn(conn::serve(accepted, Arc::clone(&ctx)));
            }
            Some(_) = sessions.join_next(), if !sessions.is_empty() => {}
        }
    }
    listeners.close_all().await;
    drop(accepted_rx);
    let _ = timeout(grace, async {
        while sessions.join_next().await.is_some() {}
    })
    .await;
    sessions.shutdown().await;
}

fn wanted_ports(routes: &mut watch::Receiver<Arc<RouteTable>>) -> BTreeSet<u16> {
    routes.borrow_and_update().listen_ports()
}
