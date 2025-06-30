use std::net::SocketAddr;

use crate::{
    data::{FromMiddlemanMsg, ToMiddlemanMsg},
    common::{UDPUNCH_ID, UDPUNCH_ID_BYTES, UDPUNCH_ID_LEN},
};

/// check the head, if it exists return the tail as bytes
fn check_head(bytes: &[u8]) -> Option<&[u8]> {
    let Some((id, tail)) = bytes.split_at_checked(UDPUNCH_ID_LEN) else {
        return None;
    };
    if id != UDPUNCH_ID_BYTES {
        return None;
    }
    Some(tail)
}

fn parse_kv(input: &str) -> Option<(&str, &str)> {
    input.split_once('=')
}

fn process_all_kv<'a>(iter: impl Iterator<Item=&'a str>, mut f: impl FnMut(&str, &str)) -> Option<()> {
    for raw_kv in iter {
        let (k, v) = parse_kv(raw_kv)?;
        f(k, v)
    }
    Some(())
}

impl FromMiddlemanMsg {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let tail = check_head(bytes)?;
        let tail = String::from_utf8_lossy(tail);
        let mut s = tail.split('/');
        let Some(command) = s.next() else {
            return None;
        };
        let parsed = match command {
            "registerok" => {
                let mut id: Option<String> = None;
                process_all_kv(s, |k, v| {
                    if k == "id" { id = Some(v.to_string()); }
                })?;
                Self::RegisterOk { id: id? }
            },
            "registererr" => {
                let mut msg: Option<String> = None;
                process_all_kv(s, |k, v| {
                    if k == "msg" { msg = Some(v.to_string()); }
                })?;
                Self::RegisterErr { msg: msg? }
            },
            "requesterr" => {
                let mut msg: Option<String> = None;
                process_all_kv(s, |k, v| {
                    if k == "msg" { msg = Some(v.to_string()); }
                })?;
                Self::RequestErr { msg: msg? }
            },
            "punchorder" => {
                let mut remote: Option<SocketAddr> = None;
                process_all_kv(s, |k, v| {
                    if k == "remote" { remote = v.parse::<SocketAddr>().ok(); }
                })?;
                Self::PunchOrder { remote: remote? }
            },
            _ => return None,
        };
        Some(parsed)
    }
}

impl ToMiddlemanMsg {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let tail = check_head(bytes)?;
        let tail = String::from_utf8_lossy(tail);
        let mut s = tail.split('/');
        let Some(command) = s.next() else {
            return None;
        };
        let parsed = match command {
            "register" => {
                Self::Register
            },
            "request" => {
                let mut pass: Option<String> = None;
                let mut id: Option<String> = None;
                let mut connecting: Option<String> = None;
                process_all_kv(s, |k, v| {
                    if k == "pass" { pass = Some(v.to_string()); }
                    if k == "connecting" { connecting = Some(v.to_string()); }
                    if k == "id" { id = Some(v.to_string()); }
                })?;
                Self::Request { id: id? }
            },
            _ => return None,
        };
        Some(parsed)
    }
}

#[test]
fn parse_deserialized_from_middleman() {
    let remote = "127.0.0.1:15555".parse::<SocketAddr>().unwrap();
    let orig = FromMiddlemanMsg::PunchOrder { remote };
    let deser = FromMiddlemanMsg::parse(&orig.serialize()).unwrap();
    assert_eq!(orig, deser);
}

#[test]
fn parse_deserialized_to_middleman() {
    let orig = ToMiddlemanMsg::Request { id: format!("1234") };
    let deser = ToMiddlemanMsg::parse(&orig.serialize()).unwrap();
    assert_eq!(orig, deser);
}