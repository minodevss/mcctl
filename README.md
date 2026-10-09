# mcctl

Run Minecraft Java servers on one Linux machine. mcctl handles everything around the server: the address players type, DNS, Java, the server software, and the process. What happens inside the server is yours.

- One port for every server. Players join `survival.example.com` or `creative.example.com`; mcctl routes each connection by the address they typed.
- DNS records stay pointed at your public IP (Cloudflare, Porkbun, DuckDNS, or your own command).
- The right Java for each server, downloaded and verified.
- Vanilla, Fabric, Paper, NeoForge and Forge.
- Each server runs as a systemd service under its own user, starts at boot and restarts after a crash.

## Install

Linux with systemd, x86_64 or aarch64.

```sh
curl -fsSL https://github.com/minodevss/mcctl/releases/latest/download/install.sh | sudo sh
```

## Quick start

```sh
sudo mcctl new survival fabric 26.3 --address survival.example.com --memory 8G
sudo mcctl start survival
mcctl
```

Forward TCP port 25565 on your router to this machine, then run `mcctl doctor` to check the setup end to end.

## Commands

```
mcctl                                     Show servers, the router and DNS
mcctl new <server> [loader] [version]     Create a server (vanilla by default, newest version)
mcctl new <server> --from <dir>           Create a server from an existing folder
mcctl delete <server>                     Delete a server and its world
mcctl start | stop | restart <server>...  Stopped servers stay stopped across reboots
mcctl update <server> [version]           Update a server's software
mcctl update                              Update mcctl itself
mcctl console <server> [command]          Send a command, or open the console
mcctl logs <server> [-f]                  Show the server log
mcctl doctor                              Check ports, DNS and reachability
mcctl uninstall [--purge]                 Remove mcctl
```

## Configuration

Each server has one file, `/etc/mcctl/servers/<server>.toml`:

```toml
address = "survival.example.com"
memory = "8G"
internal-port = 25600
```

`address` is either a hostname routed on port 25565 or a dedicated port such as `":25566"`. Run `sudo mcctl restart <server>` after editing.

DNS is optional. Put the provider in `/etc/mcctl/mcctl.toml` and the token in `/etc/mcctl/dns-token` (mode 0600):

```toml
[dns]
provider = "cloudflare"
```

| Setup | Provider | Players type |
| --- | --- | --- |
| Your own domain on Cloudflare or Porkbun | `cloudflare` / `porkbun` | `survival.example.com` |
| DuckDNS | `duckdns` | `survival.you.duckdns.org` |
| Router-managed DDNS, one name | `none`, servers use ports | `you.example.net:25566` |
| Any other provider | `exec` with `command = "/path/to/hook"` | your names |

mcctl only changes records it created. A Cloudflare token needs `Zone.DNS: Edit` for the zone.

## Scope

mcctl does not manage worlds, mods, plugins, configs, whitelists or backups. Put mods in the server folder, manage players in game, and back up the machine with your usual tools.

## Build

```sh
cargo build --release -p mcctl
```

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at your option.
