# Changelog

All notable changes to this project are documented in this file.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-10-09

### Added
- Hostname router on port 25565 with dedicated-port routes and an offline reply for stopped servers.
- DNS record sync for Cloudflare, Porkbun, DuckDNS and command hooks, with public IP detection.
- Server creation for vanilla, Fabric, Paper, NeoForge and Forge, or from an existing folder.
- Automatic Java installation per server.
- systemd lifecycle with one user per server: start, stop, restart, console and logs.
- `update` for server software and for mcctl itself, verified with minisign.
- `doctor` for ports, DNS and reachability checks.

[Unreleased]: https://github.com/minodevss/mcctl/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/minodevss/mcctl/releases/tag/v0.1.0
