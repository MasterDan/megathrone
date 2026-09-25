//! The system proxy integrations — macOS `networksetup`, the Windows
//! registry (WinINet), GNOME `gsettings`, KDE `kioslaverc` — behind their
//! cfg gates, kept together in one file so the per-platform surface stays
//! reviewable. The backup types are the on-disk format of the session
//! marker: field names and serde attributes are frozen.

use serde::{Deserialize, Serialize};

/// What one proxy kind (web / secure web / socks) looked like before we
/// touched it, so the user's own settings can be put back on disconnect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum ProxyBackup {
    Disabled,
    Enabled { host: String, port: u16 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct MacServiceBackup {
    pub(super) service: String,
    /// web, secure web, socks — aligned with `sysproxy::KINDS`
    pub(super) kinds: [ProxyBackup; 3],
}

/// The Windows system proxy state before the app touched it (HKCU
/// `Internet Settings`): each value is `None` when the key did not exist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct WindowsProxyBackup {
    enable: Option<u32>,
    server: Option<String>,
    /// a configured PAC URL wins over the manual proxy — cleared while ours
    /// runs, put back on restore
    auto_config_url: Option<String>,
}

/// One GNOME proxy key with its previous raw `gsettings get` value (fed
/// verbatim back to `gsettings set`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct GnomeProxySetting {
    schema: String,
    key: String,
    value: String,
}

/// One KDE `kioslaverc` `[Proxy Settings]` key with its previous value;
/// `None` — the key was absent (deleted again on restore).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct KdeProxySetting {
    key: String,
    value: Option<String>,
}

/// Everything needed to put the user's own system proxy settings back on
/// disconnect. Only the variant of the running OS is ever constructed; the
/// serde derives keep the foreign variants constructible on every target
/// (the session marker serializes them).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) enum SystemProxyBackup {
    MacOs(Vec<MacServiceBackup>),
    Windows(WindowsProxyBackup),
    Gnome(Vec<GnomeProxySetting>),
    Kde(Vec<KdeProxySetting>),
}

#[derive(Debug, Default, Clone)]
pub(super) enum SystemProxyRestore {
    #[default]
    Untouched,
    /// never constructed on platforms without a system proxy integration
    /// (android) — silence its dead-code lint
    #[cfg_attr(target_os = "android", allow(dead_code))]
    Applied(SystemProxyBackup),
}

