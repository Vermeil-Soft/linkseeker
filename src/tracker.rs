use crate::{client::compute_linkseeker_key, data::{FromMiddlemanMsg, ToMiddlemanMsg}};

use std::{
    collections::{hash_map::Entry, HashMap},
    hash::Hash,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket as StdUdpSocket},
    sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard},
    time::{Duration, Instant},
};

use tokio::{net::UdpSocket, task::JoinSet};

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
/// New punch checks are ignored past this amount, until old ones expire
const PUNCH_CHECK_MAX: usize = 1 << 16;
/// The only domain answered to `DomainNameReq`, unless set with the `LNKSK_DOMAIN` env variable
const DEFAULT_LNKSK_DOMAIN: &str = "localhost";
const DNS_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a resolved domain is cached
const DNS_REFRESH_INTERVAL: Duration = Duration::from_hours(1);
/// Retry delay when resolving fails (the previous result keeps being used meanwhile)
const DNS_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(1);
const LOG_INTERVAL: Duration = Duration::from_mins(10);

/// Lock helpers: a panic in one handler must not take the whole tracker down with poisoned locks
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn read<T>(m: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|e| e.into_inner())
}

fn write<T>(m: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|e| e.into_inner())
}

struct RdvRemote {
    socket_addr: SocketAddr,
    expiring: Instant,
    /// host is not punch compatible, so proxy must always be used
    always_proxy: bool,
}

impl RdvRemote {
    fn is_expired(&self, now: Instant) -> bool {
        now >= self.expiring
    }
}

#[derive(Default)]
struct RdvHosts {
    by_id: HashMap<u32, RdvRemote>,
    by_addr: HashMap<SocketAddr, u32>,
}

impl RdvHosts {
    /// Returns (rdv_id, is_new). Registering again from the same address refreshes the existing id.
    fn register(&mut self, socket_addr: SocketAddr, always_proxy: bool, now: Instant) -> (u32, bool) {
        if let Some(host) = self.by_addr.get(&socket_addr).and_then(|id| self.by_id.get_mut(id)) {
            host.expiring = now + REGISTER_EXPIRE_TIME;
            return (self.by_addr[&socket_addr], false);
        }
        loop {
            let random_id: u32 = rand::random();
            let Entry::Vacant(v) = self.by_id.entry(random_id) else {
                continue;
            };
            v.insert(RdvRemote {
                socket_addr,
                expiring: now + REGISTER_EXPIRE_TIME,
                always_proxy,
            });
            self.by_addr.insert(socket_addr, random_id);
            return (random_id, true);
        }
    }

    /// Returns (host address, always_proxy)
    fn get(&self, id: u32) -> Option<(SocketAddr, bool)> {
        self.by_id.get(&id).map(|host| (host.socket_addr, host.always_proxy))
    }

    fn cleanup(&mut self, now: Instant) {
        let by_addr = &mut self.by_addr;
        self.by_id.retain(|k, remote| {
            let r = !remote.is_expired(now);
            if !r {
                log::info!("registered id={:x} for {} has expired", k, remote.socket_addr);
                by_addr.remove(&remote.socket_addr);
            }
            r
        });
    }
}

struct ProxyState {
    in_packets: u64,
    out_packets: u64,
    last_active: Instant,
    /// (packet, came from `incoming`) received before the proxy delay was over
    pending: Vec<(Box<[u8]>, bool)>,
}

struct ProxyData {
    /// set by `ProxyTable::insert`
    id: u64,
    /// socket that asked for a proxy
    incoming: SocketAddr,
    socket_n: usize,
    connector_dh_id: [u8; 4],
    /// socket we need to proxy to
    outgoing: SocketAddr,
    first_active: Instant,
    state: Mutex<ProxyState>,
}

impl ProxyData {
    fn new(incoming: (SocketAddr, usize), outgoing: SocketAddr, connector_dh_id: u32, now: Instant) -> Self {
        Self {
            id: 0,
            incoming: incoming.0,
            socket_n: incoming.1,
            outgoing,
            connector_dh_id: connector_dh_id.to_be_bytes(),
            first_active: now,
            state: Mutex::new(ProxyState {
                in_packets: 0,
                out_packets: 0,
                last_active: now,
                pending: Vec::new(),
            }),
        }
    }

