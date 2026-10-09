use std::collections::BTreeSet;
use std::net::{Ipv4Addr, TcpListener};

use crate::config::Servers;
use crate::error::Error;
use crate::names::{INTERNAL_PORTS, InternalPort};

/// The lowest internal port no config uses and `bindable` accepts.
pub(crate) fn choose_internal_port(
    taken: &BTreeSet<u16>,
    mut bindable: impl FnMut(u16) -> bool,
) -> Option<u16> {
    INTERNAL_PORTS
        .filter(|port| !taken.contains(port))
        .find(|port| bindable(*port))
}

pub(crate) fn bindable_on_loopback(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

pub(crate) fn allocate(servers: &Servers) -> Result<InternalPort, Error> {
    let taken = servers
        .values()
        .map(|config| config.internal_port.get())
        .collect();
    let port = choose_internal_port(&taken, bindable_on_loopback).ok_or(Error::NoFreePort)?;
    InternalPort::try_from(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_lowest_free_internal_port() {
        let cases: [(&[u16], &[u16], Option<u16>); 4] = [
            (&[], &[], Some(25600)),
            (&[25600, 25601], &[], Some(25602)),
            (&[25601], &[], Some(25600)),
            (&[25600], &[25601, 25602], Some(25603)),
        ];
        for (taken, busy, expected) in cases {
            let taken = taken.iter().copied().collect();
            let chosen = choose_internal_port(&taken, |port| !busy.contains(&port));
            assert_eq!(chosen, expected, "{taken:?} {busy:?}");
        }
        let all: BTreeSet<u16> = INTERNAL_PORTS.collect();
        assert_eq!(choose_internal_port(&all, |_| true), None);
        assert_eq!(choose_internal_port(&BTreeSet::new(), |_| false), None);
    }
}
