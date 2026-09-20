<p align="center">
  <img src="app-icon.png" width="128" alt="Megathrone" />
</p>

# Megathrone

A desktop proxy client built on Tauri v2. A **sing-box** core handles endpoints and
subscriptions, a **byedpi** (ciadpi) tunnel handles DPI circumvention, and a
category-based router decides — per site — what goes through the proxy, what goes
through the DPI tunnel, and what stays direct.

## Features

**Profiles & subscriptions**

- Share links: VLESS, VMess, Shadowsocks, Trojan, Hysteria2, TUIC, SOCKS5; whole-payload
  Base64 subscriptions; native sing-box JSON configs (SSR is not supported)
- Import from URL, file, or pasted text; auto-refresh applies a **diff**, so surviving
  endpoints keep their latency history and the current selection
- Tolerates broken generators: empty/duplicate tags, mangled separators, junk uTLS
  fingerprints

**Availability & automatic selection**

- One-click scans run a real URL test through every endpoint (a throwaway sing-box
  instance with the Clash API — the same request a GUI would make)
- Deep probes: endpoints that pass the base test are additionally tested against the
  rules of your proxy categories; an availability floor keeps known-weak endpoints out
  of automatic picks
- Strategies: **Round Robin**, **Fastest**, **Most Available**, **Manual** — a
  background supervisor re-checks on schedule, rotates, and rescues a dead current
  endpoint mid-scan

**Session**

- Modes: Off / System Proxy / TUN
- Raw local proxy: a mixed inbound at `127.0.0.1:7890` (configurable) that follows the
  selected endpoint and its live switches — point any app with a proxy setting at it
- Switching endpoints while connected is a live selector flip — no reconnect, no port
  churn; open connections drain on the old endpoint
- Live traffic charts per outbound and a per-host request summary (ok/failed, with the
  tunnel each host rode)

**Routing**

- URL categories with typed rules: URL, domain suffix, domain, keyword, regex
- Per-category action — **DPI tunnel**, **proxy** or **direct** — plus a global
  fallback; changes apply to a running session without dropping it
- Default catalog seeded from ByeByeDPI's site lists

**DPI circumvention**

- byedpi (ciadpi) sidecar, started on demand only when routing actually needs it
- 61 named preset strategies (adapted from ByeByeDPI's `proxytest_strategies.list`)
- Strategy testing: every candidate tunnel is benchmarked against your test-marked
  rules and the list re-sorts by the results

## Downloads

Grab a bundle from [releases](../../releases):

| OS      | Artifact                                            |
| ------- | --------------------------------------------------- |
| Windows | NSIS installer (`.exe`)                             |
| macOS   | Universal `.dmg` (Apple Silicon + Intel)            |
| Linux   | `.deb`, `.rpm`, `.AppImage`, Arch `.pkg.tar.zst` / tarball |

All bundles ship the sing-box and byedpi sidecars — nothing is downloaded at runtime.

## Building from source

Prerequisites: Rust (stable), Node.js 22, pnpm 11.

```bash
pnpm install
pnpm tauri dev      # run the app
pnpm tauri build    # release bundle
```

`src-tauri/build.rs` fetches the sing-box/byedpi binaries for the cargo target triple
before every build (sha256-verified, cached in `src-tauri/binaries/`). The first build
per target needs network access; later builds are instant.

## Stack

Tauri 2 · SolidJS · Vite · Tailwind CSS 4 · DaisyUI 5 · Rust · SQLite · sing-box · byedpi

## Legal

This software is intended for circumventing censorship and network restrictions. You
are responsible for complying with the laws of your jurisdiction.

## License

[MIT](LICENSE). The bundled sing-box and byedpi sidecars are distributed under
their own licenses (GPL-3.0-or-later and MIT respectively) — see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