    fn is_delay_over(&self, now: Instant) -> bool {
        now >= self.first_active + PROXY_DELAY_BEFORE_FIRST_PACKET
    }

    fn is_expired(&self, now: Instant) -> bool {
        now >= lock(&self.state).last_active + PROXY_EXPIRE_TIME
    }

    /// Relay a packet received from one side of the proxy to the other side
    fn relay(&self, sockets: &Sockets, worker: usize, bytes: &[u8], from_incoming: bool, now: Instant) {
        let mut state = lock(&self.state);
        state.last_active = now;

        if !self.is_delay_over(now) {
            if state.pending.len() < PROXY_MAX_PENDING_PACKETS {
                state.pending.push((Box::from(bytes), from_incoming));
            }
            return;
        }

        // keep the order: held back packets go first
        self.flush_pending(&mut state, sockets, worker);
        self.forward(&mut state, sockets, worker, bytes, from_incoming);
    }

    /// Send a packet to the other side of the proxy
    fn forward(&self, state: &mut ProxyState, sockets: &Sockets, worker: usize, bytes: &[u8], from_incoming: bool) {
        if from_incoming {
            state.in_packets += 1;
            sockets.send_to(self.socket_n, worker, bytes, self.outgoing);
        } else {
            state.out_packets += 1;
            sockets.send_to(self.socket_n, worker, bytes, self.incoming);
        }
    }

    /// Send the held back packets. Only call once the delay is over.
    fn flush_pending(&self, state: &mut ProxyState, sockets: &Sockets, worker: usize) {
        for (bytes, from_incoming) in std::mem::take(&mut state.pending) {
            self.forward(state, sockets, worker, &bytes, from_incoming);
        }
    }
}

/// All the proxies, indexed for the per-packet lookup
#[derive(Default)]
struct ProxyTable {
    next_id: u64,
    by_id: HashMap<u64, Arc<ProxyData>>,
    /// (incoming, outgoing) -> id, there is at most one proxy per pair
    by_pair: HashMap<(SocketAddr, SocketAddr), u64>,
    /// (socket_n, incoming) -> ids, oldest first
    by_incoming: HashMap<(usize, SocketAddr), Vec<u64>>,
    /// (socket_n, outgoing, connector_dh_id) -> ids, oldest first
    by_outgoing: HashMap<(usize, SocketAddr, [u8; 4]), Vec<u64>>,
}

impl ProxyTable {
    fn find_pair(&self, incoming: SocketAddr, outgoing: SocketAddr) -> Option<&Arc<ProxyData>> {
        self.by_pair.get(&(incoming, outgoing)).and_then(|id| self.by_id.get(id))
    }

    fn insert(&mut self, mut proxy: ProxyData) -> Arc<ProxyData> {
        let id = self.next_id;
        self.next_id += 1;
        proxy.id = id;
        let proxy = Arc::new(proxy);
        self.by_pair.insert((proxy.incoming, proxy.outgoing), id);
        self.by_incoming.entry((proxy.socket_n, proxy.incoming)).or_default().push(id);
        self.by_outgoing.entry((proxy.socket_n, proxy.outgoing, proxy.connector_dh_id)).or_default().push(id);
        self.by_id.insert(id, proxy.clone());
        proxy
    }

    fn remove(&mut self, id: u64) -> Option<Arc<ProxyData>> {
        fn remove_from_index<K: Hash + Eq>(index: &mut HashMap<K, Vec<u64>>, key: K, id: u64) {
            if let Entry::Occupied(mut e) = index.entry(key) {
                e.get_mut().retain(|i| *i != id);
                if e.get().is_empty() {
                    e.remove();
                }
            }
        }

        let proxy = self.by_id.remove(&id)?;
        if self.by_pair.get(&(proxy.incoming, proxy.outgoing)) == Some(&id) {
            self.by_pair.remove(&(proxy.incoming, proxy.outgoing));
        }
        remove_from_index(&mut self.by_incoming, (proxy.socket_n, proxy.incoming), id);
        remove_from_index(&mut self.by_outgoing, (proxy.socket_n, proxy.outgoing, proxy.connector_dh_id), id);
        Some(proxy)
    }

