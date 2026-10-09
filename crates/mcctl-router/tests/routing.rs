mod support;

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use mcctl_router::{Event, RejectReason, RouteTable, Settings};
use support::{
    LOGIN, PROTOCOL, Router, STATUS, accept, assert_closed, assert_no_connection, backend, connect,
    dead_backend, eventually, framed, free_port, handshake, login_start, read_exactly, read_frame,
    settings, string_field,
};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::sleep;

#[tokio::test]
async fn routes_login_to_backend_by_host() {
    let (alpha_listener, alpha) = backend("alpha").await;
    let (beta_listener, beta) = backend("beta").await;
    let main = free_port();
    let mut table = RouteTable::new(main);
    table.add_host("alpha.test", alpha).unwrap();
    table.add_host("beta.test", beta).unwrap();
    let router = Router::start(table, settings()).await;

    let hello = handshake("Beta.Test.", main, LOGIN);
    let mut client = connect(main).await;
    client.write_all(&hello).await.unwrap();
    let mut upstream = accept(&beta_listener).await;
    assert_eq!(read_exactly(&mut upstream, hello.len()).await, hello);
    upstream.write_all(b"welcome").await.unwrap();
    assert_eq!(read_exactly(&mut client, 7).await, b"welcome");
    assert_no_connection(&alpha_listener).await;

    router.stop().await;
}

#[tokio::test]
async fn dedicated_port_ignores_host() {
    let (listener, modded) = backend("modded").await;
    let main = free_port();
    let dedicated = free_port();
    let mut table = RouteTable::new(main);
    table.add_port(dedicated, modded).unwrap();
    let router = Router::start(table, settings()).await;

    let hello = handshake("anything.invalid", dedicated, LOGIN);
    let mut client = connect(dedicated).await;
    client.write_all(&hello).await.unwrap();
    let mut upstream = accept(&listener).await;
    assert_eq!(read_exactly(&mut upstream, hello.len()).await, hello);

    router.stop().await;
}

#[tokio::test]
async fn forwards_bytes_exactly_including_login_start() {
    let (listener, survival) = backend("survival").await;
    let main = free_port();
    let mut table = RouteTable::new(main);
    table.add_host("survival.test", survival).unwrap();
    let router = Router::start(table, settings()).await;

    let hello = handshake("Survival.Test\0FML3\0", main, LOGIN);
    let greeting = [hello.as_slice(), login_start("Steve").as_slice()].concat();
    let (head, tail) = greeting.split_at(3);
    let mut client = connect(main).await;
    client.write_all(head).await.unwrap();
    sleep(Duration::from_millis(20)).await;
    client.write_all(tail).await.unwrap();
    let mut upstream = accept(&listener).await;
    let later = framed(&[0x03, 0x01, 0x02, 0x03]);
    client.write_all(&later).await.unwrap();

    let expected = [greeting.as_slice(), later.as_slice()].concat();
    assert_eq!(read_exactly(&mut upstream, expected.len()).await, expected);

    router.stop().await;
}

#[tokio::test]
async fn closes_unknown_host() {
    let (listener, survival) = backend("survival").await;
    let main = free_port();
    let mut table = RouteTable::new(main);
    table.add_host("survival.test", survival).unwrap();
    let mut router = Router::start(table, settings()).await;

    let mut client = connect(main).await;
    client
        .write_all(&handshake("Nobody.Test", main, LOGIN))
        .await
        .unwrap();
    assert_closed(&mut client).await;
    let rejected = router
        .wait_for(|event| matches!(event, Event::Rejected { .. }))
        .await;
    assert_eq!(
        rejected,
        Event::Rejected {
            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            reason: RejectReason::UnknownHost {
                host: "nobody.test".to_owned()
            },
        }
    );
    assert_no_connection(&listener).await;

    router.stop().await;
}

#[tokio::test]
async fn answers_offline_status_when_backend_down() {
    let main = free_port();
    let mut table = RouteTable::new(main);
    table
        .add_host("survival.test", dead_backend("survival"))
        .unwrap();
    let mut router = Router::start(table, settings()).await;

    let mut client = connect(main).await;
    let status_request = framed(&[0x00]);
    let opening = [handshake("survival.test", main, STATUS), status_request].concat();
    client.write_all(&opening).await.unwrap();
    let status = read_frame(&mut client).await;
    assert_eq!(status.id, 0x00);
    assert_eq!(
        string_field(&status.body),
        format!(
            r#"{{"version":{{"name":"mcctl","protocol":{PROTOCOL}}},"players":{{"max":0,"online":0}},"description":{{"text":"survival is offline"}}}}"#
        )
    );

    let payload = [9, 8, 7, 6, 5, 4, 3, 2];
    client
        .write_all(&framed(&[[0x01].as_slice(), &payload].concat()))
        .await
        .unwrap();
    let pong = read_frame(&mut client).await;
    assert_eq!((pong.id, pong.body.as_slice()), (0x01, payload.as_slice()));
    assert_closed(&mut client).await;
    router
        .wait_for(|event| matches!(event, Event::Offline { server } if server == "survival"))
        .await;

    router.stop().await;
}

