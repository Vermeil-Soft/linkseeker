#![cfg(feature = "tracker")]

use std::{
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

use linkseeker::{
    common::UDPUNCH_ID_BYTES,
    data::{FromMiddlemanMsg, ToMiddlemanMsg},
    tracker::LinkSeekTracker,
};

/// Starts a tracker in the background, returns the address of its first socket
fn start_tracker() -> SocketAddr {
    for _ in 0..100 {
        let port = 20000 + (rand::random::<u16>() % 30000);
        if let Ok(mut tracker) = LinkSeekTracker::new(port) {
            std::thread::spawn(move || {
                let _ = tracker.run();
            });
            return ([127, 0, 0, 1], port).into();
        }
    }
    panic!("could not find free ports for the tracker");
}

fn client(ip: [u8; 4]) -> UdpSocket {
    let socket = UdpSocket::bind(SocketAddr::from((ip, 0))).unwrap();
    socket.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    socket
}

fn send(socket: &UdpSocket, msg: ToMiddlemanMsg, to: SocketAddr) {
    socket.send_to(&msg.serialize(), to).unwrap();
}

fn second_port(addr: SocketAddr) -> SocketAddr {
    (addr.ip(), addr.port() + 1).into()
}

/// Waits for a linkseeker message matching `f` (messages are sent twice, so others are skipped)
fn recv_msg(socket: &UdpSocket, f: impl Fn(&FromMiddlemanMsg) -> bool) -> FromMiddlemanMsg {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut buf = [0u8; 1500];
    while Instant::now() < deadline {
        let Ok((len, _)) = socket.recv_from(&mut buf) else { continue };
        if let Some(msg) = FromMiddlemanMsg::parse(&buf[..len]) {
            if f(&msg) {
                return msg;
            }
        }
    }
    panic!("did not receive the expected message");
}

/// Waits for a non-linkseeker (proxied) packet
fn recv_raw(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut buf = [0u8; 1500];
    while Instant::now() < deadline {
        let Ok((len, from)) = socket.recv_from(&mut buf) else { continue };
        if !buf[..len].starts_with(UDPUNCH_ID_BYTES) {
            return (buf[..len].to_vec(), from);
        }
    }
    panic!("did not receive the expected raw packet");
}

fn register(socket: &UdpSocket, tracker: SocketAddr, use_proxy: bool) -> u32 {
    send(socket, ToMiddlemanMsg::Register { use_proxy }, tracker);
    match recv_msg(socket, |m| matches!(m, FromMiddlemanMsg::RegisterOk { .. })) {
        FromMiddlemanMsg::RegisterOk { id } => id,
        _ => unreachable!(),
    }
}

#[test]
fn register_twice_keeps_id_and_ping() {
    let tracker = start_tracker();
    let host = client([127, 0, 0, 1]);
    let id = register(&host, tracker, false);
    assert_eq!(register(&host, tracker, false), id);

    send(&host, ToMiddlemanMsg::Ping { id: 42 }, tracker);
    assert_eq!(recv_msg(&host, |m| matches!(m, FromMiddlemanMsg::Pong { .. })), FromMiddlemanMsg::Pong { id: 42 });
}

#[test]
fn request_unknown_host() {
    let tracker = start_tracker();
    let c = client([127, 0, 0, 1]);
    send(&c, ToMiddlemanMsg::Request { id: 1234, use_proxy: true, dh_id: Some(1) }, tracker);
    recv_msg(&c, |m| matches!(m, FromMiddlemanMsg::RequestErr { .. }));
}

#[test]
fn proxy_relays_both_ways() {
    let tracker = start_tracker();
    let host = client([127, 0, 0, 1]);
    let id = register(&host, tracker, true);

    let connector = client([127, 0, 0, 1]);
    let dh_id: u32 = 0xAABBCCDD;
    send(&connector, ToMiddlemanMsg::Request { id, use_proxy: true, dh_id: Some(dh_id) }, tracker);
    assert_eq!(
        recv_msg(&connector, |m| matches!(m, FromMiddlemanMsg::RequestOk { .. })),
        FromMiddlemanMsg::RequestOk { id, use_proxy: true }
    );

    // sent during the proxy delay: must be held back, not dropped
    let to_host = b"\x01\x02\x03\x04\x05\x06\x07\x08\x11\x22\x33\x44payload-c";
    connector.send_to(to_host, tracker).unwrap();
    let (bytes, from) = recv_raw(&host);
    assert_eq!(bytes, to_host);
    assert_eq!(from, tracker);

    let mut to_connector = b"\x01\x02\x03\x04\x05\x06\x07\x08\0\0\0\0payload-h".to_vec();
    to_connector[8..12].copy_from_slice(&dh_id.to_be_bytes());
    host.send_to(&to_connector, tracker).unwrap();
    let (bytes, from) = recv_raw(&connector);
    assert_eq!(bytes, to_connector);
    assert_eq!(from, tracker);

    // the host's packets with another dh_id are not for this connector
    let mut wrong = to_connector.clone();
    wrong[8..12].copy_from_slice(&1u32.to_be_bytes());
    host.send_to(&wrong, tracker).unwrap();
    connector.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    let mut buf = [0u8; 1500];
    assert!(connector.recv_from(&mut buf).is_err());
}

#[test]
fn punch_order_between_different_ips() {
    let tracker = start_tracker();
    let host = client([127, 0, 0, 2]);
    let id = register(&host, tracker, false);

    let connector = client([127, 0, 0, 1]);
    send(&connector, ToMiddlemanMsg::Request { id, use_proxy: false, dh_id: None }, tracker);
    assert_eq!(
        recv_msg(&connector, |m| matches!(m, FromMiddlemanMsg::RequestOk { .. })),
        FromMiddlemanMsg::RequestOk { id, use_proxy: false }
    );
    assert_eq!(
        recv_msg(&connector, |m| matches!(m, FromMiddlemanMsg::PunchOrder { .. })),
        FromMiddlemanMsg::PunchOrder { remote: host.local_addr().unwrap() }
    );
    assert_eq!(
        recv_msg(&host, |m| matches!(m, FromMiddlemanMsg::PunchOrder { .. })),
        FromMiddlemanMsg::PunchOrder { remote: connector.local_addr().unwrap() }
    );
}

#[test]
fn punch_check_from_same_socket() {
    let tracker = start_tracker();
    let c = client([127, 0, 0, 1]);
    send(&c, ToMiddlemanMsg::PunchCheck { id: 77 }, tracker);
    std::thread::sleep(Duration::from_millis(50));
    send(&c, ToMiddlemanMsg::PunchCheck { id: 77 }, second_port(tracker));
    assert_eq!(
        recv_msg(&c, |m| matches!(m, FromMiddlemanMsg::PunchCheckResult { .. })),
        FromMiddlemanMsg::PunchCheckResult { ok: true }
    );
}

/// Returns whether any linkseeker message is received within `wait`
fn receives_anything(socket: &UdpSocket, wait: Duration) -> bool {
    socket.set_read_timeout(Some(wait)).unwrap();
    let mut buf = [0u8; 1500];
    socket.recv_from(&mut buf).is_ok()
}

#[test]
fn domain_name_request() {
    let tracker = start_tracker();
    let c = client([127, 0, 0, 1]);
    // the domain is resolved in the background, so the first requests may be ignored: retry like clients do
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        assert!(Instant::now() < deadline, "did not receive the DNS result");
        send(&c, ToMiddlemanMsg::DomainNameReq { domain: "LocalHost:1234".into() }, tracker);
        if receives_anything(&c, Duration::from_millis(100)) {
            break;
        }
    }
    send(&c, ToMiddlemanMsg::DomainNameReq { domain: "localhost:1234".into() }, tracker);
    let FromMiddlemanMsg::DomainNameResult { domain, results } =
        recv_msg(&c, |m| matches!(m, FromMiddlemanMsg::DomainNameResult { domain, .. } if domain == "localhost:1234")) else { unreachable!() };
    assert_eq!(domain, "localhost:1234");
    assert!(results.contains(&([127, 0, 0, 1], 1234).into()), "{:?}", results);
}

#[test]
fn other_domains_are_ignored() {
    let tracker = start_tracker();
    let c = client([127, 0, 0, 1]);
    for domain in ["example.com:80", "localhost", "localhost.evil.com:80", "localhost:notaport"] {
        send(&c, ToMiddlemanMsg::DomainNameReq { domain: domain.into() }, tracker);
    }
    assert!(!receives_anything(&c, Duration::from_millis(500)));
}

#[test]
fn proxy_to_is_ignored() {
    let tracker = start_tracker();
    let c = client([127, 0, 0, 1]);
    send(&c, ToMiddlemanMsg::ProxyTo { remote: ([127, 0, 0, 1], 9).into(), dh_id: 1 }, tracker);
    assert!(!receives_anything(&c, Duration::from_millis(500)));
}
