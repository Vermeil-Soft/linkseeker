use crate::{
    data::{FromMiddlemanMsg, ToMiddlemanMsg},
    common::UDPUNCH_ID
};

struct KeyValueSerializer<'a> {
    key: &'a str,
    value: Option<&'a str>,
}

impl<'a> KeyValueSerializer<'a> {
    pub fn new<I: Into<Option<&'a str>>>(key: &'a str, value: I) -> Self {
        Self { key, value: value.into() }
    }
}

impl<'a> std::fmt::Display for KeyValueSerializer<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.value {
            None => Ok(()),
            Some(value) => write!(f, "/{}={}", self.key, value),
        }
    }
}

impl FromMiddlemanMsg {
    pub fn serialize(&self) -> Vec<u8> {
        use KeyValueSerializer as KVS;
        let s = match self {
            FromMiddlemanMsg::RegisterOk { id } => {
                let id_str = format!("{}", id);
                format!(
                    "{}registerok{}",
                    UDPUNCH_ID,
                    KVS::new("id", id_str.as_ref())
                )
            },
            FromMiddlemanMsg::RegisterErr { msg } => {
                format!(
                    "{}registererr{}",
                    UDPUNCH_ID,
                    KVS::new("msg", msg.as_ref())
                )
            },
            FromMiddlemanMsg::RequestErr { msg } => {
                format!(
                    "{}requesterr{}",
                    UDPUNCH_ID,
                    KVS::new("msg", msg.as_ref()),
                )
            },
            FromMiddlemanMsg::PunchOrder { remote } => {
                let remote = remote.to_string();
                format!(
                    "{}punchorder{}",
                    UDPUNCH_ID,
                    KVS::new("remote", &*remote)
                )
            },
            FromMiddlemanMsg::PunchLinkseeker { port } => {
                let port = port.to_string();
                format!(
                    "{}punchlnksk{}",
                    UDPUNCH_ID,
                    KVS::new("port", &*port)
                )
            },
            FromMiddlemanMsg::PunchCheckResult { ok } => {
                format!(
                    "{}punchcheckr{}",
                    UDPUNCH_ID,
                    KVS::new("ok", if *ok { "1" } else { "0" }),
                )
            },
            FromMiddlemanMsg::ProxyResult { remote, ok } => {
                let remote = remote.to_string();
                format!(
                    "{}proxyr{}{}",
                    UDPUNCH_ID,
                    KVS::new("remote", &*remote),
                    KVS::new("ok", if *ok { "1" } else { "0" }),
                )
            },
            FromMiddlemanMsg::Pong { id } => {
                let id_str = format!("{}", id);
                format!(
                    "{}pong{}",
                    UDPUNCH_ID,
                    KVS::new("id", id_str.as_ref())
                )
            },
        };
        s.into_bytes()
    }
}

impl ToMiddlemanMsg {
    pub fn serialize(&self) -> Vec<u8> {
        use KeyValueSerializer as KVS;
        let s = match self {
            ToMiddlemanMsg::Register => {
                format!(
                    "{}register",
                    UDPUNCH_ID,
                )
            },
            ToMiddlemanMsg::Request { id, use_proxy } => {
                let id_str = format!("{}", id);
                format!(
                    "{}request{}{}",
                    UDPUNCH_ID,
                    KVS::new("id", id_str.as_ref()),
                    KVS::new("useproxy", if *use_proxy { "1" } else { "0" }),
                )
            }
            ToMiddlemanMsg::PunchCheck { id } => {
                let id_str = format!("{}", id);
                format!(
                    "{}punchcheck{}",
                    UDPUNCH_ID,
                    KVS::new("id", id_str.as_ref()),
                )
            },
            ToMiddlemanMsg::ProxyTo { remote } => {
                let remote = remote.to_string();
                format!(
                    "{}proxy{}",
                    UDPUNCH_ID,
                    KVS::new("remote", &*remote)
                )
            },
            ToMiddlemanMsg::Ping { id } => {
                let id_str = format!("{}", id);
                format!(
                    "{}ping{}",
                    UDPUNCH_ID,
                    KVS::new("id", id_str.as_ref()),
                )
            },
        };
        s.into_bytes()
    }
}