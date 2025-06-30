use crate::data::{FromMiddlemanMsg, ToMiddlemanMsg};

use rand::{distr::Alphanumeric, Rng};

use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    net::SocketAddr,
    time::{Duration, Instant}
};

const REGISTER_EXPIRE_TIME: Duration = Duration::from_secs(86400); // 24 hours
const REQUEST_EXPIRE_TIME: Duration = Duration::from_secs(600); // 10 minutes

pub struct Remote {
    pub socket: SocketAddr,
    pub expiring: Instant
}

impl Remote {
    fn is_expired(&self, now: Instant) -> bool {
        self.expiring >= now
    }
}

struct Request {
    pub (crate) connecting: String,
    /// socket of connecting part, for answers
    pub (crate) socket: SocketAddr,
    pub (crate) id: String,
    pub (crate) expiring: Instant,
}

pub struct PunchTracker {
    pub hosts: HashMap<String, Remote>,
    pending_requests: VecDeque<Request>,

    outgoing_messages: VecDeque<(Vec<u8>, SocketAddr)>,
}

impl PunchTracker {
    pub fn new() -> Self {
        Self {
            hosts: Default::default(),
            pending_requests: Default::default(),
            outgoing_messages: Default::default()
        }
    }

    pub fn cleanup(&mut self) {
        let now = Instant::now();
        self.hosts.retain(|_k, remote| !remote.is_expired(now));
        self.pending_requests.retain(|req| req.expiring < now);
    }

    pub fn send_msg(&mut self, msg: FromMiddlemanMsg, remote: SocketAddr) {
        let bytes = msg.serialize();
        self.outgoing_messages.push_back((bytes, remote));
    }

    fn add_pending_request(&mut self, connecting: String, remote: SocketAddr, id: String) {
        self.pending_requests.push_back(Request {
            socket: remote,
            connecting,
            id,
            expiring: Instant::now() + REQUEST_EXPIRE_TIME
        });
    }

    fn get_pending_req(&self, connecting: &str, id: String) -> Option<&Request> {
        self.pending_requests.iter().find(|req| {
            req.connecting == connecting && req.id == id
        })
    }

    pub fn process_incoming(&mut self, bytes: &[u8], socket: SocketAddr) {
        let Some(msg) = ToMiddlemanMsg::parse(bytes) else { return };

        match msg {
            ToMiddlemanMsg::Register => {
                if let Some((id, found)) = self.hosts.iter_mut().find(|(_, r)| r.socket == socket) {
                    // check if remote already exists, if it does refresh existing register
                    found.expiring = Instant::now() + REGISTER_EXPIRE_TIME;
                    let id = id.clone();
                    self.send_msg(FromMiddlemanMsg::RegisterOk { id }, socket);
                    return;
                }

                let random_id = rand::rng().sample_iter(Alphanumeric)
                    .take(12)
                    .map(char::from)
                    .collect::<String>();
                let Entry::Vacant(v) = self.hosts.entry(random_id.clone()) else {
                    /* collision, send an error */
                    self.send_msg(FromMiddlemanMsg::RegisterErr { msg: format!("id generation collision") }, socket);
                    return;
                };
                v.insert(Remote {
                    socket,
                    expiring: Instant::now() + REGISTER_EXPIRE_TIME,
                });
                self.send_msg(FromMiddlemanMsg::RegisterOk { id: random_id }, socket);
            },
            ToMiddlemanMsg::Request { id, connecting, pass } => {
                let Some(host) = self.hosts.get(&id) else {
                    self.send_msg(FromMiddlemanMsg::RequestErr { msg: format!("host code does not exist") }, socket);
                    return;
                };
                let host_socket = host.socket;
                self.add_pending_request(connecting.clone(), socket, id);
                self.send_msg(FromMiddlemanMsg::Request { connecting, pass }, host_socket);
            },
            ToMiddlemanMsg::RequestOk { id, connecting } => {
                let Some(host) = self.hosts.get(&id) else {
                    return;
                };
                if host.socket != socket {
                    // an attack: someone tried to answer for someone else
                    return;
                }
                let Some(pending_request) = self.get_pending_req(&connecting, id) else {
                    // request not found, simply ignore
                    return;
                };

                let host_socket = host.socket;
                let client_socket = pending_request.socket;
                // make them punch

                // make the host punch the connecting
                self.send_msg(FromMiddlemanMsg::PunchOrder { connecting: Some(connecting), remote: client_socket }, host_socket);
                // make the connecting punch the host
                self.send_msg(FromMiddlemanMsg::PunchOrder { connecting: None, remote: host_socket }, client_socket);
            },
            ToMiddlemanMsg::RequestErr { id, connecting, msg } => {
                let Some(host) = self.hosts.get(&id) else {
                    return;
                };
                if host.socket != socket {
                    // an attack: someone tried to answer for someone else
                    return;
                }
                let Some(pending_request) = self.get_pending_req(&connecting, id) else {
                    // request not found, simply ignore
                    return;
                };
                self.send_msg(FromMiddlemanMsg::RequestErr { msg }, pending_request.socket);
            }
        }
    }

    pub fn process_outgoing(&mut self, mut f: impl FnMut(Vec<u8>, SocketAddr)) {
        for (bytes, remote) in self.outgoing_messages.drain(..) {
            f(bytes, remote)
        }
    }
}