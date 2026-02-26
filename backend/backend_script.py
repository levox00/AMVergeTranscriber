import numpy as np
import subprocess
import math
import os
import cv2
import av
import sys
import json
from utils import generate_keyframes, emit_progress
from concurrent.futures import ThreadPoolExecutor, as_completed

def generate_thumbnails(output_dir: str, scene_count: int):
    def make_thumb(i):
        clip_path = os.path.join(output_dir, f"scene_{i:04d}.mp4")
        thumb_path = os.path.join(output_dir, f"scene_{i:04d}.jpg")
        if not os.path.exists(clip_path) or os.path.getsize(clip_path) == 0:
            return None
        cmd = [
            "ffmpeg", "-y",
            "-i", clip_path,
            "-vframes", "1",
            "-q:v", "5",        # jpeg quality 1-31, lower = better
            "-vf", "scale=320:-1",
            thumb_path
        ]
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        return thumb_path

    with ThreadPoolExecutor(max_workers=os.cpu_count()) as executor:
        futures = [executor.submit(make_thumb, i) for i in range(scene_count)]
        for _ in as_completed(futures):
            pass

def trim_scenes_at_keyframes(video_path: str, output_dir: str):
    os.makedirs(output_dir, exist_ok=True)
    
    emit_progress(10, "Extracting keyframes...")
    keyframes = generate_keyframes(video_path=video_path)
    print(f"Keyframes found: {len(keyframes)}", file=sys.stderr)
    print(f"First few: {keyframes[:5]}", file=sys.stderr)
    
    if not keyframes:
        print("No keyframes found, returning empty", file=sys.stderr)
        return []
    
    # Skip the first keyframe(0.0)
    cut_points = sorted(keyframes[1:])
    emit_progress(50, f"Cutting {len(cut_points)} scenes...")

    out_pattern = os.path.join(output_dir, "scene_%04d.mp4")

    cmd = [
        "ffmpeg", "-y",
        "-i", video_path,
        "-c", "copy",
        "-f", "segment",
        "-segment_times", ",".join(f"{t:.6f}" for t in cut_points),
        "-reset_timestamps", "1",
        out_pattern
    ]
    subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)

    print(f"Output dir: {output_dir}", file=sys.stderr)
    print(f"Files created: {os.listdir(output_dir)}", file=sys.stderr)

    emit_progress(75, "Generating thumbnails..")
    scene_count = len(cut_points) + 1
    generate_thumbnails(output_dir, scene_count)

    # Collect results
    emit_progress(95, "Assembling results...")
    final_scenes = []
    boundaries = [0.0] + cut_points
    for i, start in enumerate(boundaries):
        end = boundaries[i + 1] if i + 1 < len(boundaries) else None
        out_path = os.path.join(output_dir, f"scene_{i:04d}.mp4")
        thumb_path = os.path.join(output_dir, f"scene_{i:04d}.jpg")
        if os.path.exists(out_path) and os.path.getsize(out_path) > 0:
            final_scenes.append({
                "scene_index": i,
                "start": start,
                "end": end,
                "path": out_path,
                "thumbnail": thumb_path   # <-- new field
            })
    emit_progress(100, "Done")
    return final_scenes

if __name__ == "__main__":
    input_file = sys.argv[1]
    output_dir = sys.argv[2]
    # blocksize = int(sys.argv[3])
    # output_dir = sys.argv[4]
    # scenes = detect_and_trim_scenes(
    #     original_video_path=input_file,
    #     threshold=threshold,
    #     blocksize=blocksize,
    #     output_dir=output_dir
    # )

    scenes = trim_scenes_at_keyframes(input_file, output_dir)
    print("About to print JSON", file=sys.stderr)
    print(json.dumps(scenes)) # sends to stdout for rust to collect, react parses it
    sys.stdout.flush() 