use std::net::{ToSocketAddrs, SocketAddr, IpAddr, Ipv4Addr, Ipv6Addr};

pub const DEFAULT_LINKSEEKER_PORT: u16 = 61990;

pub fn compute_linkseeker_key(ip_addr: IpAddr) -> u8 {
    let key = match ip_addr {
        IpAddr::V4(v4) => {
            v4.octets().iter().fold(0u8, |acc, new| acc.wrapping_add(*new))
        },
        IpAddr::V6(v6) => {
            v6.octets().iter().fold(0u8, |acc, new| acc.wrapping_add(*new))
        }
    };
    key
}

pub fn fetch_linkseekers(address: &str, port: Option<u16>) -> Result<Vec<(u8, SocketAddr)>, std::io::Error> {
    let addrs = (address, port.unwrap_or(DEFAULT_LINKSEEKER_PORT)).to_socket_addrs()?;
    let values = addrs
        .map(|addr| (compute_linkseeker_key(addr.ip()), addr))
        .collect();
    Ok(values)
}