import subprocess
import json
import os
import threading
import sys
import av
from pathlib import Path
from shutil import which

if getattr(sys, "frozen", False):
    ROOT = Path(sys.executable).resolve().parent
else:
    ROOT = Path(__file__).resolve().parent.parent

def get_binary(name):
    """Return path to ffmpeg/ffprobe.

    Supports:
    - dev layout: backend/ffmpeg.exe
    - PyInstaller onedir: <dist>/backend_script.exe + <dist>/_internal/ffmpeg.exe
    - PATH fallback
    """

    candidates: list[Path] = [
        ROOT / name,
        ROOT / "_internal" / name,
    ]

    for candidate in candidates:
        if candidate.exists():
            return str(candidate)

    found = which(name)
    if found:
        return found

    return str(candidates[0])

CREATE_NO_WINDOW = 0x08000000 if sys.platform == "win32" else 0

FFMPEG = get_binary("ffmpeg.exe")
FFPROBE = get_binary("ffprobe.exe")

def preprocess_video(input_path, output_path):
    cmd = [
        FFMPEG, "-y",
        "-i", input_path,
        "-vf", "scale=480:-1",
        output_path
    ]
    subprocess.run(
        cmd,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=True,
        creationflags=CREATE_NO_WINDOW
    )

def generate_keyframes_old_ffmpeg(video_path: str):
    """Generates keyframe for a given video"""
    cmd = [
        FFPROBE,                        # Launches the ffprobe executable
        "-skip_frame", "nokey",         # ONLY look at keyframes, skip others (speed)
        "-select_streams", "v:0",       # Only looks at the first video stream
        "-show_frames",                 # Shows metadata for EVERY frame
        "-show_entries", "frame=best_effort_timestamp_time",  # Filter, we only get the frame of the data
        "-of", "json",                  # Export as json format
        video_path
    ]
    # Executing system command
    result = subprocess.run(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,  # Return strings instead of bytes
        check=True,  # Crash if the command fails
        creationflags=CREATE_NO_WINDOW
    )

    data = json.loads(result.stdout)
    
    return [
        float(frame["best_effort_timestamp_time"])
        for frame in data.get("frames", [])
        if "best_effort_timestamp_time" in frame
    ]

def generate_keyframes(video_path: str):
    def _decode_keyframe_times(container, stream):
        # Still fast: decoder is instructed to skip non-key frames.
        times: list[float] = []
        try:
            stream.codec_context.skip_frame = "NONKEY"
        except Exception:
            pass

        for frame in container.decode(stream):
            if frame.pts is None:
                continue
            t = float(frame.pts * stream.time_base)  # type: ignore
            times.append(t)
        return times

    def _looks_pathological(times: list[float], duration_s: float | None) -> bool:
        # If cut points are extremely dense, segmenting becomes unusable.
        if len(times) < 2:
            return True

        # Basic duplicate/monotonic sanity.
        times_sorted = sorted(times)
        # Any non-increasing sequence suggests bad timestamps.
        for a, b in zip(times_sorted, times_sorted[1:]):
            if b <= a:
                return True

        if duration_s and duration_s > 0:
            # > 10 cuts/sec is almost certainly wrong for "keyframes".
            if (len(times_sorted) / duration_s) > 10.0:
                return True

        # If median spacing is tiny, it will produce near-1-frame segments.
        deltas = [b - a for a, b in zip(times_sorted, times_sorted[1:])]
        deltas.sort()
        median = deltas[len(deltas) // 2]
        return median < 0.05

    keyframes: list[float] = []
    with av.open(video_path) as container:
        stream = container.streams.video[0]

        duration_s: float | None = None
        try:
            if container.duration is not None:
                duration_s = float(container.duration) / 1_000_000.0
        except Exception:
            duration_s = None

        # Fast path: packet flags.
        try:
            for packet in container.demux(stream):
                if packet.pts is None:
                    continue
                if packet.is_keyframe:
                    keyframes.append(float(packet.pts * stream.time_base))  # type: ignore
        except Exception:
            keyframes = []

        # If packet flags are missing/unreliable, fall back to decode-based keyframes.
        if not keyframes or _looks_pathological(keyframes, duration_s):
            try:
                # Re-open to reset demux/decode state.
                with av.open(video_path) as container2:
                    stream2 = container2.streams.video[0]
                    keyframes = _decode_keyframe_times(container2, stream2)
            except Exception:
                return []

    # Normalize: sort + de-dupe small floating noise.
    keyframes = sorted(set(round(t, 6) for t in keyframes if t is not None and t >= 0.0))
    return keyframes

def keyframe_windows(keyframes, radius=1.0, fps=24.0):
    """Generates keyframe windows for """
    frame_duration = 1.0 / fps
    windows = [(max(0, k - radius), k + radius - frame_duration) for k in keyframes]
    windows.sort()
    merged = [windows[0]]
    for start, end in windows[1:]:
        last_start, last_end = merged[-1]
        if start <= last_end:
            merged[-1] = (last_start, max(last_end, end))
        else:
            merged.append((start, end))
    return merged

def trim_keyframes(video_path: str, output_dir="./keyframe_clips", radius=1.0):
    os.makedirs(output_dir, exist_ok=True)

    # generating keyframes
    keyframes = generate_keyframes(video_path)
    if not keyframes:
        return []

    # generating keyframe windows
    windows = keyframe_windows(keyframes, radius)
    clips = []
    
    # going through each keyframe window to trim the video
    for i, (start, end) in enumerate(windows):
        out_path = os.path.join(output_dir, f"kf_clip_{i:04d}.mp4")

        cmd = [
            FFMPEG,
            "-y",
            "-ss", str(start),  # trim from start of keyframe
            "-to", str(end),    # trim until end of keyframe 
            "-i", video_path,
            "-c", "copy",
            out_path
        ]

        subprocess.run(
            cmd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=True,
            creationflags=CREATE_NO_WINDOW
        )

        clips.append(
            {
                "clip_path": out_path,
                "window_start": start,
                "window_end": end
            }
        )
    return clips

def merge_short_scenes(boundaries, min_duration=0.5):
    """
    Merges scene boundaries if the resulting segment
    would be shorter than min_duration seconds.
    """

    if len(boundaries) <= 2:
        return boundaries

    merged = [boundaries[0]]

    for t in boundaries[1:]:
        if t - merged[-1] < min_duration:
            # Skip this boundary (merge small segment)
            continue
        merged.append(t)

    return merged

_progress_lock = threading.Lock()
def emit_progress(percent: int, message: str):
    import sys
    percent = max(0, min(100, int(percent)))
    with _progress_lock:
        print(f"PROGRESS|{percent}|{message}", file=sys.stderr, flush=True)