use std::{
    net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket},
    time::{Duration, Instant}
};

use linkseeker::data::{FromMiddlemanMsg, ToMiddlemanMsg};

fn punch(socket: &UdpSocket, remote: SocketAddr) {
    let mut buf = [0; 1500];
    let _r = socket.send_to(b"PUNCH", remote);
    println!("punching...");
    std::thread::sleep(Duration::from_secs(2));
    let _r = socket.send_to(b"PUNCH", remote);
    println!("punching...");
    std::thread::sleep(Duration::from_secs(2));
    let _r = socket.send_to(b"PUNCH", remote);
    println!("punching...");
    std::thread::sleep(Duration::from_secs(2));
    println!("waiting for punch...");
    let Ok((len, recv_remote)) = socket.recv_from(&mut buf) else {
        return;
    };
    if &buf[0..len] == b"PUNCH" && recv_remote == remote {
        println!("successfully punched to {}", remote);
    } else {
        eprintln!("error: received unexpected message {} from remote {}", String::from_utf8_lossy(&buf[0..len]), recv_remote);
    }
}

fn send_msg(msg: ToMiddlemanMsg, socket: &UdpSocket, remote: SocketAddr) {
    let b = msg.serialize();
    let _r = socket.send_to(&b, remote);
}

fn recv_msg(socket: &UdpSocket, remote: SocketAddr) -> Option<FromMiddlemanMsg> {
    let mut buf = [0u8; 1500];
    let Ok((len, recv_remote)) = socket.recv_from(&mut buf) else {
        return None;
    };
    if remote != recv_remote {
        return None;
    }
    FromMiddlemanMsg::parse(&buf[0..len])
}

fn ping_script(socket: &UdpSocket, listener_ip: SocketAddr) -> bool {
    println!("running ping script");
    let sent_id = 10;
    send_msg(ToMiddlemanMsg::Ping { id: sent_id }, socket, listener_ip);

    match recv_msg(socket, listener_ip) {
        Some(FromMiddlemanMsg::Pong { id }) => {
            if id == sent_id {
                println!("success!");
                true
            } else {
                eprintln!("received incorrect ping id {}", id);
                false
            }
        },
        e => {
            eprintln!("did not receive correct answer for register: {:?}", e);
            false
        }
    }
}

fn host_script(socket: &UdpSocket, listener_ip: SocketAddr) -> bool {
    println!("running host script");
    send_msg(ToMiddlemanMsg::Register, socket, listener_ip);

    let Some(FromMiddlemanMsg::RegisterOk { id }) = recv_msg(socket, listener_ip) else {
        eprintln!("did not receive correct answer for register");
        return false;
    };
    println!("successfully register, have id: {}", id);
    let Some(FromMiddlemanMsg::PunchOrder { remote, .. }) = recv_msg(socket, listener_ip) else {
        return false;
    };
    println!("got request to punch {}", remote);
    punch(socket, remote);
    true
}

fn client_script(udp_socket: &UdpSocket, listener_ip: SocketAddr, conn_id: u32) -> bool {
    println!("running client script, connecting to id: {}", conn_id);

    send_msg(ToMiddlemanMsg::Request { id: conn_id }, udp_socket, listener_ip);
    println!("sent request for id {} to {}", conn_id, listener_ip);
    let remote_addr;
    match recv_msg(udp_socket, listener_ip) {
        Some(FromMiddlemanMsg::PunchOrder { remote, .. }) => {
            remote_addr = remote;
        },
        e => {
            eprintln!("unexpected {:?}", e);
            return false;
        }
    };
    println!("got request to punch {}", remote_addr);
    punch(udp_socket, remote_addr);
    true
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let arg1 = args.next();
    let arg2 = args.next();
    let listener_ip = arg1.map_or_else(
        || "127.0.0.1:61999".to_socket_addrs().unwrap().next(),
        |arg1| arg1.to_socket_addrs().unwrap().next()
    ).unwrap();
    let conn_id = arg2.and_then(|v| v.parse::<u32>().ok());
    
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(None)?;


    match conn_id {
        None => {
            if !host_script(&socket, listener_ip) {
                eprintln!("error while running host script");
                return Ok(());
            }
        },
        Some(conn_id) => {
            if !client_script(&socket, listener_ip, conn_id) {
                eprintln!("error while running client script");
                return Ok(());
            }
        }
    };
    Ok(())
}

fn _main2() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let arg1 = args.next();
    let listener_ip = arg1.map_or_else(
        || "127.0.0.1:61999".to_socket_addrs().unwrap().next(),
        |arg1| arg1.to_socket_addrs().unwrap().next()
    ).unwrap();
    
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(None)?;

    ping_script(&socket, listener_ip);

    Ok(())
}