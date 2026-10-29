"""Melete's stem separator (decision 84).

Splits a music track into its parts for the Auto DJ and sing-along: the voice for the whole
track, and drums, bass, guitar and other for its first and last minute and a half, where a DJ mixes.
The voice comes from Mel-Band RoFormer (Kim's checkpoint), the rest from Demucs' six-part model
run on the track minus the voice; piano is folded into other, and other is whatever is left, so
the parts always add back to the track exactly.

Jobs are files. Melete writes `queue/<id>.json` ({"id", "source", "folder", "edge_seconds"});
this takes one by renaming it into `running/`, writes the parts into `<stems>/<folder>/` (a new
folder swapped in whole), and answers with `done/<id>.json`. It has no network service: the
only traffic is the models' download from their publishers on first use. Music is read-only.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import traceback

import numpy as np
import soundfile as sf
from audio_separator.separator import Separator

JOBS = os.environ.get("STEMS_JOBS", "/jobs")
OUT = os.environ.get("STEMS_DIR", "/stems")
MODELS = os.environ.get("STEMS_MODELS", "/models")
VOICE_MODEL = "vocals_mel_band_roformer.ckpt"
REST_MODEL = "htdemucs_6s.yaml"
# The models work at this rate; a track at another is brought back to its own afterwards.
MODEL_RATE = 44100
# Separated with this much of the track either side, then trimmed, so a region's edges are
# separated with what surrounds them.
CONTEXT_SECONDS = 5.0
IDLE_SECONDS = 5.0


def log(message):
    print(time.strftime("%Y-%m-%dT%H:%M:%S"), message, flush=True)


def ffmpeg(*args):
    subprocess.run(["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y", *args], check=True)


def separator(model, out):
    # No amplifying, and scaling down only a part that would clip (the most it allows). The
    # parts still add back exactly: the rest is worked out from the voice as kept, and other
    # from what's left.
    sep = Separator(
        output_dir=out,
        model_file_dir=MODELS,
        output_format="WAV",
        normalization_threshold=1.0,
        amplification_threshold=0.0,
        sample_rate=MODEL_RATE,
        log_level=30,
    )
    sep.load_model(model_filename=model)
    return sep


def parts_of(sep, path, out):
    """Separates one file; returns each part, by its lowercase name, as float32 at the model rate."""
    for old in os.listdir(out):
        os.remove(os.path.join(out, old))
    for name in sep.separate(path):
        full = name if os.path.isabs(name) else os.path.join(out, name)
        part = os.path.basename(full).split("_(", 1)[1].split(")", 1)[0].lower()
        audio, _ = sf.read(full, dtype="float32", always_2d=True)
        yield part, audio


def at_rate(audio, channels, frames, rate, tmp, name):
    """`audio` at the model rate brought to the track's rate, channels and exact length."""
    if rate != MODEL_RATE:
        src, dst = os.path.join(tmp, f"{name}-in.wav"), os.path.join(tmp, f"{name}-out.wav")
        sf.write(src, audio, MODEL_RATE, subtype="FLOAT")
        ffmpeg("-i", src, "-af", "aresample=resampler=soxr:precision=28", "-ar", str(rate), "-c:a", "pcm_f32le", dst)
        audio, _ = sf.read(dst, dtype="float32", always_2d=True)
    if audio.shape[1] != channels:
        audio = np.repeat(audio.mean(axis=1, keepdims=True), channels, axis=1)
    if len(audio) < frames:
        audio = np.concatenate([audio, np.zeros((frames - len(audio), channels), np.float32)])
    return audio[:frames]


def depth_of(source):
    """The FLAC depth the parts are kept at: the track's own when it's 16-bit or lossy (16 bits
    hold all a lossy file has), else 24."""
    try:
        subtype = sf.info(source).subtype
    except Exception:  # A format soundfile can't read (AAC, Opus): lossy, so 16 will do.
        return "PCM_16"
    return "PCM_16" if subtype in ("PCM_16", "PCM_S8", "PCM_U8", "MPEG_LAYER_III", "VORBIS", "OPUS") else "PCM_24"


def write(path, audio, rate, depth):
    sf.write(path, np.clip(audio, -1.0, 1.0), rate, format="FLAC", subtype=depth)


