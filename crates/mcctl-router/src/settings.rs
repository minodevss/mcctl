use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

const SERVER_PLACEHOLDER: &str = "{server}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub listen_ip: IpAddr,
    pub handshake_timeout: Duration,
    pub connect_timeout: Duration,
    /// How long live sessions may continue after shutdown begins.
    pub shutdown_grace: Duration,
    /// IPv6 clients are counted per /64.
    pub per_ip_connections: u16,
    pub max_connections: usize,
    /// Status description while a backend is down; `{server}` becomes the server name.
    pub offline_status: String,
    /// Login disconnect reason while a backend is down; `{server}` becomes the server name.
    pub offline_login: String,
}

impl Settings {
    pub(crate) fn offline_status_for(&self, server: &str) -> String {
        self.offline_status.replace(SERVER_PLACEHOLDER, server)
    }

    pub(crate) fn offline_login_for(&self, server: &str) -> String {
        self.offline_login.replace(SERVER_PLACEHOLDER, server)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            listen_ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            handshake_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(2),
            shutdown_grace: Duration::from_secs(5),
            per_ip_connections: 8,
            max_connections: 1024,
            offline_status: format!("{SERVER_PLACEHOLDER} is offline"),
            offline_login: format!("{SERVER_PLACEHOLDER} is offline. Try again later."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_server_name_into_offline_texts() {
        let settings = Settings::default();
        assert_eq!(
            settings.offline_status_for("survival"),
            "survival is offline"
        );
        assert_eq!(
            settings.offline_login_for("survival"),
            "survival is offline. Try again later."
        );
    }
}
