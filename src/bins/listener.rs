use std::{
    time::{Instant, Duration},
    net::{UdpSocket, SocketAddr, IpAddr}
};

use udpunch::tracker::PunchTracker;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arg1 = std::env::args().skip(1).next();
    let host_ip = arg1.map_or_else(
        || "0.0.0.0:61999".parse::<SocketAddr>(),
        |arg1| arg1.parse::<SocketAddr>()
    )?;
    
    let mut tracker = PunchTracker::new();

    let mut buf: [u8; 1500] = [0; 1500];
    let socket = UdpSocket::bind(host_ip)?;
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(None)?;
    let mut i: u64 = 0;

    println!("listening to {}", host_ip);
    loop {
        let Ok((len, remote)) = socket.recv_from(&mut buf) else {
            continue;
        };
        println!("received message \"{}\" from {}", String::from_utf8_lossy(&buf[0..len]), remote);
        tracker.process_incoming(&buf[0..len], remote);
        tracker.process_outgoing(|bytes, remote| {
            println!("sent message \"{}\" to {}", String::from_utf8_lossy(&bytes), remote);
            let _r = socket.send_to(&bytes, remote);
        });

        if i % 16 == 0 {
            tracker.cleanup();
        }
        i += 1;
    }
}