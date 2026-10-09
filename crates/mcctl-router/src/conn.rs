use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use mcctl_protocol::{
    Handshake, Intent, Invalid, MAX_FRAME_LEN, Parse, StatusPacket, login_disconnect,
    normalize_host, parse_handshake, parse_status_packet, pong, status_response,
};
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::context::Context;
use crate::limit::Permit;
use crate::{Event, RejectReason, Settings, Stats};

const CLOSE_LINGER: Duration = Duration::from_secs(1);

pub(crate) struct Accepted {
    pub(crate) stream: TcpStream,
    pub(crate) ip: IpAddr,
    pub(crate) port: u16,
    pub(crate) permit: Permit,
}

struct Greeting {
    handshake: Handshake,
    bytes: Vec<u8>,
    frame_len: usize,
}

enum ReadError {
    Closed,
    Invalid(Invalid),
}

enum Finished {
    ClientToBackend,
    BackendToClient,
}

pub(crate) async fn serve(accepted: Accepted, ctx: Arc<Context>) {
    let Accepted {
        mut stream,
        ip,
        port,
        permit: _permit,
    } = accepted;
    let _ = stream.set_nodelay(true);
    let greeting = match timeout(ctx.settings.handshake_timeout, read_greeting(&mut stream)).await {
        Ok(Ok(greeting)) => greeting,
        Ok(Err(ReadError::Closed)) => return,
        Ok(Err(ReadError::Invalid(invalid))) => {
            reject(&ctx, ip, RejectReason::BadHandshake(invalid));
            return;
        }
        Err(_) => {
            reject(&ctx, ip, RejectReason::HandshakeTimeout);
            return;
        }
    };
    let routes = Arc::clone(&ctx.routes.borrow());
    let Some(backend) = routes.route(port, &greeting.handshake.host) else {
        let host = normalize_host(&greeting.handshake.host);
        reject(&ctx, ip, RejectReason::UnknownHost { host });
        return;
    };
    let Some(upstream) = connect(backend.addr, ctx.settings.connect_timeout).await else {
        ctx.events.emit(Event::Offline {
            server: backend.server.clone(),
        });
        answer_offline(stream, greeting, &backend.server, &ctx.settings).await;
        return;
    };
    relay(
        stream,
        upstream,
        &greeting.bytes,
        &ctx.stats,
        &backend.server,
    )
    .await;
}

fn reject(ctx: &Context, ip: IpAddr, reason: RejectReason) {
    ctx.events.emit(Event::Rejected { ip, reason });
}

async fn read_greeting(stream: &mut TcpStream) -> Result<Greeting, ReadError> {
    let mut bytes = Vec::with_capacity(MAX_FRAME_LEN);
    let (handshake, frame_len) = read_packet(stream, &mut bytes, parse_handshake).await?;
    Ok(Greeting {
        handshake,
        bytes,
        frame_len,
    })
}

// The parsers reject declared lengths over MAX_FRAME_LEN, so `buf` stays below that plus one read.
async fn read_packet<T>(
    stream: &mut TcpStream,
    buf: &mut Vec<u8>,
    parse: impl Fn(&[u8]) -> Parse<T>,
) -> Result<(T, usize), ReadError> {
    let mut chunk = [0; MAX_FRAME_LEN];
    loop {
        match parse(buf) {
            Parse::Done { value, consumed } => return Ok((value, consumed)),
            Parse::Invalid(invalid) => return Err(ReadError::Invalid(invalid)),
            Parse::Incomplete => {}
        }
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return Err(ReadError::Closed),
            Ok(read) => buf.extend_from_slice(&chunk[..read]),
        }
    }
}

async fn connect(addr: SocketAddr, limit: Duration) -> Option<TcpStream> {
    let stream = timeout(limit, TcpStream::connect(addr)).await.ok()?.ok()?;
    let _ = stream.set_nodelay(true);
    Some(stream)
}

async fn relay(
    mut client: TcpStream,
    mut backend: TcpStream,
    greeting: &[u8],
    stats: &Stats,
    server: &str,
) {
    if backend.write_all(greeting).await.is_err() {
        return;
    }
    let finished = {
        let _live = stats.track(server);
        pipe(&mut client, &mut backend).await
    };
    drop(backend);
    match finished {
        Finished::ClientToBackend => {}
        Finished::BackendToClient => close_gracefully(client).await,
    }
}

async fn pipe(client: &mut TcpStream, backend: &mut TcpStream) -> Finished {
    let (mut client_read, mut client_write) = client.split();
    let (mut backend_read, mut backend_write) = backend.split();
    // End on the first finished direction; waiting for both would keep half-dead sessions open.
    tokio::select! {
        _ = io::copy(&mut client_read, &mut backend_write) => Finished::ClientToBackend,
        _ = io::copy(&mut backend_read, &mut client_write) => Finished::BackendToClient,
    }
}

async fn answer_offline(
    mut client: TcpStream,
    greeting: Greeting,
    server: &str,
    settings: &Settings,
) {
    let Greeting {
        handshake,
        mut bytes,
        frame_len,
    } = greeting;
    match handshake.intent {
        Intent::Status => {
            bytes.drain(..frame_len);
            let description = settings.offline_status_for(server);
            let exchange = answer_status(&mut client, bytes, handshake.protocol, &description);
            let _ = timeout(settings.handshake_timeout, exchange).await;
        }
        Intent::Login | Intent::Transfer => {
            let reason = login_disconnect(&settings.offline_login_for(server));
            let _ = timeout(settings.handshake_timeout, client.write_all(&reason)).await;
        }
    }
    close_gracefully(client).await;
}

async fn answer_status(client: &mut TcpStream, mut buf: Vec<u8>, protocol: i32, description: &str) {
    let mut answered = false;
    while let Ok((packet, consumed)) = read_packet(client, &mut buf, parse_status_packet).await {
        buf.drain(..consumed);
        match packet {
            StatusPacket::Request if !answered => {
                answered = true;
                let status = status_response(protocol, description);
                if client.write_all(&status).await.is_err() {
                    return;
                }
            }
            StatusPacket::Request => return,
            StatusPacket::Ping(payload) => {
                let _ = client.write_all(&pong(payload)).await;
                return;
            }
        }
    }
}

async fn close_gracefully(mut stream: TcpStream) {
    // Closing with unread input sends a reset, which can make the client drop our last message.
    if stream.shutdown().await.is_ok() {
        let _ = timeout(CLOSE_LINGER, io::copy(&mut stream, &mut io::sink())).await;
    }
}
