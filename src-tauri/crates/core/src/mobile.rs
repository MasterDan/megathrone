//! The Android integration layer: the foreground service keeping the proxy
//! session alive while the app is backgrounded, and the (desktop no-op)
//! surface the connection flow calls into.
//!
//! Android kills background processes within minutes; a long-lived sing-box
//! session therefore needs a foreground service with a notification. The
//! service itself is Kotlin (`ProxyService` in `gen/android`); this module
//! registers a tiny Kotlin plugin (`ProxyServicePlugin`) over Tauri's mobile
//! plugin bridge and forwards start/stop commands to it. On desktop every
//! call is a no-op.

/// Starts/stops the Android foreground service bound to the proxy session.
/// No-op anywhere else; never blocks the connect flow on failure — the
/// session works while the app stays in the foreground either way.
pub fn set_foreground_service(running: bool) {
    #[cfg(target_os = "android")]
    {
        android::set_foreground(running);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = running;
    }
}

#[cfg(target_os = "android")]
pub use android::service_plugin;

#[cfg(target_os = "android")]
mod android {
    use std::sync::OnceLock;
    use tauri::plugin::{Builder, PluginHandle, TauriPlugin};

    /// Name both sides register under (`run_mobile_plugin` dispatch key).
    const PLUGIN_NAME: &str = "megathrone-service";

    /// The Kotlin plugin handle, filled once at setup.
    static SERVICE: OnceLock<PluginHandle<tauri::Wry>> = OnceLock::new();

    /// The Tauri plugin that registers the Kotlin-side `ProxyServicePlugin`.
    /// Added to the builder on Android only.
    pub fn service_plugin() -> TauriPlugin<tauri::Wry> {
        Builder::new(PLUGIN_NAME)
            .setup(|_app, api| {
                let handle = api
                    .register_android_plugin("com.megathrone.app", "ProxyServicePlugin")
                    .map_err(|error| {
                        format!("failed to register the proxy service plugin: {error}")
                    })?;
                let _ = SERVICE.set(handle);
                Ok(())
            })
            .build()
    }

    pub fn set_foreground(running: bool) {
        if let Some(handle) = SERVICE.get() {
            if let Err(error) = handle.run_mobile_plugin::<serde_json::Value>(
                "setRunning",
                serde_json::json!({ "running": running }),
            ) {
                eprintln!("foreground service call failed: {error}");
            }
        }
    }
}
