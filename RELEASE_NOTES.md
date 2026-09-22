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

---

## v0.2.0 — Android & a new desktop UI

The first Android build ships, and the desktop interface gets a real layout: a
sidebar, page-level scroll zones and a bottom activity bar. Under the hood the
sidecar plumbing is now shared code between desktop and Android.

### What's inside

- **Android app** — an arm64 APK with sing-box and byedpi bundled inside the
  package as native libraries (Android forbids exec'ing binaries from the app
  data dir, so they ride the APK's `nativeLibraryDir` instead); byedpi is
  compiled from source with the NDK — upstream publishes no Android builds
- **Foreground service** — while connected, a `dataSync` foreground service
  keeps the proxy alive when the app goes to the background
- **Stable APK signature** — the APK is signed with a committed keystore, so
  sideloaded updates install over previous builds
- **Desktop layout rework** — a sidebar where the brand crown is the connect
  control and the profile list carries per-row toggles (switching the live
  session without a restart); pages render in their own scroll zones; a bottom
  activity bar tracks every running job — latency scans, the DPI strategy
  test, profile updates — with progress and stop controls
- **Traffic charts on the dashboard** — live per-outbound charts pin under the
  endpoint grid while connected; clicking them opens the Stats page
- **DPI strategies get their own page** — `/dpi` on desktop (legacy
  `/settings?tab=dpi` links redirect); Settings keeps General and Routing
- **One codebase, two interfaces** — the UI variant (desktop vs mobile) is
  resolved at build time; `?ui=mobile` forces the mobile layout at runtime for
  debugging

### Under the hood

- The sidecar spawn layer moved in-house (`src-tauri/src/process.rs`): a
  faithful subset of the old shell-plugin sidecar API over `shared_child`, used
  identically on desktop and Android — `tauri-plugin-shell` is gone from the
  project
- byedpi is compiled from source on macOS as well (sha256-pinned tarball),
  since upstream publishes no darwin builds; release-profile macOS builds lipo
  both arch slices into universal binaries
- `externalBin` moved into per-platform config overlays
  (`tauri.{macos,linux,windows}.conf.json`) so mobile builds never see it
- CI gained a `build-android` job that builds and signs the APK; the Arch
  package version is sanitized for non-tag workflow runs

### Downloads

| OS      | Artifact                                                 |
| ------- | -------------------------------------------------------- |
| Windows | `Megathrone_0.2.0_x64-setup.exe` (NSIS)                  |
| macOS   | `Megathrone_0.2.0_universal.dmg` (Apple Silicon + Intel)  |
| Linux   | `.deb`, `.rpm`, `.AppImage`                              |
| Arch    | `megathrone-0.2.0-1-x86_64.pkg.tar.zst`, tarball         |
| Android | `megathrone-v0.2.0-android-arm64.apk`                    |

All bundles include the sing-box and byedpi sidecars — nothing is downloaded at
runtime.

### Notes

- **Android:** sideload the APK — the committed keystore keeps the signature
  stable, so future releases install on top. Apps with a built-in proxy setting
  can point at the raw local proxy (`127.0.0.1:7890` by default) to follow the
  selected endpoint; device-wide capture is not available yet
- macOS and Windows notes are unchanged from v0.1.0 (ad-hoc signed, not
  notarized / unsigned installer — SmartScreen)

### Known limitations

- TUN mode is not available on Android — a VpnService-based tunnel is future
  work; a stored TUN selection degrades to system-proxy
- System-proxy wiring is macOS-only (a no-op elsewhere)
- SSR links are not supported
- A DPI fallback action requires a selected strategy: without one, connecting
  with a DPI fallback is a hard error rather than silent misrouting
- No in-app auto-update yet — watch the releases page
