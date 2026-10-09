use std::sync::Arc;

use tokio::sync::watch;

use crate::event::Events;
use crate::limit::Limiter;
use crate::{RouteTable, Settings, Stats};

pub(crate) struct Context {
    pub(crate) settings: Settings,
    pub(crate) stats: Stats,
    pub(crate) events: Events,
    pub(crate) limiter: Arc<Limiter>,
    pub(crate) routes: watch::Receiver<Arc<RouteTable>>,
}

impl Context {
    pub(crate) fn new(
        settings: Settings,
        stats: Stats,
        events: Events,
        routes: watch::Receiver<Arc<RouteTable>>,
    ) -> Self {
        let limiter = Arc::new(Limiter::new(
            settings.per_ip_connections,
            settings.max_connections,
        ));
        Self {
            settings,
            stats,
            events,
            limiter,
            routes,
        }
    }
}
