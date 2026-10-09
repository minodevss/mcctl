use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::Event;
use crate::conn::Accepted;
use crate::context::Context;

const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

pub(crate) struct Listeners {
    ctx: Arc<Context>,
    accepted: mpsc::Sender<Accepted>,
    running: BTreeMap<u16, AcceptLoop>,
}

struct AcceptLoop(JoinHandle<()>);

impl Listeners {
    pub(crate) fn new(ctx: Arc<Context>, accepted: mpsc::Sender<Accepted>) -> Self {
        Self {
            ctx,
            accepted,
            running: BTreeMap::new(),
        }
    }

    /// Closes ports no longer wanted and binds wanted ports not yet open, including ones that failed before.
    pub(crate) async fn sync(&mut self, wanted: &BTreeSet<u16>) {
        let unwanted: Vec<u16> = self
            .running
            .keys()
            .copied()
            .filter(|port| !wanted.contains(port))
            .collect();
        for port in unwanted {
            self.close(port).await;
        }
        for &port in wanted {
            if !self.running.contains_key(&port) {
                self.open(port).await;
            }
        }
    }

    pub(crate) async fn close_all(&mut self) {
        let ports: Vec<u16> = self.running.keys().copied().collect();
        for port in ports {
            self.close(port).await;
        }
    }

    async fn open(&mut self, port: u16) {
        let addr = SocketAddr::new(self.ctx.settings.listen_ip, port);
        match TcpListener::bind(addr).await {
            Ok(listener) => {
                let task = tokio::spawn(accept_loop(
                    listener,
                    port,
                    Arc::clone(&self.ctx),
                    self.accepted.clone(),
                ));
                self.running.insert(port, AcceptLoop(task));
                self.ctx.events.emit(Event::Listening { port });
            }
            Err(error) => self.ctx.events.emit(Event::BindFailed {
                port,
                error: error.to_string(),
            }),
        }
    }

    async fn close(&mut self, port: u16) {
        if let Some(accept_loop) = self.running.remove(&port) {
            accept_loop.stop().await;
            self.ctx.events.emit(Event::StoppedListening { port });
        }
    }
}

impl AcceptLoop {
    // Wait for the aborted task so its socket is closed before the port can be bound again.
    async fn stop(mut self) {
        self.0.abort();
        let _ = (&mut self.0).await;
    }
}

impl Drop for AcceptLoop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn accept_loop(
    listener: TcpListener,
    port: u16,
    ctx: Arc<Context>,
    accepted: mpsc::Sender<Accepted>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let ip = peer.ip().to_canonical();
                match ctx.limiter.acquire(ip) {
                    Ok(permit) => {
                        let connection = Accepted {
                            stream,
                            ip,
                            port,
                            permit,
                        };
                        if accepted.send(connection).await.is_err() {
                            return;
                        }
                    }
                    Err(reason) => ctx.events.emit(Event::Rejected { ip, reason }),
                }
            }
            Err(error) => {
                ctx.events.emit(Event::AcceptFailed {
                    port,
                    error: error.to_string(),
                });
                // Accept keeps failing while file descriptors are exhausted; pause instead of spinning.
                sleep(ACCEPT_BACKOFF).await;
            }
        }
    }
}