/// Parses `-getwebproxy`-style output into the previous state; `None` when
/// the output cannot be understood (→ the service must not be touched).
#[cfg(target_os = "macos")]
fn parse_proxy_state(output: &str) -> Option<ProxyBackup> {
    let mut enabled: Option<bool> = None;
    let mut host: Option<String> = None;
    let mut port: Option<u16> = None;
    for line in output.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Enabled:") {
            enabled = Some(value.trim() == "Yes");
        } else if let Some(value) = line.strip_prefix("Server:") {
            host = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Port:") {
            port = value.trim().parse::<u16>().ok().filter(|port| *port != 0);
        }
    }
    match (enabled?, host?, port) {
        (false, _, _) => Some(ProxyBackup::Disabled),
        (true, host, Some(port)) if !host.is_empty() => Some(ProxyBackup::Enabled { host, port }),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
mod sysproxy {
    use super::{MacServiceBackup, ProxyBackup, SystemProxyBackup, parse_proxy_state};

    /// (get, set, set-state) for web, secure-web and socks proxies; the
    /// order is what `MacServiceBackup::kinds` is aligned with.
    const KINDS: [(&str, &str, &str); 3] = [
        ("-getwebproxy", "-setwebproxy", "-setwebproxystate"),
        ("-getsecurewebproxy", "-setsecurewebproxy", "-setsecurewebproxystate"),
        (
            "-getsocksfirewallproxy",
            "-setsocksfirewallproxy",
            "-setsocksfirewallproxystate",
        ),
    ];

    fn run(args: &[&str]) -> Result<String, String> {
        let output = std::process::Command::new("networksetup")
            .args(args)
            .output()
            .map_err(|error| format!("failed to run networksetup: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "networksetup {} failed: {}",
                args.first().unwrap_or(&""),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn list_services() -> Result<Vec<String>, String> {
        let output = run(&["-listallnetworkservices"])?;
        Ok(output
            .lines()
            .skip(1) // "An asterisk (*) denotes..." hint line
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('*')) // disabled services
            .map(str::to_string)
            .collect())
    }

    pub(super) fn enable(port: u16) -> Result<SystemProxyBackup, String> {
        let services = list_services()?;
        let host = "127.0.0.1";
        // networksetup is slow (~hundreds of ms per call) — configure the
        // services in parallel
        let backups: Vec<MacServiceBackup> = std::thread::scope(|scope| {
            let handles: Vec<_> = services
                .iter()
                .map(|service| scope.spawn(move || enable_service(service, host, port)))
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok().flatten())
                .collect()
        });
        if backups.is_empty() {
            return Err("failed to set the system proxy on any network service".to_string());
        }
        Ok(SystemProxyBackup::MacOs(backups))
    }

    /// Snapshots the service's current proxy state, then points all three
    /// proxy kinds at the local mixed port. `None` = do not touch (and do
    /// not restore) this service at all.
    fn enable_service(service: &str, host: &str, port: u16) -> Option<MacServiceBackup> {
        let mut kinds = Vec::with_capacity(KINDS.len());
        for (get, _, _) in KINDS {
            kinds.push(parse_proxy_state(&run(&[get, service]).ok()?)?);
        }
        let mut changed = false;
        for (_, set, _) in KINDS {
            if run(&[set, service, host, &port.to_string()]).is_ok() {
                changed = true;
            }
        }
        if !changed {
            return None;
        }
        let [web, secure, socks] = kinds.try_into().ok()?;
        Some(MacServiceBackup { service: service.to_string(), kinds: [web, secure, socks] })
    }

    pub(super) fn restore(backups: &[MacServiceBackup]) -> Result<(), String> {
        let errors: Vec<String> = std::thread::scope(|scope| {
            let handles: Vec<_> = backups
                .iter()
                .map(|backup| scope.spawn(move || restore_service(backup)))
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| match handle.join() {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error),
                    Err(_) => Some("restore worker panicked".to_string()),
                })
                .collect()
        });
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    fn restore_service(backup: &MacServiceBackup) -> Result<(), String> {
        for (index, (_, set, set_state)) in KINDS.iter().enumerate() {
            match &backup.kinds[index] {
                ProxyBackup::Disabled => run(&[set_state, &backup.service, "off"])?,
                ProxyBackup::Enabled { host, port } => {
                    run(&[set, &backup.service, host, &port.to_string()])?
                }
            };
        }
        Ok(())
    }
}

/// The Windows system proxy: the WinINet per-user settings in the registry
/// (`HKCU\…\Internet Settings`). Setting `ProxyEnable`/`ProxyServer` plus the
/// WinINet refresh broadcast is what every proxy tool on Windows does.
#[cfg(target_os = "windows")]
mod sysproxy {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    use super::{SystemProxyBackup, WindowsProxyBackup};

    const INTERNET_SETTINGS: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

    /// Makes WinINet (and everything honoring it — browsers included) pick
    /// the registry changes up immediately instead of on the next refresh.
    fn notify_wininet() {
        use windows_sys::Win32::Networking::WinInet::{
            InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
        };
        // SAFETY: both calls only broadcast the settings change — no
        // handles or buffers of ours are passed
        unsafe {
            InternetSetOptionW(
                std::ptr::null_mut(),
                INTERNET_OPTION_SETTINGS_CHANGED,
                std::ptr::null(),
                0,
            );
            InternetSetOptionW(
                std::ptr::null_mut(),
                INTERNET_OPTION_REFRESH,
                std::ptr::null(),
                0,
            );
        }
    }

    fn open_settings() -> Result<RegKey, String> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(INTERNET_SETTINGS, KEY_READ | KEY_WRITE)
            .map_err(|error| format!("failed to open the system proxy registry key: {error}"))
    }

    pub(super) fn enable(port: u16) -> Result<SystemProxyBackup, String> {
        let key = open_settings()?;
        let backup = WindowsProxyBackup {
            enable: key.get_value("ProxyEnable").ok(),
            server: key.get_value("ProxyServer").ok(),
            auto_config_url: key.get_value("AutoConfigURL").ok(),
        };

        // a configured PAC URL would win over the manual proxy — clear it
        // while ours runs (restored on disconnect)
        let _ = key.delete_value("AutoConfigURL");
        key.set_value("ProxyEnable", &1u32)
            .map_err(|error| format!("failed to enable the system proxy: {error}"))?;
        key.set_value("ProxyServer", &format!("127.0.0.1:{port}"))
            .map_err(|error| {
                format!("failed to point the system proxy at the local port: {error}")
            })?;
        notify_wininet();
        Ok(SystemProxyBackup::Windows(backup))
    }

    pub(super) fn restore(backup: &WindowsProxyBackup) -> Result<(), String> {
        let key = open_settings()?;
        // ProxyEnable goes last: until then the toggle still points at the
        // dead port either way, and a crash mid-restore leaves the user's
        // own server configured
        match &backup.server {
            Some(server) => key.set_value("ProxyServer", server),
            None => key.delete_value("ProxyServer").map(|_| ()),
        }
        .map_err(|error| format!("failed to restore the proxy server: {error}"))?;
        match &backup.auto_config_url {
            Some(url) => key.set_value("AutoConfigURL", url),
            None => key.delete_value("AutoConfigURL").map(|_| ()),
        }
        .map_err(|error| format!("failed to restore the PAC URL: {error}"))?;
        key.set_value("ProxyEnable", &backup.enable.unwrap_or(0))
            .map_err(|error| format!("failed to restore the proxy toggle: {error}"))?;
        notify_wininet();
        Ok(())
    }
}