def work(job, voice, rest, tmp):
    source, folder = job["source"], job["folder"]
    if not os.path.isabs(source) or ".." in source.split("/"):
        raise ValueError("source must be an absolute path")
    if os.path.isabs(folder) or ".." in folder.split("/") or not folder:
        raise ValueError("folder must be relative, within the stems folder")
    edge_seconds = float(job.get("edge_seconds", 90))

    # The track as the library has it: decoded at its own rate and channels.
    decoded = os.path.join(tmp, "track.wav")
    ffmpeg("-i", source, "-map", "0:a:0", "-c:a", "pcm_f32le", decoded)
    track, rate = sf.read(decoded, dtype="float32", always_2d=True)
    depth = depth_of(source)
    frames, channels = track.shape
    if frames == 0:
        raise ValueError("the track has no audio")
    for_model = os.path.join(tmp, "model.wav")
    ffmpeg("-i", decoded, "-ar", str(MODEL_RATE), "-ac", "2", "-c:a", "pcm_f32le", for_model)

    started = time.time()
    voiced = dict(parts_of(voice, for_model, os.path.join(tmp, "voice")))
    vocals = at_rate(voiced["vocals"], channels, frames, rate, tmp, "voice")
    remainder = track - vocals

    edge = int(edge_seconds * rate)
    context = int(CONTEXT_SECONDS * rate)
    regions = [("all", 0, frames)] if frames <= 2 * edge else [("start", 0, edge), ("end", frames - edge, frames)]

    staging = os.path.join(OUT, f".{folder.replace('/', '_')}.new")
    shutil.rmtree(staging, ignore_errors=True)
    os.makedirs(staging)
    write(os.path.join(staging, "voice.flac"), vocals, rate, depth)
    manifest_regions = []
    for name, a, b in regions:
        lo, hi = max(0, a - context), min(frames, b + context)
        segment = os.path.join(tmp, f"{name}.wav")
        clip = remainder[lo:hi]
        sf.write(segment, clip, rate, subtype="FLOAT")
        seg_model = os.path.join(tmp, f"{name}-model.wav")
        ffmpeg("-i", segment, "-ar", str(MODEL_RATE), "-ac", "2", "-c:a", "pcm_f32le", seg_model)
        found = dict(parts_of(rest, seg_model, os.path.join(tmp, "rest")))
        kept = {}
        for part in ("drums", "bass", "guitar"):
            audio = at_rate(found[part], channels, hi - lo, rate, tmp, part)
            kept[part] = audio[a - lo : b - lo]
        # Other is what's left (piano included), so the region adds back to the track exactly.
        kept["other"] = remainder[a:b] - kept["drums"] - kept["bass"] - kept["guitar"]
        files = {}
        for part, audio in kept.items():
            file = f"{name}_{part}.flac"
            write(os.path.join(staging, file), audio, rate, depth)
            files[part] = file
        manifest_regions.append({"name": name, "start": a, "end": b, "parts": files})

    manifest = {
        "version": 1,
        "rate": rate,
        "channels": channels,
        "frames": frames,
        "bits": 16 if depth == "PCM_16" else 24,
        "voice": "voice.flac",
        "regions": manifest_regions,
        "models": [VOICE_MODEL, REST_MODEL],
        "seconds": round(time.time() - started, 1),
    }
    with open(os.path.join(staging, "stems.json"), "w") as f:
        json.dump(manifest, f)
    # Swapped in whole, so a reader never sees half a set of parts.
    target = os.path.join(OUT, folder)
    os.makedirs(os.path.dirname(target), exist_ok=True)
    old = target + ".old"
    shutil.rmtree(old, ignore_errors=True)
    if os.path.exists(target):
        os.rename(target, old)
    os.rename(staging, target)
    shutil.rmtree(old, ignore_errors=True)
    return manifest


def answer(job_id, body):
    path = os.path.join(JOBS, "done", f"{job_id}.json")
    with open(path + ".tmp", "w") as f:
        json.dump(body, f)
    os.rename(path + ".tmp", path)


def main():
    for sub in ("queue", "running", "done"):
        os.makedirs(os.path.join(JOBS, sub), exist_ok=True)
    # A job a restart cut off is taken up again.
    for name in os.listdir(os.path.join(JOBS, "running")):
        os.rename(os.path.join(JOBS, "running", name), os.path.join(JOBS, "queue", name))
    tmp_root = tempfile.mkdtemp(prefix="stems-")
    for sub in ("voice", "rest"):
        os.makedirs(os.path.join(tmp_root, sub))
    log("loading models (downloaded on first use)")
    voice = separator(VOICE_MODEL, os.path.join(tmp_root, "voice"))
    rest = separator(REST_MODEL, os.path.join(tmp_root, "rest"))
    log("ready")
    while True:
        waiting = sorted(n for n in os.listdir(os.path.join(JOBS, "queue")) if n.endswith(".json"))
        if not waiting:
            time.sleep(IDLE_SECONDS)
            continue
        name = waiting[0]
        running = os.path.join(JOBS, "running", name)
        try:
            os.rename(os.path.join(JOBS, "queue", name), running)
        except FileNotFoundError:
            continue
        job_id = name[: -len(".json")]
        try:
            with open(running) as f:
                job = json.load(f)
            job_id = str(job.get("id", job_id))
            manifest = work(job, voice, rest, tmp_root)
            answer(job_id, {"id": job_id, "ok": True, "manifest": manifest})
            log(f"job {job_id}: separated in {manifest['seconds']} s")
        except Exception as err:  # Any failure is the job's answer, never the worker's end.
            answer(job_id, {"id": job_id, "ok": False, "error": f"{type(err).__name__}: {err}"[:500]})
            log(f"job {job_id} failed:\n{traceback.format_exc()}")
        finally:
            if os.path.exists(running):
                os.remove(running)


if __name__ == "__main__":
    sys.exit(main())
