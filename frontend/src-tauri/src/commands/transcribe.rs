use tauri::command;
use std::process::{Command, Stdio};
use tauri::Manager;
use crate::state::ActiveSidecar;

#[command]
pub async fn transcribe_clip(
    state: tauri::State<'_, ActiveSidecar>,
    app_handle: tauri::AppHandle,
    clip_id: String,
    video_path: String,
    start: f64,
    end: f64,
    source_lang: String,
    target_lang: String,
) -> Result<String, String> {
    // Locate the sidecar executable (same as used for scene detection)
    let sidecar_path = if cfg!(target_os = "windows") {
        "bin/backend_script-x86_64-pc-windows-msvc/backend_script.exe"
    } else {
        "bin/backend_script-x86_64-pc-windows-msvc/backend_script"
    };
    let exe_path = app_handle.path().resource_dir().unwrap().join(sidecar_path);
    if !exe_path.exists() {
        return Err(format!("Sidecar not found at {:?}", exe_path));
    }

    // Spawn the process
    let mut child = Command::new(exe_path)
        .arg("transcribe")
        .arg(&video_path)
        .arg(start.to_string())
        .arg(end.to_string())
        .arg(source_lang)
        .arg(target_lang)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start transcription: {}", e))?;

    let output = child.wait_with_output().map_err(|e| format!("Failed to read output: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Transcription failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).map_err(|e| format!("Invalid JSON: {}", e))?;
    if let Some(err) = json.get("error") {
        return Err(err.as_str().unwrap_or("Unknown error").to_string());
    }
    let text = json["text"].as_str().unwrap_or("").to_string();
    Ok(text)
}