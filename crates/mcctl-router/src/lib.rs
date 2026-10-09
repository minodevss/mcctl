//! TCP router that sends each Minecraft connection to the server named by its handshake host.
//! Never inspects traffic after the handshake.

mod conn;
mod context;
mod error;
mod event;
mod limit;
mod listeners;
mod routes;
mod settings;
mod stats;
mod supervisor;

pub use error::Error;
pub use event::{Event, RejectReason};
pub use routes::{Backend, RouteTable};
pub use settings::Settings;
pub use stats::Stats;
pub use supervisor::run;
