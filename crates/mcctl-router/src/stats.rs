use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

#[derive(Debug, Clone, Default)]
pub struct Stats {
    live: Arc<Mutex<BTreeMap<String, usize>>>,
}

impl Stats {
    /// Live routed sessions per server; servers without sessions are absent.
    pub fn snapshot(&self) -> BTreeMap<String, usize> {
        self.lock().clone()
    }

    pub(crate) fn track(&self, server: &str) -> LiveSession {
        *self.lock().entry(server.to_owned()).or_default() += 1;
        LiveSession {
            stats: self.clone(),
            server: server.to_owned(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, usize>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub(crate) struct LiveSession {
    stats: Stats,
    server: String,
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        let mut live = self.stats.lock();
        if let Some(count) = live.get_mut(&self.server) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                live.remove(&self.server);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_live_sessions_per_server() {
        let stats = Stats::default();
        let first = stats.track("survival");
        let _second = stats.track("survival");
        let _third = stats.track("creative");
        assert_eq!(
            stats.snapshot(),
            BTreeMap::from([("creative".to_owned(), 1), ("survival".to_owned(), 2)])
        );
        drop(first);
        assert_eq!(stats.snapshot().get("survival"), Some(&1));
    }

    #[test]
    fn forgets_server_when_last_session_ends() {
        let stats = Stats::default();
        drop(stats.track("survival"));
        assert!(stats.snapshot().is_empty());
    }
}
