//! Detects, installs and launches Minecraft server software and the Java it needs.
//! Never edits files the server owns after install, and never follows symlinks inside a server folder.

mod detect;
mod download;
mod error;
mod extract;
mod http;
mod install;
mod jar;
mod java;
mod launch;
mod minecraft;
mod nofollow;
mod scan;

pub use detect::{Detected, Entry, Platform, detect};
pub use download::download;
pub use error::Error;
pub use install::{Artifact, Digest, InstallerStep, Loader, Plan, installer_command, resolve};
pub use java::{ensure_java, java_path};
pub use launch::{Launch, command_line};
pub use minecraft::java_major;
