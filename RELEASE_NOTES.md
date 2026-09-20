# Release notes

Draft bodies for GitHub releases — copy the section for the tag being published.
The release workflow (`.github/workflows/release.yml`) attaches build artifacts
automatically on `release: published`.

---

## v0.1.0 — first release

Initial public release of Megathrone, a Tauri v2 desktop proxy client: a sing-box
core for endpoints and subscriptions, a byedpi (ciadpi) tunnel for DPI
circumvention, and category-based routing that decides per site what goes through
the proxy, the DPI tunnel, or stays direct.

### What's inside

- **Profiles & subscriptions** — VLESS / VMess / Shadowsocks / Trojan / Hysteria2 /
  TUIC / SOCKS5 share links, Base64 subscription payloads and native sing-box
  configs; import from URL, file or pasted text; scheduled auto-refresh applies a
  diff, so surviving endpoints keep their latency history and selection
- **Availability scans** — every endpoint gets a real URL test through a throwaway
  sing-box instance; endpoints that pass are deep-probed against your routing
  categories' test rules
- **Automatic selection** — Round Robin, Fastest, Most Available and Manual modes
  with a background supervisor that re-checks on a schedule, rotates, and rescues
  a dead endpoint mid-scan
- **Live switching** — changing endpoints while connected is a selector flip over
  the Clash API: no reconnect, open connections drain gracefully
- **Session modes** — Off / System Proxy / TUN, plus a raw local proxy at
  `127.0.0.1:7890` that follows the selected endpoint
- **Routing** — URL categories (URL, domain suffix, domain, keyword, regex rules)
  with a per-category action: DPI tunnel, proxy or direct; edits apply to a
  running session without dropping it
- **DPI strategies** — byedpi sidecar with 61 preset strategies and a built-in
  benchmark that re-sorts the list by test results
- **Stats** — live traffic charts per outbound and a per-host request summary with
  ok/failed counts and the tunnel each host rode

### Downloads

| OS      | Artifact                                              |
| ------- | ----------------------------------------------------- |
| Windows | `Megathrone_0.1.0_x64-setup.exe` (NSIS)               |
| macOS   | `Megathrone_0.1.0_universal.dmg` (Apple Silicon + Intel) |
| Linux   | `.deb`, `.rpm`, `.AppImage`                           |
| Arch    | `megathrone-0.1.0-1-x86_64.pkg.tar.zst`, tarball      |

All bundles include the sing-box and byedpi sidecars — nothing is downloaded at
runtime.

### Notes

- **macOS:** builds are ad-hoc signed and not notarized. On first launch
  right-click the app and choose *Open*, or run
  `xattr -rd com.apple.quarantine /Applications/Megathrone.app`
- **Windows:** the installer is unsigned — SmartScreen will show *More info → Run
  anyway*
- TUN mode needs the privileges your OS requires for virtual interfaces
  (typically an elevated run)
- Profile data lives in the OS app-data dir (e.g.
  `~/Library/Application Support/com.megathrone.app/`); removing the app keeps it

### Known limitations

- No in-app auto-update yet — watch the releases page
- SSR links are not supported
- A DPI fallback action requires a selected strategy: without one, connecting with
  a DPI fallback is a hard error rather than silent misrouting
