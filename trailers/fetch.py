"""Melete's trailer downloader, which also fetches songs Soulseek doesn't have.

Downloads one title's trailer from YouTube and makes it playable everywhere: at most the height
asked for, converted once to HEVC on the GPU (NVENC; on the CPU when there's no GPU) unless it
already is H.264 or HEVC, HDR kept, audio as AAC, in an MP4 that starts playing at once.

Jobs are files. Melete writes `queue/<id>.json`; this takes one by renaming it into `running/`,
and answers with `done/<id>.json`, any file beside it. Three kinds:

- a trailer, {"id", "video", "max_height"} (`video` a YouTube video id): answered {"id", "ok":
  true, "file", "height", "duration", "converted"}, or {"id", "ok": true, "unavailable":
  "<why>"} when YouTube doesn't have it (removed, private, blocked here);
- a song search, {"id", "kind": "song_search", "query"}: YouTube's first results for the words,
  answered {"id", "ok": true, "results": [{"id", "title", "channel", "duration"}]}; Melete
  picks one, or none;
- a song, {"id", "kind": "song", "video"}: the video's best audio as it comes (Opus or AAC,
  never converted), answered {"id", "ok": true, "file", "duration", "codec", "bitrate"}, or
  "unavailable" as a trailer is.

Any job can be answered {"id", "ok": false, "error": "<why>"}.

It reaches nothing but YouTube, and only through the VPN container its network is attached to
(compose.trailers.yaml); it has no port and never sees the library. Only a trailer's video id,
a song's title and artist (its search) and the video id chosen leave.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import traceback

JOBS = os.environ.get("TRAILER_JOBS", "/jobs")
# Where a video id is fetched from; only tests point it elsewhere.
SOURCE = os.environ.get("TRAILER_SOURCE", "https://www.youtube.com/watch?v={id}")
IDLE_SECONDS = 5.0
# Pause between downloads, so the VPN's address isn't taken for a bot.
BETWEEN_SECONDS = float(os.environ.get("TRAILER_PAUSE", "20"))
VIDEO_ID = re.compile(r"^[A-Za-z0-9_-]{11}$")
HEIGHTS = (720, 1080, 2160)
# Results a song search reads, and the longest search words taken.
SEARCH_RESULTS = 10
MAX_QUERY = 200
# What YouTube says when the video can't be had at all, as opposed to a failure worth retrying.
GONE = ("Video unavailable", "Private video", "This video has been removed", "available in your country",
        "Sign in to confirm your age", "members-only", "Unavailable with a VPN or proxy",
        "regionally-restricted")


def failure(result):
    """Why yt-dlp failed: its last ERROR line, not a hint it printed after it (such as "You
    might want to use a VPN or a proxy server" after a video blocked in this country)."""
    said = (result.stderr or result.stdout).strip().splitlines()
    errors = [line for line in said if line.startswith("ERROR:")]
    if errors:
        return errors[-1][:300]
    return (said[-1] if said else f"yt-dlp ended with {result.returncode}")[:300]


def log(message):
    print(time.strftime("%Y-%m-%dT%H:%M:%S"), message, flush=True)


def probe(path):
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-print_format", "json", "-show_streams", "-show_format", path],
        check=True, capture_output=True, text=True).stdout
    data = json.loads(out)
    video = next(s for s in data["streams"] if s["codec_type"] == "video")
    audio = next((s for s in data["streams"] if s["codec_type"] == "audio"), None)
    return video, audio, float(data["format"].get("duration") or 0)


def nvenc_works():
    """Whether NVENC can encode here: a GPU given to the container, and a driver for it."""
    test = subprocess.run(
        ["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=s=256x256:d=0.1",
         "-c:v", "hevc_nvenc", "-f", "null", "-"],
        capture_output=True)
    return test.returncode == 0


def download(video_id, max_height, work):
    """The trailer's best video at most `max_height` high with its best audio, as one file."""
    url = SOURCE.format(id=video_id)
    template = os.path.join(work, "source.%(ext)s")
    result = subprocess.run(
        ["yt-dlp", "--no-playlist", "--no-progress", "--no-cache-dir", "--no-mtime",
         "--format", f"bv*[height<={max_height}]+ba/b[height<={max_height}]/bv*+ba/b",
         "--merge-output-format", "mkv", "--output", template, url],
        capture_output=True, text=True)
    if result.returncode != 0:
        return None, failure(result)
    found = [f for f in os.listdir(work) if f.startswith("source.")]
    if not found:
        return None, "yt-dlp left no file"
    return os.path.join(work, found[0]), None


