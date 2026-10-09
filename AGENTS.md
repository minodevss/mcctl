# mcctl

Runs Minecraft Java servers on one Linux host: hostname router on :25565, DNS records, Java and server-software install, systemd lifecycle.
Scope is outside the server only. Never touch worlds, mods, plugins, whitelist, server.properties, or backups.

## Commands
- `cargo fmt --all`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test -p <crate>` while working; `cargo test --workspace --locked` before finishing.
- All must pass before a commit. Never assume a failure is pre-existing.

## Architecture
The crate graph is the architecture. Do not add edges.
- `mcctl-protocol`: Minecraft wire codec. Pure: no I/O, no async, std only.
- `mcctl-router`: TCP router (tokio). Uses `mcctl-protocol`.
- `mcctl-dns`: DNS provider updates and public IP check.
- `mcctl-platform`: server detection, launch arguments, server-software and Java install.
- `mcctl`: the binary (cli, config, systemd, serve, run, doctor, update, output). The only crate that depends on the others.

Decide in pure functions (values in, values out); keep I/O wrappers thin.

## Code
- Small functions; names say what they do. Prefer a better name or type over a comment.
- Parse input once into newtypes (`ServerName`, `Hostname`) and pass those inward.
- One `thiserror` enum per crate; variants carry context (path, server, url). Messages lowercase, no trailing period.
- Only `crates/mcctl/src/output.rs` writes to stdout or stderr.
- `match` on our own enums without a `_` arm.
- Silence a lint only with `#[expect(lint, reason = "...")]`.
- English only.

## Comments
Default to none. Add one only for what code cannot say: a protocol quirk, an outside constraint, a hidden invariant, or why the obvious way is wrong. One line, two at most.
- Good: `// Forge appends "\0FML3\0" to the host; route on the part before the first NUL.`
- Good: `// disable --now so a stopped server stays stopped after reboot.`
- Bad: `// read packet length` above `read_varint(buf)?`
- Bad: `// fixed bug from review`
- Each `lib.rs` opens with a 1-2 line `//!`: the crate's role and what it must not do.
- `///` only when name and types don't give the contract (units, invariants, when it errors).
- No commented-out code. Never mention the chat, prompt, or agent.

## Tests
- Tests are the spec; name them as behavior: `strips_forge_marker_from_host`.
- Pure functions get table tests.
- Bug fix: write the failing test first.
- Fixtures live in `crates/<crate>/tests/fixtures/`. No network in tests.

## Dependencies
- Prefer std and existing deps. A new dep needs a one-line reason in the PR.
- Versions live in root `[workspace.dependencies]` with default features off where possible.
- rustls only. Do not add `anyhow`, `reqwest`, `tracing`, `zbus`.

## Commits
- Conventional Commits with the crate as scope: `fix(router): strip trailing dot from host`.
- One logical change per commit.

## Never
- New commands, flags, or config keys without an approved issue.
- Root writes inside a server folder; use the server's own user.
- Printing or logging the DNS token.
- Unrelated edits, or skipping a failing test.