    /// Find the proxy a non-linkseeker packet belongs to
    fn find_for_packet(&self, socket_n: usize, from: SocketAddr, dst_dh_id: [u8; 4]) -> Option<Arc<ProxyData>> {
        // packets from the outgoing side are addressed to the connector's dh_id, which is the most specific match
        let ids = self.by_outgoing.get(&(socket_n, from, dst_dh_id))
            .or_else(|| self.by_incoming.get(&(socket_n, from)))?;
        ids.first().and_then(|id| self.by_id.get(id)).cloned()
    }
}

struct PunchCheck {
    first_received: (SocketAddr, usize),
    expire: Instant,
}

impl PunchCheck {
    fn new(from: (SocketAddr, usize), now: Instant) -> Self {
        Self {
            expire: now + PUNCH_CHECK_EXPIRE_TIME,
            first_received: from,
        }
    }

    fn is_expired(&self, now: Instant) -> bool {
        now >= self.expire
    }
}

/// Sockets used to send. They are clones of the receiving sockets, used directly with non-blocking
/// `send_to`: if the send buffer is full, the packet is dropped like any UDP packet would be.
struct Sockets {
    /// per port, one socket per worker (or a single one when SO_REUSEPORT is not available)
    send: [Vec<StdUdpSocket>; UDP_SOCKET_N],
}

impl Sockets {
    fn send_to(&self, socket_n: usize, worker: usize, bytes: &[u8], to: SocketAddr) {
        let sockets = &self.send[socket_n];
        let _r = sockets[worker % sockets.len()].send_to(bytes, to);
    }
}

/// Where a received packet comes from
#[derive(Debug, Clone, Copy)]
struct Origin {
    socket_n: usize,
    worker: usize,
    addr: SocketAddr,
}

/// Our own domain, resolved in the background
struct DnsCache {
    domain: String,
    /// empty until the first successful resolve
    ips: RwLock<Vec<IpAddr>>,
}

impl DnsCache {
    /// Resolve the domain now, then every `DNS_REFRESH_INTERVAL`
    async fn refresh_loop(self: Arc<Self>) {
        loop {
            let lookup = tokio::net::lookup_host((self.domain.as_str(), 0));
            let delay = match tokio::time::timeout(DNS_TIMEOUT, lookup).await {
                Ok(Ok(addrs)) => {
                    let mut ips: Vec<IpAddr> = Vec::new();
                    for addr in addrs {
                        if !ips.contains(&addr.ip()) {
                            ips.push(addr.ip());
                        }
                    }
                    log::debug!("resolved {} to {:?}", self.domain, ips);
                    *write(&self.ips) = ips_filter_collisions(&self.domain, ips);
                    DNS_REFRESH_INTERVAL
                },
                Ok(Err(e)) => {
                    log::warn!("could not resolve {}: {}", self.domain, e);
                    DNS_RETRY_INTERVAL
                },
                Err(_) => {
                    log::warn!("could not resolve {}: timed out", self.domain);
                    DNS_RETRY_INTERVAL
                },
            };
            tokio::time::sleep(delay).await;
        }
    }
}

/// Clients tell linkseekers apart by their key, so IPs sharing a key can't be used: remove all of them
fn ips_filter_collisions(domain: &str, ips: Vec<IpAddr>) -> Vec<IpAddr> {
    let mut by_key: HashMap<u8, Vec<IpAddr>> = HashMap::new();
    for ip in &ips {
        let key = compute_linkseeker_key(*ip);
        log::debug!("{} resolved ip {} has linkseeker key {}", domain, ip, key);
        by_key.entry(key).or_default().push(*ip);
    }
    for (key, same_key) in &by_key {
        if same_key.len() > 1 {
            let same_key: Vec<String> = same_key.iter().map(|ip| ip.to_string()).collect();
            log::error!("{} resolved ips {} have the same linkseeker key {}, they will not be sent to clients",
                domain, same_key.join(" and "), key
            );
        }
    }
    ips.into_iter().filter(|ip| by_key[&compute_linkseeker_key(*ip)].len() == 1).collect()
}

