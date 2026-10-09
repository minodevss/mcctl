#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "helpers fail the calling test on any unexpected outcome"
)]

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use mcctl_protocol::{Frame, Parse, parse_frame, read_varint, write_varint};
use mcctl_router::{Backend, Event, RouteTable, Settings, Stats};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

pub(crate) const WAIT: Duration = Duration::from_secs(5);
pub(crate) const PROTOCOL: i32 = 772;
pub(crate) const STATUS: i32 = 1;
pub(crate) const LOGIN: i32 = 2;

pub(crate) struct Router {
    tables: watch::Sender<Arc<RouteTable>>,
    events: mpsc::Receiver<Event>,
    pub(crate) stats: Stats,
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl Router {
    pub(crate) async fn start(table: RouteTable, settings: Settings) -> Self {
        let ports = table.listen_ports();
        let (tables, tables_rx) = watch::channel(Arc::new(table));
        let (events_tx, events) = mpsc::channel(256);
        let stats = Stats::default();
        let (stop, stop_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(mcctl_router::run(
            tables_rx,
            settings,
            stats.clone(),
            events_tx,
            async {
                let _ = stop_rx.await;
            },
        ));
        let mut router = Self {
            tables,
            events,
            stats,
            stop,
            task,
        };
        router.wait_listening(ports).await;
        router
    }

    pub(crate) fn reload(&self, table: RouteTable) {
        self.tables.send_replace(Arc::new(table));
    }

    pub(crate) async fn wait_for(&mut self, wanted: impl Fn(&Event) -> bool) -> Event {
        timeout(WAIT, async {
            loop {
                let event = self.events.recv().await.expect("router stopped");
                if wanted(&event) {
                    return event;
                }
            }
        })
        .await
        .expect("event did not arrive")
    }

    pub(crate) async fn wait_listening(&mut self, mut ports: BTreeSet<u16>) {
        while !ports.is_empty() {
            match self
                .wait_for(|event| {
                    matches!(event, Event::Listening { .. } | Event::BindFailed { .. })
                })
                .await
            {
                Event::Listening { port } => ports.remove(&port),
                other => panic!("router could not listen: {other:?}"),
            };
        }
    }

    pub(crate) async fn stop(self) {
        let _ = self.stop.send(());
        timeout(WAIT, self.task)
            .await
            .expect("router did not stop")
            .expect("router task failed");
    }
}

pub(crate) fn settings() -> Settings {
    Settings {
        listen_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        shutdown_grace: Duration::from_millis(100),
        ..Settings::default()
    }
}

pub(crate) fn free_port() -> u16 {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub(crate) async fn backend(server: &str) -> (TcpListener, Backend) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let backend = Backend {
        server: server.to_owned(),
        addr,
    };
    (listener, backend)
}

pub(crate) fn dead_backend(server: &str) -> Backend {
    Backend {
        server: server.to_owned(),
        addr: SocketAddr::from((Ipv4Addr::LOCALHOST, free_port())),
    }
}

pub(crate) async fn connect(port: u16) -> TcpStream {
    TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap()
}

pub(crate) async fn accept(listener: &TcpListener) -> TcpStream {
    let (stream, _) = timeout(WAIT, listener.accept())
        .await
        .expect("backend got no connection")
        .unwrap();
    stream
}

pub(crate) async fn assert_no_connection(listener: &TcpListener) {
    let accepted = timeout(Duration::from_millis(100), listener.accept()).await;
    assert!(accepted.is_err(), "backend got an unexpected connection");
}

pub(crate) async fn read_exactly(stream: &mut TcpStream, len: usize) -> Vec<u8> {
    let mut buf = vec![0; len];
    timeout(WAIT, stream.read_exact(&mut buf))
        .await
        .expect("bytes did not arrive")
        .unwrap();
    buf
}

pub(crate) async fn assert_closed(stream: &mut TcpStream) {
    let mut buf = [0; 64];
    let read = timeout(WAIT, stream.read(&mut buf))
        .await
        .expect("connection stayed open");
    assert!(matches!(read, Ok(0) | Err(_)), "unexpected data: {read:?}");
}

pub(crate) async fn read_frame(stream: &mut TcpStream) -> Frame {
    let mut buf = Vec::new();
    timeout(WAIT, async {
        loop {
            if let Parse::Done { value, consumed } = parse_frame(&buf, usize::MAX) {
                assert_eq!(consumed, buf.len(), "more than one frame arrived");
                return value;
            }
            let mut chunk = [0; 1024];
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0, "closed before a full frame");
            buf.extend_from_slice(&chunk[..read]);
        }
    })
    .await
    .expect("frame did not arrive")
}

pub(crate) fn string_field(body: &[u8]) -> String {
    let Parse::Done { value, consumed } = read_varint(body) else {
        panic!("no string length in {body:02x?}");
    };
    let text = &body[consumed..];
    assert_eq!(usize::try_from(value).unwrap(), text.len());
    String::from_utf8(text.to_vec()).unwrap()
}

pub(crate) async fn eventually(condition: impl Fn() -> bool) {
    timeout(WAIT, async {
        while !condition() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("condition never held");
}

pub(crate) fn framed(packet: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    write_varint(i32::try_from(packet.len()).unwrap(), &mut frame);
    frame.extend_from_slice(packet);
    frame
}

pub(crate) fn handshake(host: &str, port: u16, intent: i32) -> Vec<u8> {
    let mut packet = vec![0x00];
    write_varint(PROTOCOL, &mut packet);
    write_varint(i32::try_from(host.len()).unwrap(), &mut packet);
    packet.extend_from_slice(host.as_bytes());
    packet.extend_from_slice(&port.to_be_bytes());
    write_varint(intent, &mut packet);
    framed(&packet)
}

pub(crate) fn login_start(name: &str) -> Vec<u8> {
    let mut packet = vec![0x00];
    write_varint(i32::try_from(name.len()).unwrap(), &mut packet);
    packet.extend_from_slice(name.as_bytes());
    packet.extend_from_slice(&[0x5A; 16]);
    framed(&packet)
}
