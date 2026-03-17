// #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // removes the cmd line on exe

// Tauri backend entrypoint.
//
// Frontend-facing commands:
// - detect_scenes: runs the Python/packaged backend to generate clips + thumbnails
// - export_clips: merges (concat demuxer) or multi-exports clips (filesystem copy)

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::path::{PathBuf, Path};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex as AsyncMutex;

use tauri::{AppHandle, Manager, State};
use tauri::Emitter;
use serde::Serialize;

#[derive(Serialize, Clone)]
struct ProgressPayload {
    percent: u8,
    message: String,
}

// --------------------
// Preview proxy locking
// --------------------

#[derive(Default)]
struct PreviewProxyLocks {
    // One async mutex per clip path.
    // Prevents concurrent encodes of the same preview proxy (which can produce partial files).
    inner: AsyncMutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

// --------------------
// Codec check (HEVC / H.265)
// --------------------

#[tauri::command]
async fn check_hevc(app: AppHandle, video_path: String) -> Result<bool, String> {
    if video_path.trim().is_empty() {
        return Err("video_path is empty".to_string());
    }

    let ffprobe = resolve_bundled_tool(&app, "ffprobe")?;

    let ffprobe_output = tokio::task::spawn_blocking(move || {
        Command::new(&ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name",
                "-of",
                "default=nk=1:nw=1",
                &video_path,
            ])
            .output()
            .map_err(|e| format!("Failed to run ffprobe ({}): {e}", ffprobe.display()))
    })
    .await
    .map_err(|e| format!("ffprobe task panicked: {e}"))??;

    if !ffprobe_output.status.success() {
        let stderr = String::from_utf8_lossy(&ffprobe_output.stderr)
            .trim()
            .to_string();
        return Err(if stderr.is_empty() {
            "ffprobe failed".to_string()
        } else {
            format!("ffprobe failed: {stderr}")
        });
    }

    let codec = String::from_utf8_lossy(&ffprobe_output.stdout)
        .trim()
        .to_ascii_lowercase();

    Ok(codec == "hevc")
}

// --------------------
// Scene detection (clips + thumbs)
// --------------------

#[tauri::command]
async fn detect_scenes(
    app: AppHandle,
    video_path: String,
) -> Result<String, String> {
    println!("detect_scenes called");
    let output_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;

    std::fs::create_dir_all(&output_dir)
        .map_err(|e| e.to_string())?;

    if let Ok(entries) = std::fs::read_dir(&output_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    let output_dir_str = output_dir.to_string_lossy().to_string();

    let mut child = if cfg!(debug_assertions) {
        // DEV MODE → run python script from /backend using the local venv
        let mut root = std::env::current_dir().map_err(|e| e.to_string())?;
        root.pop();
        root.pop();

        let script_path = root.join("backend").join("backend_script.py");
        let python_path = root
            .join("backend")
            .join("venv")
            .join("Scripts")
            .join("python.exe");

        Command::new(python_path)
            .arg(script_path)
            .arg(&video_path)
            .arg(&output_dir_str)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn python: {e}"))?

    } else {
        // PRODUCTION → run bundled backend exe from resources
        let exe_dir = std::env::current_exe()
            .map_err(|e| format!("Can't find current exe: {e}"))?
            .parent()
            .ok_or("Can't get exe directory")?
            .to_path_buf();

            
        let backend = app
            .path()
            .resolve(
                "bin/backend_script-x86_64-pc-windows-msvc/backend_script.exe", 
                tauri::path::BaseDirectory::Resource)
            .map_err(|e| e.to_string())?;

        println!("Backend path: {:?}", backend);
        println!("Backend exists: {}", backend.exists());
        Command::new(backend)
            .current_dir(&exe_dir)
            .arg(&video_path)
            .arg(&output_dir_str)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn backend exe: {e}"))?
    };

    let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
    let stderr = child.stderr.take().ok_or("Failed to capture stderr")?;

    let stderr_accum = Arc::new(Mutex::new(String::new()));
    let app_for_thread = app.clone();
    let stderr_accum_for_thread = Arc::clone(&stderr_accum);

    let stderr_handle = tokio::task::spawn_blocking(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines().flatten() {
            if line.starts_with("TIMING|") {
                println!("{line}");
            }
            if let Ok(mut buf) = stderr_accum_for_thread.lock() {
                buf.push_str(&line);
                buf.push('\n');
            }

            if let Some(rest) = line.strip_prefix("PROGRESS|") {
                let mut parts = rest.splitn(2, '|');
                let p_str = parts.next().unwrap_or("");
                let msg = parts.next().unwrap_or("").to_string();

                if let Ok(p) = p_str.parse::<u8>() {
                    let _ = app_for_thread.emit(
                        "scene_progress",
                        ProgressPayload { percent: p, message: msg },
                    );
                }
            }
        }
    });

    let stdout_string = tokio::task::spawn_blocking(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = String::new();
        reader.read_to_string(&mut buf).map(|_| buf)
    })
    .await
    .map_err(|e| format!("stdout thread panicked: {e}"))?
    .map_err(|e| format!("Failed reading stdout: {e}"))?;

    let _ = stderr_handle.await;

    let status = tokio::task::spawn_blocking(move || child.wait())
        .await
        .map_err(|e| format!("wait thread panicked: {e}"))?
        .map_err(|e| format!("Failed waiting for python: {e}"))?;

    if !status.success() {
        let err = stderr_accum
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| "Python failed (stderr lock poisoned)".to_string());
        return Err(err);
    }

    Ok(stdout_string)
}