def convert(source, out, nvenc):
    """`source` as an MP4 that plays everywhere. H.264 and HEVC are kept as they are; anything
    else (VP9, AV1) becomes HEVC, 10-bit with its HDR signalled when the source is HDR."""
    video, audio, _ = probe(source)
    codec = video.get("codec_name")
    hdr = video.get("color_transfer") in ("smpte2084", "arib-std-b67")
    args = ["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i", source,
            "-map", "0:v:0", "-map", "0:a:0?"]
    converted = codec not in ("h264", "hevc")
    if not converted:
        args += ["-c:v", "copy"]
    elif nvenc:
        args += ["-c:v", "hevc_nvenc", "-preset", "p6", "-tune", "hq", "-rc", "vbr", "-cq", "22", "-b:v", "0"]
    else:
        args += ["-c:v", "libx265", "-preset", "medium", "-crf", "20"]
    if converted:
        if hdr:
            args += ["-pix_fmt", "p010le" if nvenc else "yuv420p10le", "-profile:v", "main10",
                     "-color_primaries", video.get("color_primaries", "bt2020"),
                     "-color_trc", video.get("color_transfer"),
                     "-colorspace", video.get("color_space", "bt2020nc")]
        else:
            args += ["-pix_fmt", "yuv420p"]
    if codec == "hevc" or converted:
        args += ["-tag:v", "hvc1"]
    if audio is None:
        pass
    elif audio.get("codec_name") == "aac":
        args += ["-c:a", "copy"]
    else:
        args += ["-c:a", "aac", "-b:a", "256k"]
    args += ["-movflags", "+faststart", "-f", "mp4", out]
    subprocess.run(args, check=True, capture_output=True)
    return converted


def answer(job_id, body):
    path = os.path.join(JOBS, "done", f"{job_id}.json")
    with open(path + ".tmp", "w") as f:
        json.dump(body, f)
    os.rename(path + ".tmp", path)


