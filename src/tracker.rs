use crate::data::{FromMiddlemanMsg, ToMiddlemanMsg};

use std::{
    collections::{hash_map::Entry, HashMap},
    net::{SocketAddr, ToSocketAddrs, UdpSocket},
    time::{Duration, Instant}
};

const REGISTER_EXPIRE_TIME: Duration = Duration::from_secs(60); // 60 seconds
const PROXY_EXPIRE_TIME: Duration = Duration::from_secs(60); // 60 seconds
const PUNCH_CHECK_EXPIRE_TIME: Duration = Duration::from_secs(60); // 60 seconds
const UDP_SOCKET_N: usize = 2;
/// Don't proxy packets too soon after the proxy is created:
/// if router's DmZ is active, if we send a packet *before* they send one,
/// the packet will go through DmZ instead of the remote, and the remote will be invalid
/// if we wait a bit before sending packets, we ensure that the DmZ does not catch our packet
const PROXY_DELAY_BEFORE_FIRST_PACKET: Duration = Duration::from_millis(250);
/// Packets received during `PROXY_DELAY_BEFORE_FIRST_PACKET` are held back (up to this amount)
/// and sent once the delay is over, instead of being dropped (e.g. the first rudp Syn).
const PROXY_MAX_PENDING_PACKETS: usize = 16;

pub struct RdvRemote {
    pub socket_addr: SocketAddr,
    pub expiring: Instant,
    /// host is not punch compatible, so proxy must always be used
    pub always_proxy: bool,
}

impl RdvRemote {
    fn is_expired(&self, now: Instant) -> bool {
        now >= self.expiring
    }
}

pub struct ProxyData {
    /// socket that asked for a proxy
    pub incoming: SocketAddr,
    pub socket_n: usize,
    pub connector_dh_id: [u8; 4],
    /// socket we need to proxy to
    pub outgoing: SocketAddr,
    pub in_packets: u64,
    pub out_packets: u64,
    pub last_active: Instant,
    pub first_active: Instant,
    /// (packet, came from `incoming`) received before the proxy delay was over
    pub pending: Vec<(Box<[u8]>, bool)>,
}

impl ProxyData {
    fn new(incoming: (SocketAddr, usize), outgoing: SocketAddr, connector_dh_id: u32, now: Instant) -> Self {
        Self {
            incoming: incoming.0,
            socket_n: incoming.1,
            outgoing,
            connector_dh_id: connector_dh_id.to_be_bytes(),
            in_packets: 0,
            out_packets: 0,
            last_active: now,
            first_active: now,
            pending: Vec::new(),
        }
    }

    fn is_delay_over(&self, now: Instant) -> bool {
        now >= self.first_active + PROXY_DELAY_BEFORE_FIRST_PACKET
    }

    /// Send a packet to the other side of the proxy
    fn forward(&mut self, udp_sockets: &[UdpSocket; UDP_SOCKET_N], bytes: &[u8], from_incoming: bool) {
        if from_incoming {
            self.in_packets += 1;
            let _r = udp_sockets[self.socket_n].send_to(bytes, self.outgoing);
        } else {
            self.out_packets += 1;
            let _r = udp_sockets[self.socket_n].send_to(bytes, self.incoming);
        }
    }

    /// Send the held back packets if the delay is over
    fn flush_pending(&mut self, udp_sockets: &[UdpSocket; UDP_SOCKET_N], now: Instant) {
        if self.pending.is_empty() || !self.is_delay_over(now) {
            return;
        }
        for (bytes, from_incoming) in std::mem::take(&mut self.pending) {
            self.forward(udp_sockets, &bytes, from_incoming);
        }
    }

    fn is_expired(&self, now: Instant) -> bool {
        now >= self.last_active + PROXY_EXPIRE_TIME
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
        now >= self.expire
    }
}

pub struct LinkSeekTracker {
    #[allow(unused)]
    pub (self) start_port: u16,
    pub (self) now: Instant,
    pub (self) last_log: Instant,
    pub rdv_hosts: HashMap<u32, RdvRemote>,
    pub punch_checks: Vec<PunchCheck>,
    pub udp_sockets: [UdpSocket; UDP_SOCKET_N],
    pub proxy_list: Vec<ProxyData>,
}