/// The Linux system proxy: there is no single OS-wide setting — GNOME (and
/// every gsettings-flavoured desktop) reads `org.gnome.system.proxy`, KDE
/// reads `kioslaverc`. Both integrations snapshot the previous values and
/// put them back verbatim on restore.
#[cfg(target_os = "linux")]
mod sysproxy {
    use std::env;
    use std::process::Command;

    use super::{GnomeProxySetting, KdeProxySetting, SystemProxyBackup};

    const GNOME_SCHEMA: &str = "org.gnome.system.proxy";

    /// The (schema, key) pairs owned while connected: the master mode plus
    /// the http/https/socks endpoints (the mixed port speaks all of them).
    const GNOME_KEYS: [(&str, &str); 7] = [
        (GNOME_SCHEMA, "mode"),
        ("org.gnome.system.proxy.http", "host"),
        ("org.gnome.system.proxy.http", "port"),
        ("org.gnome.system.proxy.https", "host"),
        ("org.gnome.system.proxy.https", "port"),
        ("org.gnome.system.proxy.socks", "host"),
        ("org.gnome.system.proxy.socks", "port"),
    ];

    const KDE_GROUP: &str = "Proxy Settings";
    const KDE_KEYS: [&str; 4] = ["ProxyType", "httpProxy", "httpsProxy", "socksProxy"];