/// State shared by all the receiving tasks
struct Shared {
    sockets: Sockets,
    rdv_hosts: Mutex<RdvHosts>,
    punch_checks: Mutex<HashMap<u32, PunchCheck>>,
    proxies: RwLock<ProxyTable>,
    dns: Arc<DnsCache>,
}

impl Shared {
    fn send_msg(&self, msg: FromMiddlemanMsg, socket_n: usize, worker: usize, remote: SocketAddr) {
        let bytes = msg.serialize();
        // send each message twice just to be sure
        self.sockets.send_to(socket_n, worker, &bytes, remote);
        self.sockets.send_to(socket_n, worker, &bytes, remote);
    }

    fn reply(&self, origin: Origin, msg: FromMiddlemanMsg) {
        self.send_msg(msg, origin.socket_n, origin.worker, origin.addr);
    }

    fn process_incoming(self: &Arc<Self>, bytes: &[u8], origin: Origin) {
        match ToMiddlemanMsg::parse(bytes) {
            Some(msg) => self.process_linkseeker_msg(msg, origin),
            None => self.process_other_msg(bytes, origin),
        };
    }

    /// Send the packets held back by `proxy` once its delay is over
    fn schedule_flush(self: &Arc<Self>, proxy: Arc<ProxyData>, worker: usize) {
        let shared = self.clone();
        tokio::spawn(async move {
            let deadline = proxy.first_active + PROXY_DELAY_BEFORE_FIRST_PACKET;
            tokio::time::sleep_until(deadline.into()).await;
            let mut state = lock(&proxy.state);
            proxy.flush_pending(&mut state, &shared.sockets, worker);
        });
    }

    /// return true if new proxy was created, false if already created OR could not create
    fn start_proxy(self: &Arc<Self>, rdv_id: u32, host: Option<(SocketAddr, bool)>, origin: Origin, dh_id: Option<u32>, client_need_proxy: bool) -> bool {
        let Some((host_addr, host_always_proxy)) = host else {
            self.reply(origin, FromMiddlemanMsg::RequestErr { msg: format!("host code does not exist") });
            return false;
        };
        let Some(dh_id) = dh_id else {
            self.reply(origin, FromMiddlemanMsg::RequestErr { msg: format!("no dh_id given") });
            return false;
        };
        let created = {
            let mut proxies = write(&self.proxies);
            if proxies.find_pair(origin.addr, host_addr).is_some() {
                None
            } else {
                Some(proxies.insert(ProxyData::new((origin.addr, origin.socket_n), host_addr, dh_id, Instant::now())))
            }
        };
        // answer favorably to the request. If the proxy already exists,
        // the client is probably retrying because our answer got lost, so answer again
        self.reply(origin, FromMiddlemanMsg::RequestOk { id: rdv_id, use_proxy: true });

        let Some(proxy) = created else {
            return false;
        };
        self.schedule_flush(proxy, origin.worker);
        log::info!("starting to proxy {} -> (lnksk:{}) -> {} (rdv_id={:8x}); server_need_proxy={}, client_need_proxy={}",
            origin.addr, origin.socket_n, host_addr, rdv_id, host_always_proxy, client_need_proxy
        );
        true
    }

