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
            FromMiddlemanMsg::PunchOrder { connecting, remote } => {
                format!(
                    "{}punchorder{}{}",
                    UDPUNCH_ID,
                    KVS::new("connecting", connecting.as_ref().map(|s| s.as_ref())),
                    KVS::new("remote", remote.as_ref())
                )
            },
            FromMiddlemanMsg::RegisterErr { msg } => {
                format!(
                    "{}registererr{}",
                    UDPUNCH_ID,
                    KVS::new("msg", msg.as_ref())
                )
            },
            FromMiddlemanMsg::RegisterOk { id } => {
                format!(
                    "{}registerok{}",
                    UDPUNCH_ID,
                    KVS::new("id", id.as_ref())
                )
            },
            FromMiddlemanMsg::Request { connecting, pass } => {
                format!(
                    "{}request{}{}",
                    UDPUNCH_ID,
                    KVS::new("connecting", connecting.as_ref()),
                    KVS::new("pass", pass.as_ref().map(|s| s.as_ref()))
                )
            },
            FromMiddlemanMsg::RequestErr { msg } => {
                format!(
                    "{}requesterr{}",
                    UDPUNCH_ID,
                    KVS::new("msg", msg.as_ref()),
                )
            }
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
            ToMiddlemanMsg::RequestErr { id, connecting, msg } => {
                format!(
                    "{}requesterr{}{}{}",
                    UDPUNCH_ID,
                    KVS::new("id", id.as_ref()),
                    KVS::new("msg", msg.as_ref()),
                    KVS::new("connecting", connecting.as_ref()),
                )
            },
            ToMiddlemanMsg::RequestOk { id, connecting } => {
                format!(
                    "{}requestok{}{}",
                    UDPUNCH_ID,
                    KVS::new("id", id.as_ref()),
                    KVS::new("connecting", connecting.as_ref()),
                )
            },
            ToMiddlemanMsg::Request { id, connecting, pass } => {
                format!(
                    "{}request{}{}{}",
                    UDPUNCH_ID,
                    KVS::new("id", id.as_ref()),
                    KVS::new("connecting", connecting.as_ref()),
                    KVS::new("pass", pass.as_ref().map(|s| s.as_ref()))
                )
            }
        };
        s.into_bytes()
    }
}