    fn run(command: &mut Command, what: &str) -> Result<String, String> {
        let output = command
            .output()
            .map_err(|error| format!("failed to run {what}: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "{what} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn gsettings(args: &[&str]) -> Result<String, String> {
        let mut command = Command::new("gsettings");
        command.args(args);
        run(&mut command, "gsettings")
    }

    /// The `kreadconfig`/`kwriteconfig` pair of the installed KDE
    /// generation (KDE 6 preferred over KDE 5).
    struct KdeTools {
        read: String,
        write: String,
    }

    fn kde_tools() -> Option<KdeTools> {
        ["6", "5"].iter().find_map(|generation| {
            let read = format!("kreadconfig{generation}");
            let mut probe = Command::new(&read);
            probe.arg("--version");
            probe.output().ok()?.status.success().then(|| KdeTools {
                write: format!("kwriteconfig{generation}"),
                read,
            })
        })
    }

    fn kde_read(tools: &KdeTools, key: &str) -> Result<String, String> {
        let mut command = Command::new(&tools.read);
        command.args(["--file", "kioslaverc", "--group", KDE_GROUP, "--key", key]);
        run(&mut command, "kreadconfig")
    }

    /// `None` deletes the key (back to its absent state). The value is a
    /// positional argument (no `--value` flag exists), `--notify` makes KDE
    /// apps pick the change up immediately.
    fn kde_write(tools: &KdeTools, key: &str, value: Option<&str>) -> Result<(), String> {
        let mut command = Command::new(&tools.write);
        command.args(["--file", "kioslaverc", "--group", KDE_GROUP, "--key", key, "--notify"]);
        match value {
            Some(value) => command.arg(value),
            None => command.arg("--delete"),
        };
        run(&mut command, "kwriteconfig").map(|_| ())
    }

    pub(super) fn enable(port: u16) -> Result<SystemProxyBackup, String> {
        let desktop = env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_uppercase();
        // KDE first by name: KDE boxes often carry the GNOME schemas through
        // GTK dependencies, where writing them would change nothing
        if !desktop.contains("KDE") && gnome_available() {
            return enable_gnome(port);
        }
        if let Some(tools) = kde_tools() {
            return enable_kde(port, &tools);
        }
        Err(
            "system proxy mode is not supported on this desktop environment \
             (GNOME and KDE are supported) — use the local proxy port instead"
                .to_string(),
        )
    }

    /// Whether the GNOME proxy schema answers (a `gsettings` binary alone
    /// proves nothing — it exists wherever GLib does).
    fn gnome_available() -> bool {
        gsettings(&["get", GNOME_SCHEMA, "mode"]).is_ok()
    }

    fn enable_gnome(port: u16) -> Result<SystemProxyBackup, String> {
        let backup: Vec<GnomeProxySetting> = GNOME_KEYS
            .iter()
            .map(|&(schema, key)| {
                Ok(GnomeProxySetting {
                    schema: schema.to_string(),
                    key: key.to_string(),
                    // the raw gvariant value — round-trips verbatim
                    value: gsettings(&["get", schema, key])?,
                })
            })
            .collect::<Result<_, String>>()
            .map_err(|error| format!("reading the GNOME proxy settings failed: {error}"))?;

        let port = port.to_string();
        let sets: [(&str, &str, &str); 7] = [
            (GNOME_SCHEMA, "mode", "'manual'"),
            ("org.gnome.system.proxy.http", "host", "'127.0.0.1'"),
            ("org.gnome.system.proxy.http", "port", &port),
            ("org.gnome.system.proxy.https", "host", "'127.0.0.1'"),
            ("org.gnome.system.proxy.https", "port", &port),
            ("org.gnome.system.proxy.socks", "host", "'127.0.0.1'"),
            ("org.gnome.system.proxy.socks", "port", &port),
        ];
        for (schema, key, value) in sets {
            if let Err(error) = gsettings(&["set", schema, key, value]) {
                let _ = restore_gnome(&backup);
                return Err(format!("failed to apply the GNOME proxy settings: {error}"));
            }
        }
        Ok(SystemProxyBackup::Gnome(backup))
    }

    fn restore_gnome(backup: &[GnomeProxySetting]) -> Result<(), String> {
        let errors: Vec<String> = backup
            .iter()
            .filter_map(|setting| {
                gsettings(&["set", &setting.schema, &setting.key, &setting.value]).err()
            })
            .collect();
        match errors.is_empty() {
            true => Ok(()),
            false => Err(errors.join("; ")),
        }
    }

    fn enable_kde(port: u16, tools: &KdeTools) -> Result<SystemProxyBackup, String> {
        let backup: Vec<KdeProxySetting> = KDE_KEYS
            .iter()
            .map(|&key| {
                // kreadconfig prints an empty string for an absent key
                let value = kde_read(tools, key)?;
                Ok(KdeProxySetting {
                    key: key.to_string(),
                    value: (!value.is_empty()).then_some(value),
                })
            })
            .collect::<Result<_, String>>()
            .map_err(|error| format!("reading the KDE proxy settings failed: {error}"))?;

        let sets: [(&str, String); 4] = [
            ("ProxyType", "1".to_string()), // 1 — manual proxies
            ("httpProxy", format!("http://127.0.0.1:{port}")),
            ("httpsProxy", format!("http://127.0.0.1:{port}")),
            ("socksProxy", format!("socks://127.0.0.1:{port}")),
        ];
        for (key, value) in sets {
            if let Err(error) = kde_write(tools, key, Some(&value)) {
                let _ = restore_kde(backup.as_slice(), tools);
                return Err(format!("failed to apply the KDE proxy settings: {error}"));
            }
        }
        Ok(SystemProxyBackup::Kde(backup))
    }

    fn restore_kde(backup: &[KdeProxySetting], tools: &KdeTools) -> Result<(), String> {
        let errors: Vec<String> = backup
            .iter()
            .filter_map(|setting| {
                kde_write(tools, &setting.key, setting.value.as_deref()).err()
            })
            .collect();
        match errors.is_empty() {
            true => Ok(()),
            false => Err(errors.join("; ")),
        }
    }

    pub(super) fn restore(backup: &SystemProxyBackup) -> Result<(), String> {
        match backup {
            SystemProxyBackup::Gnome(settings) => restore_gnome(settings),
            SystemProxyBackup::Kde(settings) => {
                let Some(tools) = kde_tools() else {
                    return Err(
                        "kreadconfig/kwriteconfig not found — cannot restore the KDE \
                         proxy settings"
                            .to_string(),
                    );
                };
                restore_kde(settings, &tools)
            }
            _ => Ok(()),
        }
    }
}

#[cfg(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux"
))]
pub(super) fn enable_system_proxy(port: u16) -> Result<SystemProxyRestore, String> {
    sysproxy::enable(port).map(SystemProxyRestore::Applied)
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux"
)))]
pub(super) fn enable_system_proxy(_port: u16) -> Result<SystemProxyRestore, String> {
    Err(
        "system proxy mode is not available on this platform — apps reach the proxy \
         through the local port instead"
            .to_string(),
    )
}