    fn process_linkseeker_msg(self: &Arc<Self>, msg: ToMiddlemanMsg, origin: Origin) {
        let now = Instant::now();
        match msg {
            ToMiddlemanMsg::Register { use_proxy } => {
                let (id, is_new) = lock(&self.rdv_hosts).register(origin.addr, use_proxy, now);
                if is_new {
                    log::info!("registered id {:x} for {}", id, origin.addr);
                }
                self.reply(origin, FromMiddlemanMsg::RegisterOk { id });
            },
            ToMiddlemanMsg::Heartbeat => {},
            ToMiddlemanMsg::Request { id, use_proxy: false, dh_id } => {
                let host = lock(&self.rdv_hosts).get(id);
                let Some((host_socket, host_always_proxy)) = host else {
                    self.reply(origin, FromMiddlemanMsg::RequestErr { msg: format!("host code does not exist") });
                    return;
                };

                if host_always_proxy {
                    self.start_proxy(id, host, origin, dh_id, false);
                    return;
                }

                if host_socket.ip() == origin.addr.ip() {
                    // if the host ip and the requesting ip are the same, they are probably
                    // part of the same network. odds are, they probably can't talk to each other through
                    // the public network through punching, so let's proxy them through us instead.

                    let success = self.start_proxy(id, host, origin, dh_id, false);
                    if success {
                        log::info!("connector and host (rdv_id={:8x}) share same ip {}, started proxy", id, host_socket);
                    }
                    return;
                }

                // answer favorably to the request, without proxy needed
                self.reply(origin, FromMiddlemanMsg::RequestOk { id, use_proxy: false });
                log::info!("trying to punch {} <-> {} (id={:x})", host_socket, origin.addr, id);
                // order server to punch client
                self.reply(origin, FromMiddlemanMsg::PunchOrder { remote: host_socket });
                // order client to punch server
                self.send_msg(
                    FromMiddlemanMsg::PunchOrder { remote: origin.addr },
                    origin.socket_n,
                    origin.worker,
                    host_socket
                );
            },
            ToMiddlemanMsg::Request { id, use_proxy: true, dh_id } => {
                let host = lock(&self.rdv_hosts).get(id);
                self.start_proxy(id, host, origin, dh_id, true);
            },
            ToMiddlemanMsg::PunchCheck { id } => {
                let first_received = {
                    let mut checks = lock(&self.punch_checks);
                    let first_received = checks.get(&id).map(|c| c.first_received);
                    if first_received.is_none() {
                        if checks.len() < PUNCH_CHECK_MAX {
                            checks.insert(id, PunchCheck::new((origin.addr, origin.socket_n), now));
                        } else {
                            log::debug!("too many punch checks, ignoring id={:8x} from {}", id, origin.addr);
                        }
                    }
                    first_received
                };

                // received a punch check. If it comes from the same port, we already received this request, ignore it
                if let Some(first_received) = first_received.filter(|f| f.1 != origin.socket_n) {
                    // coming from a different port: check if the socket_addr is different
                    let result = first_received.0 == origin.addr;
                    // addresses are the same from our PoV = udp punching is possible
                    // addresses are different from our PoV = udp punching is not possible
                    log::info!("udp punch check for {} (rdv_id={:8x}): {}", origin.addr, id, result);

                    // send the result to remote (both ways).
                    self.reply(origin, FromMiddlemanMsg::PunchCheckResult { ok: result });
                    self.send_msg(FromMiddlemanMsg::PunchCheckResult { ok: result }, first_received.1, origin.worker, first_received.0);
                }
            },
            ToMiddlemanMsg::ProxyTo { .. } => {
                // disabled for now: requests are ignored and never answered
                //
                // let created = {
                //     let mut proxies = write(&self.proxies);
                //     match proxies.find_pair(remote, origin.addr).map(|p| (p.id, p.connector_dh_id)) {
                //         Some((_, existing_dh_id)) if existing_dh_id == dh_id.to_be_bytes() => None,
                //         existing => {
                //             if let Some((old_id, _)) = existing {
                //                 // we already have a connection, but the new dh_id is different, so reset it...
                //                 proxies.remove(old_id);
                //                 log::warn!("dh_id changed, resetting proxy {} to {} (raw)", origin.addr, remote);
                //             }
                //             Some(proxies.insert(ProxyData::new((remote, origin.socket_n), origin.addr, dh_id, now)))
                //         },
                //     }
                // };
                // if let Some(proxy) = created {
                //     log::info!("starting proxying {} to {} (raw)", origin.addr, remote);
                //     self.schedule_flush(proxy, origin.worker);
                // }
                // // answer even if it already exists: our previous answer may have been lost
                // self.reply(origin, FromMiddlemanMsg::ProxyResult { remote, ok: true });
            },
            ToMiddlemanMsg::DomainNameReq { domain } => {
                // only our own domain is resolved, as "domain:port"
                let port = domain.rsplit_once(':')
                    .filter(|(host, _)| host.eq_ignore_ascii_case(&self.dns.domain))
                    .and_then(|(_, port)| port.parse::<u16>().ok());
                let Some(port) = port else {
                    log::debug!("ignoring DNS request for {:?} from {}", domain, origin.addr);
                    return;
                };
                let results: Vec<SocketAddr> = read(&self.dns.ips).iter().map(|ip| SocketAddr::new(*ip, port)).collect();
                if results.is_empty() {
                    // not resolved yet, or it never succeeded
                    return;
                }
                self.reply(origin, FromMiddlemanMsg::DomainNameResult { domain, results });
            },
            ToMiddlemanMsg::Ping { id } => {
                self.reply(origin, FromMiddlemanMsg::Pong { id });
            },
        }
    }

