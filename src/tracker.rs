use crate::data::{FromMiddlemanMsg, ToMiddlemanMsg};

use rand::{distr::Alphanumeric, Rng};

use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant}
};

const REGISTER_EXPIRE_TIME: Duration = Duration::from_secs(60); // 60 seconds
const PROXY_EXPIRE_TIME: Duration = Duration::from_secs(60); // 60 seconds
const PUNCH_CHECK_EXPIRE_TIME: Duration = Duration::from_secs(60); // 60 seconds
const UDP_SOCKET_N: usize = 4;

pub struct RdvRemote {
    pub socket_addr: SocketAddr,
    pub expiring: Instant
}

impl RdvRemote {
    fn is_expired(&self, now: Instant) -> bool {
        self.expiring >= now
    }
}

pub struct ProxyData {
    /// socket that asked for a proxy
    pub incoming: SocketAddr,
    pub in_socket_n: usize,
    /// socket we need to proxy to
    pub outgoing: SocketAddr,
    pub out_socket_n: usize,
    pub in_packets: u64,
    pub out_packets: u64,
    pub last_active: Instant,
}

impl ProxyData {
    fn new(incoming: (SocketAddr, usize), outgoing: (SocketAddr, usize), now: Instant) -> Self {
        Self {
            incoming: incoming.0,
            in_socket_n: incoming.1,
            outgoing: outgoing.0,
            out_socket_n: outgoing.1,
            in_packets: 0,
            out_packets: 0,
            last_active: now
        }
    }

    fn is_expired(&self, now: Instant) -> bool {
        self.last_active + PROXY_EXPIRE_TIME >= now
    }
}

pub struct PunchCheck {
    pub first_received: (SocketAddr, usize),
    pub id: u32,
    pub expire: Instant,
}

impl PunchCheck {
    pub fn new(id: u32, from: (SocketAddr, usize), now: Instant) -> Self {
        Self {
            id,
            expire: now + PUNCH_CHECK_EXPIRE_TIME,
            first_received: from,
        }
    }

    fn is_expired(&self, now: Instant) -> bool {
        self.expire >= now
    }
}

pub struct LinkSeekTracker {
    pub (self) _start_port: u16,
    pub (self) now: Instant,
    pub rdv_hosts: HashMap<u32, RdvRemote>,
    pub punch_checks: Vec<PunchCheck>,
    pub udp_sockets: [UdpSocket; UDP_SOCKET_N],
    pub proxy_list: Vec<ProxyData>,
}

impl LinkSeekTracker {
    pub fn new(start_port: u16) -> Result<Self, Box<dyn std::error::Error>> {
        log::info!("starting link seek tracker");
        // use 4 sockets internally. If we have a normal host and all others should use proxy,
        // if we don't have different sockets, the remote will have no way of knowing which host is talking
        // to it. So yes we are limited to 4 proxy remotes per server...
        // if that's not enough that time is far in the future and a new, smarter me will be able to handle it
        let socket1 = UdpSocket::bind(("0.0.0.0", start_port))?;
        let socket2 = UdpSocket::bind(("0.0.0.0", start_port + 1))?;
        let socket3 = UdpSocket::bind(("0.0.0.0", start_port + 2))?;
        let socket4 = UdpSocket::bind(("0.0.0.0", start_port + 3))?;
        socket1.set_nonblocking(true)?;
        socket2.set_nonblocking(true)?;
        socket3.set_nonblocking(true)?;
        socket4.set_nonblocking(true)?;
        Ok(Self {
            _start_port: start_port,
            rdv_hosts: Default::default(),
            proxy_list: Vec::new(),
            udp_sockets: [socket1, socket2, socket3, socket4],
            punch_checks: Vec::new(),
            now: Instant::now(),
        })
    }

    pub fn cleanup(&mut self) {
        self.now = Instant::now();
        self.rdv_hosts.retain(|k, remote| {
            let r = !remote.is_expired(self.now);
            if !r {
                log::info!("registered id={:x} for {} has expired", k, remote.socket_addr);
            }
            r
        });
        self.punch_checks.retain(|check| !check.is_expired(self.now));
        self.proxy_list.retain(|proxy_data| {
            let r = !proxy_data.is_expired(self.now);
            if !r {
                log::info!("proxying S={} <-> R={} has expired: {}p from S, {}p from R",
                    proxy_data.incoming, proxy_data.outgoing, proxy_data.out_packets, proxy_data.in_packets
                );
            }
            r
        });
    }