// --------------------
// Export (merge or copy)
// --------------------

#[tauri::command]
async fn export_clips(
    app: AppHandle,
    clips: Vec<String>,
    save_path: String,
    merge_enabled: bool,
) -> Result<(), String> {

    if clips.is_empty() {
        return Ok(());
    }


    // Export uses FFmpeg but does not re-encode.
    // - merge_enabled: concat demuxer + stream copy
    // - else: plain filesystem copies with consistent naming
    let ffmpeg = resolve_bundled_tool(&app, "ffmpeg")?;


    let save_path = PathBuf::from(&save_path);

    if merge_enabled {
        // ---------------- MERGE ----------------

        fn escape_concat_path(path: &str) -> String {
            // FFmpeg concat demuxer is happier with forward slashes on Windows.
            // Also escape single quotes when using the `file '...'
            path.replace('\\', "/").replace('\'', "\\'")
        }

        let list_path = {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_millis();
            std::env::temp_dir().join(format!(
                "amverge_concat_{}_{}.txt",
                std::process::id(),
                ts
            ))
        };

        let mut list_content = String::new();
        for clip in &clips {
            let escaped = escape_concat_path(clip);
            list_content.push_str(&format!("file '{}'\n", escaped));
        }

        std::fs::write(&list_path, list_content).map_err(|e| e.to_string())?;

        let output = Command::new(&ffmpeg)
            .args([
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
                list_path
                    .to_str()
                    .ok_or("Invalid concat list path")?,
                // Normalize timestamps and re-encode for maximum compatibility (e.g., After Effects).
                "-fflags",
                "+genpts",
                "-avoid_negative_ts",
                "make_zero",
                // Video: widely-compatible H.264
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-profile:v",
                "high",
                "-level",
                "4.1",
                // Keep quality high; encoding is the cost we pay for reliability.
                "-preset",
                "medium",
                "-crf",
                "18",
                // Audio: AAC stereo
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-ar",
                "48000",
                "-ac",
                "2",
                // Streamable MP4
                "-movflags",
                "+faststart",
                // Avoid rare muxing queue overflows on tricky inputs.
                "-max_muxing_queue_size",
                "1024",
                save_path
                    .to_str()
                    .ok_or("Invalid output path")?,
            ])
            .output()
            .map_err(|e| {
                format!(
                    "Failed to run ffmpeg ({}): {}",
                    ffmpeg.display(),
                    e
                )
            })?;

        let _ = std::fs::remove_file(&list_path);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                return Err("FFmpeg merge failed".into());
            }
            return Err(format!("FFmpeg merge failed: {}", stderr));
        }

    } else {
        // ---------------- MULTIPLE EXPORT ----------------

        // In merge-disabled mode, the frontend passes a *file path* chosen via a Save dialog.
        // We treat it as a naming template: <user_stem>_<clip_code>.<ext>
        let destination_dir = save_path.parent().ok_or("Invalid save path")?;
        if !destination_dir.exists() {
            std::fs::create_dir_all(destination_dir).map_err(|e| e.to_string())?;
        }

        let user_stem = save_path
            .file_stem()
            .ok_or("Invalid filename")?
            .to_string_lossy();

        let ext = save_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mp4");

        for (i, clip) in clips.iter().enumerate() {
            let clip_path = Path::new(clip);
            let clip_stem = clip_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");

            let clip_code = clip_stem
                .rsplit('_')
                .next()
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| "0000");

            // If the code isn't purely digits (unexpected naming), fall back to index.
            let code = if clip_code.chars().all(|c| c.is_ascii_digit()) {
                clip_code.to_string()
            } else {
                format!("{:04}", i)
            };

            let new_filename = format!("{}_{}.{}", user_stem, code, ext);
            let destination = destination_dir.join(new_filename);

            std::fs::copy(clip_path, destination).map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

// --------------------
// Preview error signal (grid hover)
// --------------------

#[tauri::command]
async fn hover_preview_error(
    clip_id: String,
    clip_path: String,
    error_code: Option<u16>,
) -> Result<(), String> {
    // Minimal implementation: just log. The frontend uses this to detect
    // unsupported codecs (e.g., HEVC) and we will add proxy generation later.
    let clip_id = clip_id.replace('\n', " ").replace('\r', " ");
    let clip_path = clip_path.replace('\n', " ").replace('\r', " ");
    println!(
        "hover_preview_error|clip_id={}|clip_path={}|error_code={:?}",
        clip_id, clip_path, error_code
    );

    Ok(())
}

#[tauri::command]
async fn ensure_preview_proxy(
    app: AppHandle,
    proxy_locks: State<'_, PreviewProxyLocks>,
    clip_path: String,
) -> Result<String, String> {
    // Serialize proxy generation per clip to avoid partially-written proxies being served.
    let clip_key = clip_path.clone();
    let clip_lock = {
        let mut map = proxy_locks.inner.lock().await;
        map.entry(clip_key)
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    };
    let _guard = clip_lock.lock().await;

    let ffmpeg = resolve_bundled_tool(&app, "ffmpeg")?;
    println!("ensure_preview_proxy|ffmpeg={}", ffmpeg.display());

    let input_path = PathBuf::from(&clip_path);
    if !input_path.exists() {
        return Err(format!("Clip not found: {}", input_path.display()));
    }

    let parent = input_path
        .parent()
        .ok_or("Invalid clip path (no parent directory)")?;

    let stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("Invalid clip filename")?;

    let proxy_path = parent.join(format!("{stem}.preview.mp4"));
    let proxy_tmp_path = parent.join(format!("{stem}.preview.tmp.mp4"));

    // If proxy already exists and is non-empty, reuse it.
    if let Ok(meta) = std::fs::metadata(&proxy_path) {
        if meta.is_file() && meta.len() > 0 {
            return Ok(proxy_path.to_string_lossy().to_string());
        }
    }

    // Clean up any stale temp file from a previous failed/aborted run.
    let _ = std::fs::remove_file(&proxy_tmp_path);

    // Run FFmpeg in a blocking task.
    let ffmpeg_clone = ffmpeg.clone();
    let input = input_path.clone();
    let output = proxy_tmp_path.clone();

    let ffmpeg_output = tokio::task::spawn_blocking(move || {
        Command::new(&ffmpeg_clone)
            .args([
                "-y",
                "-i",
                input
                    .to_str()
                    .ok_or_else(|| "Invalid input path".to_string())?,
                // Map video and optional audio.
                "-map",
                "0:v:0",
                "-map",
                "0:a?",
                // Video: H.264
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "28",
                "-pix_fmt",
                "yuv420p",
                // Audio: AAC (best HTML5 compatibility)
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                // Make MP4 streamable
                "-movflags",
                "+faststart",
                output
                    .to_str()
                    .ok_or_else(|| "Invalid output path".to_string())?,
            ])
            .output()
            .map_err(|e| format!("Failed to run ffmpeg: {e}"))
    })
    .await
    .map_err(|e| format!("ffmpeg task panicked: {e}"))??;

    if !ffmpeg_output.status.success() {
        let _ = std::fs::remove_file(&proxy_tmp_path);
        let stderr = String::from_utf8_lossy(&ffmpeg_output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "FFmpeg proxy encode failed".to_string()
        } else {
            format!("FFmpeg proxy encode failed: {stderr}")
        });
    }

    // Verify tmp proxy exists.
    let meta = std::fs::metadata(&proxy_tmp_path).map_err(|e| e.to_string())?;
    if meta.len() == 0 {
        let _ = std::fs::remove_file(&proxy_tmp_path);
        return Err("Proxy encode produced empty file".to_string());
    }

    // Atomically publish: rename tmp -> final. (On Windows, remove target first.)
    match std::fs::remove_file(&proxy_path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Failed to remove existing proxy: {e}")),
    }

    if let Err(e) = std::fs::rename(&proxy_tmp_path, &proxy_path) {
        // Fallback for any odd rename edge-case.
        std::fs::copy(&proxy_tmp_path, &proxy_path)
            .map_err(|copy_err| format!("Failed to publish proxy (rename={e}, copy={copy_err})"))?;
        let _ = std::fs::remove_file(&proxy_tmp_path);
    }

    Ok(proxy_path.to_string_lossy().to_string())
}

