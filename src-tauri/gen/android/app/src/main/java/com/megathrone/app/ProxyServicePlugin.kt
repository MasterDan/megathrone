package com.megathrone.app

import android.app.Activity
import android.content.Intent
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin

@InvokeArg
class ServiceArgs {
    var running: Boolean = false
}

/**
 * Bridge between the Rust session lifecycle (`mobile.rs` → `run_mobile_plugin`)
 * and [ProxyService]: starts the foreground service while the proxy session
 * runs, stops it when the session ends.
 */
@TauriPlugin
class ProxyServicePlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun setRunning(invoke: Invoke) {
        val args = invoke.parseArgs(ServiceArgs::class.java)
        val intent = Intent(activity, ProxyService::class.java)
        if (args.running) {
            ContextCompat.startForegroundService(activity, intent)
        } else {
            activity.stopService(intent)
        }
        invoke.resolve()
    }
}