def song_search(job, job_id):
    """YouTube's first results for the words, with each video's title, channel and length."""
    query = " ".join(str(job.get("query", "")).split())[:MAX_QUERY]
    if not query:
        raise ValueError("the job has no words to search for")
    result = subprocess.run(
        ["yt-dlp", "--flat-playlist", "--dump-single-json", "--no-cache-dir",
         f"ytsearch{SEARCH_RESULTS}:{query}"],
        capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(failure(result))
    found = json.loads(result.stdout or "{}")
    results = []
    for entry in found.get("entries") or []:
        video = str(entry.get("id") or "")
        if not VIDEO_ID.match(video):
            continue
        results.append({"id": video, "title": str(entry.get("title") or ""),
                        "channel": str(entry.get("channel") or entry.get("uploader") or ""),
                        "duration": entry.get("duration")})
    return {"id": job_id, "ok": True, "results": results}


def song(job, job_id):
    """The video's best audio, as it comes: Opus in an .opus file or AAC in an .m4a."""
    video_id = str(job.get("video", ""))
    if not VIDEO_ID.match(video_id):
        raise ValueError("the job names no YouTube video id")
    started = time.time()
    with tempfile.TemporaryDirectory(dir=os.path.join(JOBS, "running")) as scratch:
        result = subprocess.run(
            ["yt-dlp", "--no-playlist", "--no-progress", "--no-cache-dir", "--no-mtime",
             "--format", "bestaudio[acodec=opus]/bestaudio[ext=m4a]/bestaudio",
             "--extract-audio", "--audio-format", "best",
             "--output", os.path.join(scratch, "source.%(ext)s"), SOURCE.format(id=video_id)],
            capture_output=True, text=True)
        if result.returncode != 0:
            reason = failure(result)
            if any(g.lower() in reason.lower() for g in GONE):
                return {"id": job_id, "ok": True, "unavailable": reason}
            raise RuntimeError(reason)
        found = [f for f in os.listdir(scratch) if f.startswith("source.")]
        if not found:
            raise RuntimeError("yt-dlp left no file")
        source = os.path.join(scratch, found[0])
        out = subprocess.run(
            ["ffprobe", "-v", "error", "-print_format", "json", "-show_streams", "-show_format",
             source], check=True, capture_output=True, text=True).stdout
        data = json.loads(out)
        audio = next((s for s in data["streams"] if s["codec_type"] == "audio"), None)
        if audio is None:
            raise RuntimeError("what YouTube gave has no audio")
        codec = audio.get("codec_name")
        extension = {"opus": "opus", "aac": "m4a"}.get(codec)
        if extension is None:
            raise RuntimeError(f"YouTube gave {codec} audio, which isn't kept")
        file = f"{job_id}.{extension}"
        partial = os.path.join(JOBS, "done", f".{file}.partial")
        try:
            # Into its own kind of file, the audio as it is.
            subprocess.run(
                ["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i", source,
                 "-map", "0:a:0", "-c:a", "copy", "-f", "ogg" if extension == "opus" else "ipod",
                 partial], check=True, capture_output=True)
            os.rename(partial, os.path.join(JOBS, "done", file))
        finally:
            if os.path.exists(partial):
                os.remove(partial)
    bitrate = int(data["format"].get("bit_rate") or audio.get("bit_rate") or 0)
    return {"id": job_id, "ok": True, "file": file,
            "duration": round(float(data["format"].get("duration") or 0), 2),
            "codec": codec, "bitrate": bitrate, "seconds": round(time.time() - started, 1)}


def work(job, job_id, nvenc):
    kind = job.get("kind", "trailer")
    if kind == "song_search":
        return song_search(job, job_id)
    if kind == "song":
        return song(job, job_id)
    if kind != "trailer":
        raise ValueError(f"{kind} isn't a kind of job")
    video_id = str(job.get("video", ""))
    max_height = int(job.get("max_height", 0))
    if not VIDEO_ID.match(video_id):
        raise ValueError("the job names no YouTube video id")
    if max_height not in HEIGHTS:
        raise ValueError(f"{max_height} isn't a height to download at")
    started = time.time()
    with tempfile.TemporaryDirectory(dir=os.path.join(JOBS, "running")) as scratch:
        source, gone = download(video_id, max_height, scratch)
        if source is None:
            if any(g.lower() in gone.lower() for g in GONE):
                return {"id": job_id, "ok": True, "unavailable": gone}
            raise RuntimeError(gone)
        file = f"{job_id}.mp4"
        partial = os.path.join(JOBS, "done", f".{file}.partial")
        try:
            converted = convert(source, partial, nvenc)
            video, _, duration = probe(partial)
            os.rename(partial, os.path.join(JOBS, "done", file))
        finally:
            if os.path.exists(partial):
                os.remove(partial)
    return {"id": job_id, "ok": True, "file": file, "height": int(video.get("height", 0)),
            "duration": round(duration, 2), "converted": converted,
            "seconds": round(time.time() - started, 1)}


def main():
    for sub in ("queue", "running", "done"):
        os.makedirs(os.path.join(JOBS, sub), exist_ok=True)
    # A job a restart cut off is taken up again; its scratch folder goes.
    for name in os.listdir(os.path.join(JOBS, "running")):
        path = os.path.join(JOBS, "running", name)
        if name.endswith(".json"):
            os.rename(path, os.path.join(JOBS, "queue", name))
        elif os.path.isdir(path):
            shutil.rmtree(path, ignore_errors=True)
    nvenc = nvenc_works()
    log("ready; converting on the " + ("GPU (NVENC)" if nvenc else "CPU: no GPU here"))
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
            if not re.match(r"^[A-Za-z0-9_-]{1,64}$", job_id):
                raise ValueError("the job's id isn't one to name files by")
            body = work(job, job_id, nvenc)
            answer(job_id, body)
            done = "searched" if "results" in body else "downloaded"
            log(f"job {job_id}: {body.get('unavailable') or done}")
        except Exception as err:  # Any failure is the job's answer, never the worker's end.
            if re.match(r"^[A-Za-z0-9_-]{1,64}$", job_id):
                answer(job_id, {"id": job_id, "ok": False, "error": f"{type(err).__name__}: {err}"[:500]})
            log(f"job {job_id} failed:\n{traceback.format_exc()}")
        finally:
            os.remove(running)
        time.sleep(BETWEEN_SECONDS)


if __name__ == "__main__":
    sys.exit(main())
