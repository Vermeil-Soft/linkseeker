#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToMiddlemanMsg {
    /// Register to the middleman, should return an id
    Register,
    /// Request to connect to the registered.
    Request { id: u32, use_proxy: bool },
    PunchCheck { id: u32 },
    ProxyTo { remote: std::net::SocketAddr },
    Ping { id: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromMiddlemanMsg {
    RegisterOk { id: u32 },
    RegisterErr { msg: String },
    /// Request to connect to the registered has failed.
    RequestErr { msg: String },
    /// Order the client or host to punch the remote
    PunchOrder { remote: std::net::SocketAddr },
    /// Order a client to punch THIS server, at port given
    PunchLinkseeker { port: u16 },
    PunchCheckResult { ok: bool },
    ProxyResult { remote: std::net::SocketAddr, ok: bool },
    Pong { id: u32 },
}