    fn process_other_msg(&self, bytes: &[u8], origin: Origin) {
        let Some(dst_dh_id) = bytes.get(8..12) else {
            return;
        };
        let dst_dh_id: [u8; 4] = dst_dh_id.try_into().unwrap();
        let Some(proxy) = read(&self.proxies).find_for_packet(origin.socket_n, origin.addr, dst_dh_id) else {
            return;
        };
        let from_incoming = proxy.incoming == origin.addr;
        proxy.relay(&self.sockets, origin.worker, bytes, from_incoming, Instant::now());
    }

    fn cleanup(&self) {
        let now = Instant::now();
        lock(&self.rdv_hosts).cleanup(now);
        lock(&self.punch_checks).retain(|_, check| !check.is_expired(now));

        // find the expired proxies without blocking the packets being relayed
        let expired: Vec<u64> = read(&self.proxies).by_id.values()
            .filter(|p| p.is_expired(now))
            .map(|p| p.id)
            .collect();
        if expired.is_empty() {
            return;
        }
        let mut proxies = write(&self.proxies);
        for id in expired {
            // a packet may have refreshed it in between
            if proxies.by_id.get(&id).is_none_or(|p| !p.is_expired(now)) {
                continue;
            }
            let Some(proxy_data) = proxies.remove(id) else { continue };
            let state = lock(&proxy_data.state);
            log::info!("proxying S={} <-> R={} has expired: {}p from S, {}p from R",
                proxy_data.incoming, proxy_data.outgoing, state.out_packets, state.in_packets
            );
        }
    }

    fn log_status(&self) {
        let hosts = lock(&self.rdv_hosts).by_id.len();
        let proxies = read(&self.proxies).by_id.len();
        if hosts == 0 && proxies == 0 {
            log::info!("status: no hosts or proxies registered");
        } else {
            log::info!("status: currently {} hosts registered, and proxying {} connections", hosts, proxies);
        }
    }

    async fn recv_loop(self: Arc<Self>, socket: Arc<UdpSocket>, socket_n: usize, worker: usize) {
        let mut buf = [0; 1500];
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((size, addr)) => {
                    self.process_incoming(&buf[..size], Origin { socket_n, worker, addr });
                },
                Err(e) => {
                    // e.g. ICMP errors reported on the socket: they don't concern the other remotes
                    log::debug!("recv error on socket {}: {}", socket_n, e);
                    tokio::task::yield_now().await;
                },
            }
        }
    }
}

/// Bind `count` sockets on the same port. With more than one, SO_REUSEPORT lets the kernel spread
/// the incoming packets between them (always the same socket for a given remote).
fn bind_udp_port(port: u16, count: usize) -> std::io::Result<Vec<StdUdpSocket>> {
    use socket2::{Domain, Protocol, Socket, Type};

    let addr: SocketAddr = (Ipv4Addr::UNSPECIFIED, port).into();
    (0..count).map(|_| {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        #[cfg(target_os = "linux")]
        socket.set_reuse_port(count > 1)?;
        socket.set_nonblocking(true)?;
        socket.bind(&addr.into())?;
        Ok(socket.into())
    }).collect()
}

pub struct LinkSeekTracker {
    start_port: u16,
    workers_n: usize,
    domain: String,
    /// per port, the sockets not yet given to the runtime
    sockets: [Vec<StdUdpSocket>; UDP_SOCKET_N],
}