fn resolve_bundled_tool(app: &AppHandle, tool_name: &str) -> Result<PathBuf, String> {
    // Resolve a bundled tool (ffmpeg/ffprobe) across common resource paths.
    let exe_name = format!("{tool_name}.exe");

    // 1) Common bundled location: resources/bin/<tool>.exe
    if let Ok(p) = app
        .path()
        .resolve(format!("bin/{exe_name}"), tauri::path::BaseDirectory::Resource)
    {
        if p.exists() {
            return Ok(p);
        }
    }

    // 2) Alternative location if only backend internal <tool> is bundled
    if let Ok(p) = app.path().resolve(
        format!("bin/backend_script-x86_64-pc-windows-msvc/_internal/{exe_name}"),
        tauri::path::BaseDirectory::Resource,
    ) {
        if p.exists() {
            return Ok(p);
        }
    }

    // 3) Dev fallback: walk upward looking for ./bin/<tool>.exe
    // Prefer the backend_script _internal tools (they include more codecs, e.g. software HEVC)
    // over the plain ./bin/<tool>.exe.
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if let Some(mut dir) = exe.parent().map(|p| p.to_path_buf()) {
        for _ in 0..5 {
            let internal_candidate = dir
                .join("bin")
                .join("backend_script-x86_64-pc-windows-msvc")
                .join("_internal")
                .join(&exe_name);
            if internal_candidate.exists() {
                return Ok(internal_candidate);
            }

            let candidate = dir.join("bin").join(&exe_name);
            if candidate.exists() {
                return Ok(candidate);
            }
            if !dir.pop() {
                break;
            }
        }
    }

    Err(format!(
        "{exe_name} not found (looked in resources/bin, backend _internal, and dev src-tauri/bin)"
    ))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(PreviewProxyLocks::default())
        .invoke_handler(tauri::generate_handler![
            detect_scenes,
            export_clips,
            check_hevc,
            hover_preview_error,
            ensure_preview_proxy,
        ])
        .run(tauri::generate_context!())
        .expect("error running app");
}