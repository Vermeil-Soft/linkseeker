
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToMiddlemanMsg {
    /// Register to the middleman, should return an id
    Register,
    /// Request to connect to the registered.
    Request { id: String, connecting: String, pass: Option<String> },
    /// The server accepts the connecting remote
    RequestOk { id: String, connecting: String },
    /// The server does not accept the connecting remote
    RequestErr { id: String, connecting: String, msg: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromMiddlemanMsg {
    RegisterOk { id: String },
    RegisterErr { msg: String },
    /// Request to connect to the registered.
    Request { connecting: String, pass: Option<String> },
    /// Request to connect to the registered has failed.
    RequestErr { msg: String },
    /// Order the client or host to punch the remote
    PunchOrder { connecting: Option<String>, remote: String },
}