#[tokio::test]
async fn sends_disconnect_on_login_when_backend_down() {
    let main = free_port();
    let mut table = RouteTable::new(main);
    table
        .add_host("survival.test", dead_backend("survival"))
        .unwrap();
    let mut router = Router::start(table, settings()).await;

    let mut client = connect(main).await;
    let opening = [
        handshake("survival.test", main, LOGIN),
        login_start("Steve"),
    ]
    .concat();
    client.write_all(&opening).await.unwrap();
    let disconnect = read_frame(&mut client).await;
    assert_eq!(disconnect.id, 0x00);
    assert_eq!(
        string_field(&disconnect.body),
        r#"{"text":"survival is offline. Try again later."}"#
    );
    let mut rest = [0; 8];
    let read = tokio::io::AsyncReadExt::read(&mut client, &mut rest).await;
    assert_eq!(read.unwrap(), 0, "expected a clean close, not a reset");
    router
        .wait_for(|event| matches!(event, Event::Offline { server } if server == "survival"))
        .await;

    router.stop().await;
}

#[tokio::test]
async fn rejects_over_per_ip_limit() {
    let (listener, survival) = backend("survival").await;
    let main = free_port();
    let mut table = RouteTable::new(main);
    table.add_host("survival.test", survival).unwrap();
    let limited = Settings {
        per_ip_connections: 2,
        ..settings()
    };
    let mut router = Router::start(table, limited).await;

    let mut first = connect(main).await;
    let _second = connect(main).await;
    let mut third = connect(main).await;
    let rejected = router
        .wait_for(|event| matches!(event, Event::Rejected { .. }))
        .await;
    assert_eq!(
        rejected,
        Event::Rejected {
            ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            reason: RejectReason::TooManyFromIp,
        }
    );
    assert_closed(&mut third).await;

    let hello = handshake("survival.test", main, LOGIN);
    first.write_all(&hello).await.unwrap();
    let mut upstream = accept(&listener).await;
    assert_eq!(read_exactly(&mut upstream, hello.len()).await, hello);

    router.stop().await;
}

#[tokio::test]
async fn reload_adds_and_removes_listeners() {
    let (listener, survival) = backend("survival").await;
    let main = free_port();
    let old_port = free_port();
    let new_port = free_port();
    let mut before = RouteTable::new(main);
    before.add_port(old_port, survival.clone()).unwrap();
    let mut router = Router::start(before, settings()).await;

    let mut kept = connect(old_port).await;
    let kept_hello = handshake("survival.test", old_port, LOGIN);
    kept.write_all(&kept_hello).await.unwrap();
    let mut kept_upstream = accept(&listener).await;
    assert_eq!(
        read_exactly(&mut kept_upstream, kept_hello.len()).await,
        kept_hello
    );

    let mut after = RouteTable::new(main);
    after.add_port(new_port, survival).unwrap();
    router.reload(after);
    router
        .wait_for(|event| *event == Event::StoppedListening { port: old_port })
        .await;
    router
        .wait_for(|event| *event == Event::Listening { port: new_port })
        .await;

    assert!(
        TcpStream::connect((Ipv4Addr::LOCALHOST, old_port))
            .await
            .is_err()
    );
    let hello = handshake("survival.test", new_port, LOGIN);
    let mut client = connect(new_port).await;
    client.write_all(&hello).await.unwrap();
    let mut upstream = accept(&listener).await;
    assert_eq!(read_exactly(&mut upstream, hello.len()).await, hello);

    kept.write_all(b"still here").await.unwrap();
    assert_eq!(read_exactly(&mut kept_upstream, 10).await, b"still here");
    kept_upstream.write_all(b"yes").await.unwrap();
    assert_eq!(read_exactly(&mut kept, 3).await, b"yes");

    router.stop().await;
}

#[tokio::test]
async fn session_ends_when_client_disconnects() {
    let (listener, survival) = backend("survival").await;
    let main = free_port();
    let mut table = RouteTable::new(main);
    table.add_host("survival.test", survival).unwrap();
    let router = Router::start(table, settings()).await;

    let hello = handshake("survival.test", main, LOGIN);
    let mut client = connect(main).await;
    client.write_all(&hello).await.unwrap();
    let mut upstream = accept(&listener).await;
    read_exactly(&mut upstream, hello.len()).await;
    let one_live = BTreeMap::from([("survival".to_owned(), 1)]);
    eventually(|| router.stats.snapshot() == one_live).await;

    drop(client);
    assert_closed(&mut upstream).await;
    eventually(|| router.stats.snapshot().is_empty()).await;

    router.stop().await;
}

#[tokio::test]
async fn shutdown_closes_sessions_after_grace() {
    let (listener, survival) = backend("survival").await;
    let main = free_port();
    let mut table = RouteTable::new(main);
    table.add_host("survival.test", survival).unwrap();
    let grace = Duration::from_millis(300);
    let router = Router::start(
        table,
        Settings {
            shutdown_grace: grace,
            ..settings()
        },
    )
    .await;

    let hello = handshake("survival.test", main, LOGIN);
    let mut client = connect(main).await;
    client.write_all(&hello).await.unwrap();
    let mut upstream = accept(&listener).await;
    read_exactly(&mut upstream, hello.len()).await;

    let started = Instant::now();
    router.stop().await;
    assert!(started.elapsed() >= grace, "sessions were cut before grace");
    assert_closed(&mut client).await;
    assert_closed(&mut upstream).await;
    assert!(
        TcpStream::connect((Ipv4Addr::LOCALHOST, main))
            .await
            .is_err()
    );
}