    pub fn send_msg(&mut self, msg: FromMiddlemanMsg, socket_n: usize, remote: SocketAddr) {
        let bytes = msg.serialize();
        // send each message twice just to be sure
        let _r = self.udp_sockets[socket_n].send_to(&*bytes, remote);
        let _r = self.udp_sockets[socket_n].send_to(&*bytes, remote);
    }

    pub fn run(&mut self) {
        let mut buf = [0; 1500];
        loop {
            let mut processed = false;
            processed |= self.process(&mut buf);
            processed |= self.process(&mut buf);
            processed |= self.process(&mut buf);
            processed |= self.process(&mut buf);
            processed |= self.process(&mut buf);
            processed |= self.process(&mut buf);
            processed |= self.process(&mut buf);
            processed |= self.process(&mut buf);

            self.cleanup();
            if !processed {
                if self.proxy_list.len() > 0 {
                    std::thread::sleep(std::time::Duration::from_micros(100));
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        }
    }

    /// Returns whether or not we processed a message
    pub fn process(&mut self, buf: &mut [u8; 1500]) -> bool {
        let mut has_any: bool = false;
        for i in 0..UDP_SOCKET_N {
            match self.udp_sockets[i].recv_from(buf) {
                Ok((size, socket_addr)) => {
                    self.process_incoming(unsafe { buf.get_unchecked(0..size) }, i, socket_addr);
                    has_any = true;
                },
                _ => { continue }
            }
        }
        has_any
    }

    fn gen_random_rdv_id(&mut self, socket_addr: SocketAddr) -> u32 {
        'gen_loop: loop {
            let random_id: u32 = rand::rng().random();
            let Entry::Vacant(v) = self.rdv_hosts.entry(random_id) else {
                continue 'gen_loop;
            };

            v.insert(RdvRemote {
                socket_addr,
                expiring: self.now + REGISTER_EXPIRE_TIME,
            });
            return random_id
        }
    }

    pub fn process_incoming(&mut self, bytes: &[u8], our_socket_n: usize, socket_addr: SocketAddr) {
        match ToMiddlemanMsg::parse(bytes) {
            Some(msg) => self.process_linkseeker_msg(msg, our_socket_n, socket_addr),
            None => self.process_other_msg(bytes, our_socket_n, socket_addr),
        };
    }

    pub fn process_linkseeker_msg(&mut self, msg: ToMiddlemanMsg, our_socket_n: usize, socket_addr: SocketAddr) {
        match msg {
            ToMiddlemanMsg::Register => {
                if let Some((id, found)) = self.rdv_hosts.iter_mut().find(|(_, r)| r.socket_addr == socket_addr) {
                    // check if remote already exists, if it does refresh existing register
                    found.expiring = Instant::now() + REGISTER_EXPIRE_TIME;
                    let id = id.clone();
                    self.send_msg(FromMiddlemanMsg::RegisterOk { id }, our_socket_n, socket_addr);
                    return;
                }

                let rdv_id = self.gen_random_rdv_id(socket_addr);
                log::info!("registered id {:x} for {}", rdv_id, socket_addr);
                self.send_msg(FromMiddlemanMsg::RegisterOk { id: rdv_id }, our_socket_n, socket_addr);
            },
            ToMiddlemanMsg::Request { id, use_proxy: false } => {
                let Some(host) = self.rdv_hosts.get(&id) else {
                    self.send_msg(
                        FromMiddlemanMsg::RequestErr { msg: format!("host code does not exist") },
                        our_socket_n,
                        socket_addr
                    );
                    return;
                };
                let host_socket = host.socket_addr;
                log::info!("trying to punch {} <-> {} (id={:x})", host_socket, socket_addr, id);
                // order server to punch client
                self.send_msg(
                    FromMiddlemanMsg::PunchOrder { remote: host_socket },
                    our_socket_n,
                    socket_addr
                );
                // order client to punch server
                self.send_msg(
                    FromMiddlemanMsg::PunchOrder { remote: socket_addr },
                    our_socket_n,
                    host_socket
                );
            },
            ToMiddlemanMsg::Request { id, use_proxy: true } => {
                let Some(host) = self.rdv_hosts.get(&id) else {
                    self.send_msg(
                        FromMiddlemanMsg::RequestErr { msg: format!("host code does not exist") },
                        our_socket_n,
                        socket_addr
                    );
                    return;
                };
                let host_socket = host.socket_addr;
                log::info!("trying to punch {} <-> {} (id={:x})", host_socket, socket_addr, id);
                // order server to punch client
                self.send_msg(
                    FromMiddlemanMsg::PunchOrder { remote: host_socket },
                    our_socket_n,
                    socket_addr
                );
                // order client to punch server
                self.send_msg(
                    FromMiddlemanMsg::PunchOrder { remote: socket_addr },
                    our_socket_n,
                    host_socket
                );
            },
            ToMiddlemanMsg::PunchCheck { id } => {
                let found = self.punch_checks.iter().find(|c| c.id == id);

                if let Some(found) = found {
                    let first_received = found.first_received;
                    // received a punch check
                    if first_received.1 != our_socket_n {
                        // coming from a different port: check if the socket_addr is different
                        let result = first_received.0 == socket_addr;
                        // addresses are the same from our PoV = udp punching is possible
                        // addresses are different from our PoV = udp punching is not possible
                        log::info!("udp punch check for {} (id={:x}): {}", socket_addr, id, result);

                        // send the result to remote (both ways).
                        self.send_msg(FromMiddlemanMsg::PunchCheckResult { ok: result }, our_socket_n, socket_addr);
                        self.send_msg(FromMiddlemanMsg::PunchCheckResult { ok: result }, first_received.1, first_received.0);
                    } else {
                        // coming from the same port: already received this request, ignore it
                    }
                } else {
                    self.punch_checks.push(PunchCheck::new(id, (socket_addr, our_socket_n), self.now));
                }
            },
            ToMiddlemanMsg::ProxyTo { remote } => {
                // check if the proxy doesn't already exist
                if self.proxy_list.iter().any(|p| p.outgoing == remote && p.incoming == socket_addr) {
                    return;
                }
                log::info!("starting proxying {} to {}", socket_addr, remote);
                // find the first socket we haven't use for that remote
                let mut our_socket_avail = [true; UDP_SOCKET_N];
                self.proxy_list.iter()
                    .filter(|p| p.outgoing == remote)
                    .for_each(|p| our_socket_avail[p.out_socket_n] = false);
                let used_socket_n = our_socket_avail.iter().enumerate().rev().find_map(|(i, p)| p.then_some(i));
                if let Some(used_socket_n) = used_socket_n {
                    self.proxy_list.push(ProxyData::new(
                        (remote, used_socket_n),
                        (socket_addr, our_socket_n),
                        self.now
                    ));
                    self.send_msg(
                        FromMiddlemanMsg::ProxyResult { remote: remote, ok: true },
                        our_socket_n,
                        socket_addr
                    );
                } else {
                    self.send_msg(
                        FromMiddlemanMsg::ProxyResult { remote: remote, ok: false },
                        our_socket_n,
                        socket_addr
                    );
                }
            },
            ToMiddlemanMsg::Ping { id } => {
                self.send_msg(
                    FromMiddlemanMsg::Pong { id },
                    our_socket_n,
                    socket_addr
                );
            },
        }
    }

    pub fn process_other_msg(&mut self, bytes: &[u8], our_socket_n: usize, socket_addr: SocketAddr) {
        let found = self.proxy_list.iter_mut().find(|p|
            (p.out_socket_n == our_socket_n && p.outgoing == socket_addr) ||
            (p.in_socket_n == our_socket_n && p.incoming == socket_addr)
        );
        if let Some(found) = found {
            // proxy
            if found.incoming == socket_addr {
                found.in_packets += 1;
                let _r = self.udp_sockets[found.out_socket_n].send_to(bytes, found.outgoing);
            } else {
                found.out_packets += 1;
                let _r = self.udp_sockets[found.in_socket_n].send_to(bytes, found.incoming);
            }
        }
    }

}