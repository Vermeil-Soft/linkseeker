use crate::data::{FromMiddlemanMsg, ToMiddlemanMsg};

use rand::{distr::Alphanumeric, Rng};

use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    net::SocketAddr,
    time::{Duration, Instant}
};

const REGISTER_EXPIRE_TIME: Duration = Duration::from_secs(60); // 60 seconds

pub struct Remote {
    pub socket: SocketAddr,
    pub expiring: Instant
}

impl Remote {
    fn is_expired(&self, now: Instant) -> bool {
        self.expiring >= now
    }
}

pub struct PunchTracker {
    pub hosts: HashMap<String, Remote>,

    outgoing_messages: VecDeque<(Vec<u8>, SocketAddr)>,
}

impl PunchTracker {
    pub fn new() -> Self {
        Self {
            hosts: Default::default(),
            outgoing_messages: Default::default()
        }
    }

    pub fn cleanup(&mut self) {
        let now = Instant::now();
        self.hosts.retain(|_k, remote| !remote.is_expired(now));
    }

    pub fn send_msg(&mut self, msg: FromMiddlemanMsg, remote: SocketAddr) {
        let bytes = msg.serialize();
        self.outgoing_messages.push_back((bytes, remote));
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
            ToMiddlemanMsg::Request { id } => {
                let Some(host) = self.hosts.get(&id) else {
                    self.send_msg(FromMiddlemanMsg::RequestErr { msg: format!("host code does not exist") }, socket);
                    return;
                };
                let host_socket = host.socket;
                // order server to punch client
                self.send_msg(FromMiddlemanMsg::PunchOrder { remote: host_socket }, socket);
                // order client to punch server
                self.send_msg(FromMiddlemanMsg::PunchOrder { remote: socket }, host_socket);
            },
        }
    }

    pub fn process_outgoing(&mut self, mut f: impl FnMut(Vec<u8>, SocketAddr)) {
        for (bytes, remote) in self.outgoing_messages.drain(..) {
            f(bytes, remote)
        }
    }
}