impl LinkSeekTracker {
    pub fn new(start_port: u16) -> Result<Self, Box<dyn std::error::Error>> {
        log::info!("starting link seek tracker");
        // use 2 sockets internally. One for normal use and the other to test punching.
        let socket1 = UdpSocket::bind(("0.0.0.0", start_port))?;
        let socket2 = UdpSocket::bind(("0.0.0.0", start_port + 1))?;
        socket1.set_nonblocking(true)?;
        socket2.set_nonblocking(true)?;
        let now = Instant::now();
        Ok(Self {
            start_port,
            rdv_hosts: Default::default(),
            proxy_list: Vec::new(),
            udp_sockets: [socket1, socket2],
            punch_checks: Vec::new(),
            now,
            last_log: now
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

    /// Sends the packets that were held back while proxies were too young
    pub fn flush_pending_proxies(&mut self) {
        for proxy_data in &mut self.proxy_list {
            proxy_data.flush_pending(&self.udp_sockets, self.now);
        }
    }

    /// Every X minutes, tries to log the current tracker status. Should be called regularly.
    pub fn try_log_status(&mut self) {
        const LOG_INTERVAL: Duration = Duration::from_mins(10);
        let diff = self.now.saturating_duration_since(self.last_log);
        if diff < LOG_INTERVAL {
            return;
        }
        self.last_log = self.now;

        if self.rdv_hosts.len() == 0 && self.proxy_list.len() == 0 {
            log::info!("status: no hosts or proxies registered");
        } else {
            log::info!("status: currently {} hosts registered, and proxying {} connections",
                self.rdv_hosts.len(), self.proxy_list.len()
            );
        }
    }

    pub fn send_msg(&mut self, msg: FromMiddlemanMsg, our_socket_n: usize, remote: SocketAddr) {
        let bytes = msg.serialize();
        // send each message twice just to be sure
        let _r = self.udp_sockets[our_socket_n].send_to(&*bytes, remote);
        let _r = self.udp_sockets[our_socket_n].send_to(&*bytes, remote);
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
            self.flush_pending_proxies();
            self.try_log_status();
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

    fn gen_random_rdv_id(&mut self, socket_addr: SocketAddr, always_proxy: bool) -> u32 {
        'gen_loop: loop {
            let random_id: u32 = rand::random();
            let Entry::Vacant(v) = self.rdv_hosts.entry(random_id) else {
                continue 'gen_loop;
            };

            v.insert(RdvRemote {
                socket_addr,
                expiring: self.now + REGISTER_EXPIRE_TIME,
                always_proxy
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

    /// return true if new proxy was created, false if already created OR could not create
    fn start_proxy(&mut self, rdv_id: u32, (socket_n, socket_addr): (usize, SocketAddr), dh_id: Option<u32>, client_need_proxy: bool) -> bool {
        let Some(host) = self.rdv_hosts.get(&rdv_id) else {
            self.send_msg(
                FromMiddlemanMsg::RequestErr { msg: format!("host code does not exist") },
                socket_n,
                socket_addr
            );
            return false;
        };
        let Some(dh_id) = dh_id else {
            self.send_msg(
                FromMiddlemanMsg::RequestErr { msg: format!("no dh_id given") },
                socket_n,
                socket_addr
            );
            return false;
        };
        let host_addr = host.socket_addr;
        let host_always_proxy = host.always_proxy;
        if self.proxy_list.iter().any(|proxy| proxy.incoming == socket_addr && proxy.outgoing == host_addr) {
            // already exists: the client is probably retrying because our answer got lost, so answer again
            self.send_msg(
                FromMiddlemanMsg::RequestOk { id: rdv_id, use_proxy: true },
                socket_n,
                socket_addr
            );
            return false;
        }
        // answer favorably to the request
        self.send_msg(
            FromMiddlemanMsg::RequestOk { id: rdv_id, use_proxy: true },
            socket_n,
            socket_addr
        );

        self.proxy_list.push(ProxyData::new(
            (socket_addr, socket_n),
            host_addr,
            dh_id,
            self.now
        ));
        log::info!("starting to proxy {} -> (lnksk:{}) -> {} (rdv_id={:8x}); server_need_proxy={}, client_need_proxy={}",
            socket_addr, socket_n, host_addr, rdv_id, host_always_proxy, client_need_proxy
        );
        true
    }

    pub fn process_linkseeker_msg(&mut self, msg: ToMiddlemanMsg, our_socket_n: usize, socket_addr: SocketAddr) {
        match msg {
            ToMiddlemanMsg::Register { use_proxy } => {
                if let Some((id, found)) = self.rdv_hosts.iter_mut().find(|(_, r)| r.socket_addr == socket_addr) {
                    // check if remote already exists, if it does refresh existing register
                    found.expiring = Instant::now() + REGISTER_EXPIRE_TIME;
                    let id = id.clone();
                    self.send_msg(FromMiddlemanMsg::RegisterOk { id }, our_socket_n, socket_addr);
                    return;
                }

                let rdv_id = self.gen_random_rdv_id(socket_addr, use_proxy);
                log::info!("registered id {:x} for {}", rdv_id, socket_addr);
                self.send_msg(FromMiddlemanMsg::RegisterOk { id: rdv_id }, our_socket_n, socket_addr);
            },
            ToMiddlemanMsg::Heartbeat => {},
            ToMiddlemanMsg::Request { id, use_proxy: false, dh_id } => {
                let Some(host) = self.rdv_hosts.get(&id) else {
                    self.send_msg(
                        FromMiddlemanMsg::RequestErr { msg: format!("host code does not exist") },
                        our_socket_n,
                        socket_addr
                    );
                    return;
                };
                let host_socket = host.socket_addr;

                if host.always_proxy {
                    self.start_proxy(id, (our_socket_n, socket_addr), dh_id, false);
                    return;
                }

                if host_socket.ip() == socket_addr.ip() {
                    // if the host ip and the requesting ip are the same, they are probably
                    // part of the same network. odds are, they probably can't talk to each other through
                    // the public network through punching, so let's proxy them through us instead.

                    let success = self.start_proxy(id, (our_socket_n, socket_addr), dh_id, false);
                    if success {
                        log::info!("connector and host (rdv_id={:8x}) share same ip {}, started proxy", id, host_socket);
                    }
                    return;
                }

                // answer favorably to the request, without proxy needed
                self.send_msg(
                    FromMiddlemanMsg::RequestOk { id, use_proxy: false },
                    our_socket_n,
                    socket_addr
                );
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
            ToMiddlemanMsg::Request { id, use_proxy: true, dh_id } => {
                self.start_proxy(id, (our_socket_n, socket_addr), dh_id, true);
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
                        log::info!("udp punch check for {} (rdv_id={:8x}): {}", socket_addr, id, result);

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
            ToMiddlemanMsg::ProxyTo { remote, dh_id } => {
                // check if the proxy doesn't already exist
                if let Some(p) = self.proxy_list.iter().find(|p| p.outgoing == remote && p.incoming == socket_addr) {
                    if p.connector_dh_id != dh_id.to_be_bytes() {
                        // we already have a connection, by the new dh_id is different, so reset it...
                        self.proxy_list.retain(|p| p.outgoing != remote || p.incoming != socket_addr);
                        log::warn!("starting proxying {} to {} (raw)", socket_addr, remote);
                    } else {
                        return;
                    }
                }
                log::info!("starting proxying {} to {} (raw)", socket_addr, remote);
                self.proxy_list.push(ProxyData::new(
                    (remote, our_socket_n),
                    socket_addr,
                    dh_id,
                    self.now
                ));
                self.send_msg(
                    FromMiddlemanMsg::ProxyResult { remote: remote, ok: true },
                    our_socket_n,
                    socket_addr
                );
            },
            ToMiddlemanMsg::DomainNameReq { domain } => {
                let Ok(results) = domain.to_socket_addrs() else {
                    return;
                };
                let mut v = Vec::new();
                for result in results {
                    v.push(result);
                }
                self.send_msg(
                    FromMiddlemanMsg::DomainNameResult { domain, results: v },
                    our_socket_n,
                    socket_addr
                );
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

    pub fn process_other_msg(&mut self, bytes: &[u8], socket_n: usize, socket_addr: SocketAddr) {
        let Some(dst_dh_id_bytes) = bytes.get(8..12) else {
            return;
        };
        let dst_dh_id: &[u8; 4] = dst_dh_id_bytes.as_array().unwrap();
        let found = self.proxy_list.iter_mut().find(|p|
            (p.socket_n == socket_n && p.outgoing == socket_addr && p.connector_dh_id == *dst_dh_id) ||
            (p.socket_n == socket_n && p.incoming == socket_addr)
        );
        if let Some(found) = found {
            found.last_active = self.now;
            let from_incoming = found.incoming == socket_addr;

            if !found.is_delay_over(self.now) {
                if found.pending.len() < PROXY_MAX_PENDING_PACKETS {
                    found.pending.push((Box::from(bytes), from_incoming));
                }
                return;
            }

            // keep the order: held back packets go first
            found.flush_pending(&self.udp_sockets, self.now);
            found.forward(&self.udp_sockets, bytes, from_incoming);
        }
    }

}