#[cfg(target_os = "macos")]
pub(super) fn restore_system_proxy(backup: &SystemProxyBackup) -> Result<(), String> {
    match backup {
        SystemProxyBackup::MacOs(backups) => sysproxy::restore(backups),
        _ => Ok(()),
    }
}

#[cfg(target_os = "windows")]
pub(super) fn restore_system_proxy(backup: &SystemProxyBackup) -> Result<(), String> {
    match backup {
        SystemProxyBackup::Windows(backup) => sysproxy::restore(backup),
        _ => Ok(()),
    }
}

#[cfg(target_os = "linux")]
pub(super) fn restore_system_proxy(backup: &SystemProxyBackup) -> Result<(), String> {
    sysproxy::restore(backup)
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux"
)))]
pub(super) fn restore_system_proxy(_backup: &SystemProxyBackup) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn parse_proxy_state_handles_enabled_and_disabled() {
        let disabled = "\
Enabled: No
Server:
Port: 0
Authenticated Proxy: 0";
        assert_eq!(parse_proxy_state(disabled), Some(ProxyBackup::Disabled));

        let enabled = "\
Enabled: Yes
Server: 10.0.0.2
Port: 3128
Authenticated Proxy: 0";
        assert_eq!(
            parse_proxy_state(enabled),
            Some(ProxyBackup::Enabled { host: "10.0.0.2".to_string(), port: 3128 })
        );

        // enabled but unparseable host/port → unknown, must not be touched
        let broken = "Enabled: Yes\nServer: \nPort: applesauce";
        assert_eq!(parse_proxy_state(broken), None);

        // garbage output → unknown
        assert_eq!(parse_proxy_state("not networksetup output"), None);
    }
}
