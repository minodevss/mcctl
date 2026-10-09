#!/bin/sh
set -eu

repo="minodevss/mcctl"
unit_dir="/usr/local/lib/systemd/system"

say() {
    printf 'mcctl: %s\n' "$1"
}

fail() {
    printf 'mcctl: error: %s\n' "$1" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

target_triple() {
    case "$(uname -m)" in
        x86_64 | amd64) echo "x86_64-unknown-linux-musl" ;;
        aarch64 | arm64) echo "aarch64-unknown-linux-musl" ;;
        *) fail "unsupported architecture: $(uname -m)" ;;
    esac
}

release_url() {
    if [ -n "${MCCTL_VERSION:-}" ]; then
        echo "https://github.com/${repo}/releases/download/${MCCTL_VERSION}"
    else
        echo "https://github.com/${repo}/releases/latest/download"
    fi
}

ensure_user() {
    getent group mcctl >/dev/null || groupadd --system mcctl
    getent passwd mcctl >/dev/null || useradd --system --gid mcctl --home-dir /nonexistent --no-create-home --shell /usr/sbin/nologin mcctl
}

ensure_layout() {
    install -d -m 0755 /etc/mcctl /etc/mcctl/servers /var/lib/mcctl /var/lib/mcctl/servers /var/lib/mcctl/java "${unit_dir}"
    [ -e /etc/mcctl/dns-token ] || install -m 0600 /dev/null /etc/mcctl/dns-token
}

main() {
    [ "$(id -u)" -eq 0 ] || fail "run as root: curl -fsSL https://github.com/${repo}/releases/latest/download/install.sh | sudo sh"
    [ "$(uname -s)" = "Linux" ] || fail "mcctl runs on Linux only"
    need curl
    need sha256sum
    need systemctl
    need useradd
    [ -d /run/systemd/system ] || fail "systemd is not running"

    target="$(target_triple)"
    base="$(release_url)"
    tmp="$(mktemp -d)"
    trap 'rm -rf "${tmp}"' EXIT

    say "downloading ${target}"
    for file in "mcctl-${target}" SHA256SUMS SHA256SUMS.minisig mcctl.service mc@.service; do
        curl -fsSL --proto '=https' --tlsv1.2 -o "${tmp}/${file}" "${base}/${file}"
    done

    (cd "${tmp}" && grep -E " (mcctl-${target}|mcctl.service|mc@.service)$" SHA256SUMS | sha256sum -c --quiet -) || fail "checksum mismatch"
    if command -v minisign >/dev/null 2>&1; then
        minisign -V -q -P "RWRQWwCQGpxyfYQsg66tP0ow+D77z9iGy/Z002ACeLOapSoviRUBeXlW" -m "${tmp}/SHA256SUMS" || fail "signature mismatch"
    fi

    ensure_user
    ensure_layout
    install -m 0755 "${tmp}/mcctl-${target}" /usr/local/bin/mcctl
    install -m 0644 "${tmp}/mcctl.service" "${tmp}/mc@.service" "${unit_dir}/"
    systemctl daemon-reload
    systemctl enable --now mcctl.service

    if [ -n "${SUDO_USER:-}" ] && [ "${SUDO_USER}" != "root" ]; then
        usermod -a -G mcctl "${SUDO_USER}"
    fi

    say "installed $(/usr/local/bin/mcctl --version)"
    say "next: sudo mcctl new <name> fabric --address <host>"
}

main "$@"
