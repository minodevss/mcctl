use std::fmt;
use std::net::IpAddr;

use mcctl_protocol::Invalid;
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Listening {
        port: u16,
    },
    StoppedListening {
        port: u16,
    },
    /// The port stays closed until the next route change retries it.
    BindFailed {
        port: u16,
        error: String,
    },
    AcceptFailed {
        port: u16,
        error: String,
    },
    Rejected {
        ip: IpAddr,
        reason: RejectReason,
    },
    Offline {
        server: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    TooManyFromIp,
    TooManyConnections,
    HandshakeTimeout,
    BadHandshake(Invalid),
    /// `host` is normalized but otherwise client-controlled; escape it before printing.
    UnknownHost {
        host: String,
    },
}

impl fmt::Display for RejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyFromIp => f.write_str("too many connections from this address"),
            Self::TooManyConnections => f.write_str("too many connections"),
            Self::HandshakeTimeout => f.write_str("no handshake in time"),
            Self::BadHandshake(invalid) => write!(f, "bad handshake: {invalid}"),
            Self::UnknownHost { host } => write!(f, "unknown host {host:?}"),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Events(mpsc::Sender<Event>);

impl Events {
    pub(crate) fn new(sender: mpsc::Sender<Event>) -> Self {
        Self(sender)
    }

    pub(crate) fn emit(&self, event: Event) {
        // Drop the event when the channel is full: the data path never waits on logging.
        let _ = self.0.try_send(event);
    }
}
