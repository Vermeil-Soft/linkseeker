use std::{
    time::{Instant, Duration},
    net::{UdpSocket, SocketAddr, IpAddr}
};

use linkseeker::tracker::LinkSeekTracker;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arg1 = std::env::args().skip(1).next();
    let start_port = if let Some(arg1) = arg1 {
        arg1.parse::<u16>()?
    } else {
        linkseeker::client::DEFAULT_LINKSEEKER_PORT
    };
    
    let tracker = LinkSeekTracker::new(start_port);
}