impl LinkSeekTracker {
    pub fn new(start_port: u16) -> Result<Self, Box<dyn std::error::Error>> {
        let domain = std::env::var("LNKSK_DOMAIN").unwrap_or_else(|_| DEFAULT_LNKSK_DOMAIN.to_string());
        log::info!("starting link seek tracker, using LNKSK_DOMAIN of {}", domain);
        let workers_n = std::thread::available_parallelism().map_or(1, |n| n.get());
        let sockets_per_port = if cfg!(target_os = "linux") { workers_n } else { 1 };
        let second_port = start_port.checked_add(1).ok_or("start port is too high")?;
        // use 2 ports internally. One for normal use and the other to test punching.
        Ok(Self {
            start_port,
            workers_n,
            domain,
            sockets: [
                bind_udp_port(start_port, sockets_per_port)?,
                bind_udp_port(second_port, sockets_per_port)?
            ],
        })
    }

    /// Runs the tracker on a multi-threaded runtime. Only returns on error.
    pub fn run(&mut self) -> std::io::Result<()> {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(self.workers_n)
            .enable_all()
            .build()?
            .block_on(self.run_async())
    }

    /// Runs the tracker on the current tokio runtime. Only returns on error.
    pub async fn run_async(&mut self) -> std::io::Result<()> {
        if self.sockets.iter().any(|s| s.is_empty()) {
            return Err(std::io::Error::other("tracker is already running"));
        }
        let mut send: [Vec<StdUdpSocket>; UDP_SOCKET_N] = Default::default();
        let mut recv: Vec<(usize, usize, Arc<UdpSocket>)> = Vec::new();
        for (socket_n, port_sockets) in std::mem::take(&mut self.sockets).into_iter().enumerate() {
            for (worker, socket) in port_sockets.into_iter().enumerate() {
                send[socket_n].push(socket.try_clone()?);
                recv.push((socket_n, worker, Arc::new(UdpSocket::from_std(socket)?)));
            }
        }
        let shared = Arc::new(Shared {
            sockets: Sockets { send },
            rdv_hosts: Default::default(),
            punch_checks: Default::default(),
            proxies: Default::default(),
            dns: Arc::new(DnsCache { domain: self.domain.clone(), ips: Default::default() }),
        });
        log::info!("listening on ports {} and {} with {} workers", self.start_port, self.start_port + 1, self.workers_n);

        let mut tasks = JoinSet::new();
        // without SO_REUSEPORT there is one socket per port: share it between several tasks instead
        let sockets_per_port = recv.len() / UDP_SOCKET_N;
        let tasks_per_socket = (self.workers_n / sockets_per_port).max(1);
        for (socket_n, worker, socket) in recv {
            for i in 0..tasks_per_socket {
                tasks.spawn(shared.clone().recv_loop(socket.clone(), socket_n, worker * tasks_per_socket + i));
            }
        }

        // spawn the DNS refresh loop
        tasks.spawn(shared.dns.clone().refresh_loop());
    
        // spawn 2 maintenance loops: one for cleanup, one for logging
        let maintenance = shared.clone();
        tasks.spawn(async move {
            let mut cleanup = tokio::time::interval(CLEANUP_INTERVAL);
            cleanup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                cleanup.tick().await;
                maintenance.cleanup();
            }
        });
        let maintenance = shared.clone();
        tasks.spawn(async move {
            let mut status = tokio::time::interval_at(tokio::time::Instant::now() + LOG_INTERVAL, LOG_INTERVAL);
            loop {
                status.tick().await;
                maintenance.log_status();
            }
        });

        // every task loops forever, so getting here means one of them panicked
        match tasks.join_next().await {
            Some(Err(e)) => Err(std::io::Error::other(format!("tracker task failed: {}", e))),
            _ => Err(std::io::Error::other("tracker task stopped")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_collisions_are_removed() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        // 10.0.0.1 and 10.0.1.0 both have the key 11
        let ips = vec![ip("10.0.0.1"), ip("10.0.0.2"), ip("10.0.1.0"), ip("10.0.0.3")];
        assert_eq!(ips_filter_collisions("test", ips), vec![ip("10.0.0.2"), ip("10.0.0.3")]);

        let ips = vec![ip("10.0.0.1"), ip("10.0.1.0"), ip("10.1.0.0")];
        assert!(ips_filter_collisions("test", ips).is_empty());
    }
}
