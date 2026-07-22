#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToMiddlemanMsg {
    /// Register to the middleman, should return an id.
    ///
    /// use_proxy means that all remotes connecting will need use a proxy under the hood.
    Register { use_proxy: bool },
    /// Request to connect to the registered.
    ///
    /// If use_proxy is true, you must include a dh_id.
    Request { id: u32, use_proxy: bool, dh_id: Option<u32> },
    PunchCheck { id: u32 },
    /// Ask to proxy to this specific public address. Only works for rudp v2
    ///
    /// DH_ID should be encoded as BigEndian
    ProxyTo { remote: std::net::SocketAddr, dh_id: u32 },
    Ping { id: u32 },
    DomainNameReq { domain: String },
    /// A request to ensure the connection is kept alive by NATs and such
    Heartbeat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromMiddlemanMsg {
    RegisterOk { id: u32 },
    RegisterErr { msg: String },
    /// Request to connect to the registered has failed.
    RequestErr { msg: String },
    /// Order the client or host to punch the remote
    PunchOrder { remote: std::net::SocketAddr },
    /// The request to connect to id has been accepted. If proxy=true, just send messages to the same address,
    /// otherwise a PunchOrder is coming.
    RequestOk { id: u32, use_proxy: bool },
    PunchCheckResult { ok: bool },
    /// Answer whether or not the request has been granted
    ProxyResult { remote: std::net::SocketAddr, ok: bool },
    DomainNameResult { domain: String, results: Vec<std::net::SocketAddr> },
    Pong { id: u32 },
    /// A request to ensure the connection is kept alive by NATs and such
    Heartbeat,
}