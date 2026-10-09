use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::RejectReason;

const IPV6_PREFIX_64: u128 = u128::MAX << 64;

pub(crate) struct Limiter {
    per_client: u16,
    max_total: usize,
    counts: Mutex<Counts>,
}

#[derive(Default)]
struct Counts {
    total: usize,
    by_client: HashMap<IpAddr, u16>,
}

/// Holds one connection slot until dropped.
pub(crate) struct Permit {
    limiter: Arc<Limiter>,
    client: IpAddr,
}

impl Limiter {
    pub(crate) fn new(per_client: u16, max_total: usize) -> Self {
        Self {
            per_client,
            max_total,
            counts: Mutex::default(),
        }
    }

    pub(crate) fn acquire(self: &Arc<Self>, ip: IpAddr) -> Result<Permit, RejectReason> {
        let client = client_key(ip);
        let mut counts = self.lock();
        if counts.total >= self.max_total {
            return Err(RejectReason::TooManyConnections);
        }
        if counts.by_client.get(&client).copied().unwrap_or(0) >= self.per_client {
            return Err(RejectReason::TooManyFromIp);
        }
        *counts.by_client.entry(client).or_default() += 1;
        counts.total += 1;
        Ok(Permit {
            limiter: Arc::clone(self),
            client,
        })
    }

    fn release(&self, client: IpAddr) {
        let mut counts = self.lock();
        counts.total = counts.total.saturating_sub(1);
        if let Entry::Occupied(mut slot) = counts.by_client.entry(client) {
            let left = slot.get().saturating_sub(1);
            if left == 0 {
                slot.remove();
            } else {
                slot.insert(left);
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, Counts> {
        self.counts.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.limiter.release(self.client);
    }
}

fn client_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from_bits(v6.to_bits() & IPV6_PREFIX_64)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn rejects_client_at_its_limit() {
        let limiter = Arc::new(Limiter::new(2, 100));
        let _first = limiter.acquire(ip("203.0.113.7")).unwrap();
        let _second = limiter.acquire(ip("203.0.113.7")).unwrap();
        assert_eq!(
            limiter.acquire(ip("203.0.113.7")).err(),
            Some(RejectReason::TooManyFromIp)
        );
        assert!(limiter.acquire(ip("203.0.113.8")).is_ok());
    }

    #[test]
    fn frees_slot_when_permit_drops() {
        let limiter = Arc::new(Limiter::new(1, 100));
        drop(limiter.acquire(ip("203.0.113.7")).unwrap());
        let again = limiter.acquire(ip("203.0.113.7"));
        assert!(again.is_ok());
        drop(again);
        assert_eq!(limiter.lock().total, 0);
        assert!(limiter.lock().by_client.is_empty());
    }

    #[test]
    fn rejects_over_global_cap() {
        let limiter = Arc::new(Limiter::new(8, 2));
        let _first = limiter.acquire(ip("203.0.113.1")).unwrap();
        let _second = limiter.acquire(ip("203.0.113.2")).unwrap();
        assert_eq!(
            limiter.acquire(ip("203.0.113.3")).err(),
            Some(RejectReason::TooManyConnections)
        );
    }

    #[test]
    fn groups_clients_by_address_family_rules() {
        let cases = [
            ("203.0.113.7", "203.0.113.7"),
            ("::ffff:203.0.113.7", "203.0.113.7"),
            ("2001:db8:1:2:aaaa:bbbb:cccc:dddd", "2001:db8:1:2::"),
            ("2001:db8:1:2::1", "2001:db8:1:2::"),
            ("2001:db8:1:3::1", "2001:db8:1:3::"),
        ];
        for (raw, expected) in cases {
            assert_eq!(client_key(ip(raw)), ip(expected), "{raw}");
        }
    }

    #[test]
    fn counts_ipv6_neighbors_as_one_client() {
        let limiter = Arc::new(Limiter::new(1, 100));
        let _first = limiter.acquire(ip("2001:db8:1:2::1")).unwrap();
        assert_eq!(
            limiter.acquire(ip("2001:db8:1:2::2")).err(),
            Some(RejectReason::TooManyFromIp)
        );
        assert!(limiter.acquire(ip("2001:db8:1:3::1")).is_ok());
    }
}
