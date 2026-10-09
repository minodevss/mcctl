use std::io;
use std::path::PathBuf;

use crate::names::ServerName;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("run with sudo: sudo mcctl {args}")]
    NotRoot { args: String },
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot run {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: io::Error,
    },
    #[error("{command} failed: {detail}")]
    CommandFailed { command: String, detail: String },
    #[error("invalid server name {name:?}: {reason}")]
    InvalidServerName { name: String, reason: &'static str },
    #[error("invalid hostname {input:?}: {reason}")]
    InvalidHostname { input: String, reason: &'static str },
    #[error("invalid address {input:?}: {reason}")]
    InvalidAddress { input: String, reason: String },
    #[error("invalid memory size {input:?}: {reason}")]
    InvalidMemory { input: String, reason: &'static str },
    #[error("internal port {port} is outside 25600-25999")]
    InvalidInternalPort { port: u16 },
    #[error("dns provider exec needs command = \"/path/to/hook\"")]
    ExecWithoutCommand,
    #[error("command = is only used with provider = \"exec\"")]
    CommandWithoutExec,
    #[error("{}: {message}", path.display())]
    ConfigSyntax { path: PathBuf, message: String },
    #[error("cannot write the config of {server}: {message}")]
    ConfigFormat { server: ServerName, message: String },
    #[error("{server}: {source}")]
    RouteConflict {
        server: ServerName,
        #[source]
        source: mcctl_router::Error,
    },
    #[error("internal port {port} is used by both {first} and {second}")]
    DuplicateInternalPort {
        port: u16,
        first: ServerName,
        second: ServerName,
    },
    #[error("address {address} is already used by {server}")]
    AddressTaken { address: String, server: ServerName },
    #[error("no server named {name}")]
    NoSuchServer { name: ServerName },
    #[error("{name} already exists: {}", what.display())]
    ServerExists { name: ServerName, what: PathBuf },
    #[error("user mc-{name} already exists")]
    UserExists { name: ServerName },
    #[error("user {user} does not exist")]
    UnknownUser { user: String },
    #[error("no free internal port in 25600-25999")]
    NoFreePort,
    #[error("another mcctl command is running (lock {})", path.display())]
    Locked { path: PathBuf },
    #[error("{action}: {source}")]
    Platform {
        action: String,
        #[source]
        source: mcctl_platform::Error,
    },
    #[error("{action}: {source}")]
    Dns {
        action: String,
        #[source]
        source: mcctl_dns::Error,
    },
    #[error("dns provider {provider} needs a token in /etc/mcctl/dns-token")]
    DnsTokenMissing { provider: &'static str },
    #[error("the porkbun dns token must look like pk1_...:sk1_...")]
    PorkbunToken,
    #[error("request to {url} failed: {reason}")]
    Http { url: String, reason: String },
    #[error("signature check of {file} failed: {reason}")]
    Signature { file: String, reason: String },
    #[error("SHA256SUMS has no entry for {file}")]
    ChecksumMissing { file: String },
    #[error("checksum mismatch for {file}")]
    ChecksumMismatch { file: String },
    #[error("self-update supports x86_64 and aarch64 linux, not {arch}")]
    UnsupportedArch { arch: String },
    #[error("unexpected release tag {tag:?}")]
    BadTag { tag: String },
    #[error("{name} is not running")]
    NotRunning { name: ServerName },
    #[error("the server command is too long ({len} bytes, at most {max})")]
    CommandTooLong { len: usize, max: usize },
    #[error("the server command must be a single line")]
    MultilineCommand,
    #[error("java {major} is not installed; run: sudo mcctl start {name}")]
    JavaMissing { major: u8, name: ServerName },
    #[error("{name} has jars in {folder}/ and mcctl cannot check mod compatibility")]
    ModsPresent {
        name: ServerName,
        folder: &'static str,
    },
    #[error("{name} runs minecraft {current}; {target} is older")]
    Downgrade {
        name: ServerName,
        current: String,
        target: String,
    },
    #[error("cannot tell whether minecraft {target} is newer than {current}")]
    UnknownOrder { current: String, target: String },
    #[error(
        "{name} runs forge {minecraft} from before the installer era, which mcctl cannot update"
    )]
    ForgeLegacy { name: ServerName, minecraft: String },
    #[error("confirmation needed: rerun with --yes")]
    ConfirmationNeeded,
    #[error("cancelled")]
    Cancelled,
    #[error("the minecraft eula was not accepted")]
    EulaDeclined,
    #[error("{failed} of {total} servers failed")]
    SomeFailed { failed: usize, total: usize },
    #[error("{count} doctor checks failed")]
    ChecksFailed { count: usize },
    #[error("cannot read the terminal: {0}")]
    Terminal(#[source] io::Error),
    #[error("cannot start the async runtime: {0}")]
    Runtime(#[source] io::Error),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { path, source }
    }

    pub(crate) fn platform(
        action: impl Into<String>,
    ) -> impl FnOnce(mcctl_platform::Error) -> Self {
        let action = action.into();
        move |source| Self::Platform { action, source }
    }

    pub(crate) fn dns(action: impl Into<String>) -> impl FnOnce(mcctl_dns::Error) -> Self {
        let action = action.into();
        move |source| Self::Dns { action, source }
    }

    /// A command that fixes or explains the problem, when there is one.
    pub(crate) fn hint(&self) -> Option<String> {
        match self {
            Self::NoSuchServer { .. } => Some("mcctl status".to_owned()),
            Self::ServerExists { name, .. } | Self::UserExists { name } => Some(format!(
                "pick another name, or remove the old one with: sudo mcctl delete {name}"
            )),
            Self::Locked { .. } => Some("wait for the other mcctl command to finish".to_owned()),
            Self::ModsPresent { name, .. } => Some(format!(
                "back up the server, then: sudo mcctl update {name} --force"
            )),
            Self::Downgrade { name, target, .. } => Some(format!(
                "back up the server, then: sudo mcctl update {name} {target} --force"
            )),
            Self::DnsTokenMissing { .. } | Self::PorkbunToken => {
                Some("sudo nano /etc/mcctl/dns-token && sudo systemctl restart mcctl".to_owned())
            }
            Self::ConfigSyntax { path, .. } => Some(format!("sudo nano {}", path.display())),
            Self::NotRunning { name } => Some(format!("sudo mcctl start {name}")),
            Self::ChecksFailed { .. } => Some("follow the fix lines above".to_owned()),
            Self::Platform { source, .. } if is_network(source) => {
                Some("check the network connection and try again".to_owned())
            }
            Self::Http { .. } => Some("check the network connection and try again".to_owned()),
            Self::NotRoot { .. }
            | Self::Io { .. }
            | Self::Spawn { .. }
            | Self::CommandFailed { .. }
            | Self::InvalidServerName { .. }
            | Self::InvalidHostname { .. }
            | Self::InvalidAddress { .. }
            | Self::InvalidMemory { .. }
            | Self::InvalidInternalPort { .. }
            | Self::ExecWithoutCommand
            | Self::CommandWithoutExec
            | Self::ConfigFormat { .. }
            | Self::RouteConflict { .. }
            | Self::DuplicateInternalPort { .. }
            | Self::AddressTaken { .. }
            | Self::UnknownUser { .. }
            | Self::NoFreePort
            | Self::Signature { .. }
            | Self::ChecksumMissing { .. }
            | Self::ChecksumMismatch { .. }
            | Self::UnsupportedArch { .. }
            | Self::BadTag { .. }
            | Self::CommandTooLong { .. }
            | Self::MultilineCommand
            | Self::JavaMissing { .. }
            | Self::UnknownOrder { .. }
            | Self::ForgeLegacy { .. }
            | Self::ConfirmationNeeded
            | Self::Cancelled
            | Self::EulaDeclined
            | Self::SomeFailed { .. }
            | Self::Terminal(_)
            | Self::Runtime(_)
            | Self::Platform { .. }
            | Self::Dns { .. } => None,
        }
    }
}

fn is_network(error: &mcctl_platform::Error) -> bool {
    use mcctl_platform::Error as Platform;
    match error {
        Platform::Http { .. } | Platform::Json { .. } => true,
        Platform::Io { .. }
        | Platform::NotAServer { .. }
        | Platform::Ambiguous { .. }
        | Platform::UnknownVersion { .. }
        | Platform::UnknownLoader { .. }
        | Platform::UnsupportedVersion { .. }
        | Platform::NoStableBuild { .. }
        | Platform::ChecksumMismatch { .. }
        | Platform::BadChecksumFile { .. }
        | Platform::InsecureUrl { .. }
        | Platform::UnsafePath { .. }
        | Platform::AlreadyExists { .. }
        | Platform::UnsafeArchive { .. }
        | Platform::ArchiveLayout { .. }
        | Platform::UnsupportedPlatform { .. }
        | Platform::NoJavaBuild { .. } => false,
    }
}
