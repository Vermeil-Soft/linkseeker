
# LinkSeeker, NAT punching one way or another

# Utilities

LinkSeeker has two utilities:

* Parse/Serialize UDP NAT Punch requests, and track requesters.
* Act as a pseudo-TURN server, meaning basically that RUDP packets will "proxy" through this

## Ways of using

This crate, LinkSeeker has 2 ways of being used:

* Executable, which acts as a standalone server.
* Library, which is exepcted to be used by client using this protocol

There is no wrapping of a RUDP socket in this library, it is assumed you have your own system and can filter
directly the udpunch messages from your framework.

**Note that this is only expected to work with the `rbtl-rudp` library, raw UDP messages will not be properly
proxied.**

# How it works

All udpunch messages start with "#lnksk@". Every UDP that has those 5 characters can be considered to be owned by this
library. Everything else is ignored or passed-through depending on the mode.

More precisely each message has this structure `#lnksk@$REQUEST_ID/key1=value1/key2=value2/key3=value3`.

For instance, a request to connect message would be like this:

`#lnksk@register`.

## Message types

Here are the messages types used in this library:

### Client -> Server

* Register: register an id (u32) to transmit to the other party. The result will be in RegisterOk
* Request(id:u32, use_proxy:bool, dh_id: u32): request to connect to someone who has registered an id.
use_proxy true means that the UDP packets will be proxied through this server, while false will attempt NAT punching
between the 2 parties
* PunchCheck(id: u32): with a random u32, send this request to 2 different working ports of this linkseeker server.
This way, we could be able to see whether or not a host is behind a strong NAT, and proxying is necessary
* ProxyTo (remote:ip+port, dh_id: u32): directly rudp proxy to an ip+port
* Ping(id: u32): will answer pong as fast as possible with the same id. Allows knowing ping.
* DomainNameReq(domain): gets all the addresses of a domain address. Useful to bypass some router which change ip
addresses of some domains

### Server - Client

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromMiddlemanMsg {
    RegisterOk(id: u32),
    RegisterErr(msg: String),
    RequestErr(msg: String),
    PunchOrder(remote: ip+port): request the client to punch this specific address of another client.
    PunchLinkseeker(port: u16): request the client to punch this specific address of ours
    PunchCheckResult(ok: bool): if yes, the check was a success, you can directly connect to the order address!
    ProxyResult(remote: ip+port, ok: bool): confirms that proxying is now in-place
    DomainNameResult(domain: String, results: comma separated(ip+port)),
    Pong(id: u32),
}