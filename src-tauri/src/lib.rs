use tauri_plugin_shell::ShellExt;

#[tauri::command]
async fn sing_box_version(app: tauri::AppHandle) -> Result<String, String> {
    let output = app
        .shell()
        .sidecar("sing-box")
        .map_err(|e| format!("failed to resolve sing-box sidecar: {e}"))?
        .args(["version"])
        .output()
        .await
        .map_err(|e| format!("failed to run sing-box: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "sing-box exited with {:?}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().next().unwrap_or_default().trim().to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![sing_box_version])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
