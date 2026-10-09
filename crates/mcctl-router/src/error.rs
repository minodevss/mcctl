#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("host {host} is already routed to {server}")]
    DuplicateHost { host: String, server: String },
    #[error("port {port} is already routed to {server}")]
    DuplicatePort { port: u16, server: String },
    #[error("port {port} is the main port and cannot be dedicated to {server}")]
    MainPort { port: u16, server: String },
}
