"""Melete's motion cover renderer (decision 124).

Makes short loops from an album's still cover. The square one: Depth Anything V2 Small
(Apache-2.0) estimates the cover's depth, lettering (PP-OCRv3's text detector, Apache-2.0) and
faces (YuNet, MIT) are found and each held flat so it moves as one rigid piece, and the cover is
displaced by its depth as a camera drifts across it and slowly in and out again (OpenCV), then
encoded as H.264 by ffmpeg. Covers that are mostly text, flat, or have no depth to move get no
square loop, with the reason. The tall one (9:16, for full-screen players) is made for every
cover: the cover large and sharp (moving as the square loop does, when there is one) over a
blurred, darkened copy of itself that drifts slowly.

Jobs are files. Melete writes `queue/<id>.cover` (the cover's bytes) and `queue/<id>.json`
({"id", "cover", "version", "loop": {"seconds", "fps", "max_side", "path", "strength"},
"tall": {"width", "height"}}); this takes a job by renaming it into `running/`, and answers
with `done/<id>.json` and the videos beside it (`video`, unless skipped, and `tall`). It has no network service and never sees the library: the only traffic is the models'
download from their publishers on first use, each checked against its SHA-256.

`python render.py --try <image> <square.mp4> <tall.mp4>` renders one image outside the queue (for
checking).
"""

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
import traceback
import urllib.request

import cv2
import numpy as np
import onnxruntime as ort

JOBS = os.environ.get("MOTION_JOBS", "/jobs")
MODELS = os.environ.get("MOTION_MODELS", "/models")
# Depth on the CPU by default: about two seconds a cover, no warm-up, and the GPU stays free for
# transcoding. "gpu" runs it through CUDA when the container has a GPU.
DEVICE = os.environ.get("MOTION_DEVICE", "cpu")
IDLE_SECONDS = 5.0
RENDERER = "melete-motion 2"

# Pinned to a revision and checked by hash: a file that doesn't match is never used.
MODEL_FILES = {
    "depth": (
        "depth_anything_v2_small.onnx",
        "https://huggingface.co/onnx-community/depth-anything-v2-small/resolve/"
        "4472b7362082ad9968fee890ca0f1e5aca36b93d/onnx/model.onnx",
        "afb6a5c28f3b6bf1618c6e43f02073ef9dfdc70e937502d51603e57b0a1df10c",
    ),
    "text": (
        "text_detection_en_ppocrv3_2023may.onnx",
        "https://github.com/opencv/opencv_zoo/raw/48b10fa505aeb51fbd127fc60a5c660040e823b7/"
        "models/text_detection_ppocr/text_detection_en_ppocrv3_2023may.onnx",
        "03f550c6b406fda8bf54bd8327815f6c7e2edd98cea02348c93d879254366587",
    ),
    "face": (
        "face_detection_yunet_2023mar.onnx",
        "https://github.com/opencv/opencv_zoo/raw/48b10fa505aeb51fbd127fc60a5c660040e823b7/"
        "models/face_detection_yunet/face_detection_yunet_2023mar.onnx",
        "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
    ),
}

# When a cover is skipped, checked against about 30 real album covers of many kinds and 20 made
# ones. Text: a headline over a photo stays under it, a typographic cover goes over.
MOSTLY_TEXT = 0.35  # share of the cover inside found lettering
# Flat: people on a plain backdrop stay under it (0.82), a flat graphic goes over (0.91 and up).
FLAT = 0.85  # share of the cover in its three most common colours
# Depth Anything gives relative disparity: on a photo the far end falls towards zero, on a
# poster-like cover it stays close to the near end, a shallow bowl that would move as a bulge.
# Photos and paintings measured 0.67 and up; gradients, patterns and framed graphics 0.64 and below.
MIN_DEPTH_RANGE = 0.65


def log(message):
    print(time.strftime("%Y-%m-%dT%H:%M:%S"), message, flush=True)


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def model(key):
    """The model's file, downloaded on first use and checked against its hash either way."""
    name, url, digest = MODEL_FILES[key]
    path = os.path.join(MODELS, name)
    if not os.path.exists(path):
        log(f"downloading {name} (once)")
        os.makedirs(MODELS, exist_ok=True)
        partial = path + ".part"
        req = urllib.request.Request(url, headers={"User-Agent": "Python-urllib"})
        with urllib.request.urlopen(req, timeout=300) as r, open(partial, "wb") as f:
            while chunk := r.read(1 << 20):
                f.write(chunk)
        if sha256(partial) != digest:
            os.remove(partial)
            raise RuntimeError(f"{name} doesn't match its SHA-256; not used")
        os.rename(partial, path)
    elif sha256(path) != digest:
        raise RuntimeError(f"{name} in the models folder doesn't match its SHA-256; delete it to fetch it again")
    return path


class Models:
    def __init__(self, device):
        providers = ["CPUExecutionProvider"]
        if device == "gpu":
            # CUDA and cuDNN come as Python packages with the base image's PyTorch; ONNX Runtime
            # finds them only when told to load them first.
            ort.preload_dlls()
            providers = ["CUDAExecutionProvider", "CPUExecutionProvider"]
        self.depth = ort.InferenceSession(model("depth"), providers=providers)
        self.providers = self.depth.get_providers()
        if device == "gpu" and self.providers[0] != "CUDAExecutionProvider":
            log("MOTION_DEVICE=gpu, but CUDA isn't available here; depth runs on the CPU")
        self.input = self.depth.get_inputs()[0].name
        self.text = cv2.dnn.TextDetectionModel_DB(model("text"))
        self.text.setBinaryThreshold(0.3).setPolygonThreshold(0.5).setMaxCandidates(200).setUnclipRatio(2.0)
        self.text.setInputParams(1.0 / 255.0, (736, 736), (122.67891434, 116.66876762, 104.00698793))
        self.face_path = model("face")

    def depth_of(self, rgb):
        h, w = rgb.shape[:2]
        x = cv2.resize(rgb, (518, 518), interpolation=cv2.INTER_CUBIC).astype(np.float32) / 255.0
        x = (x - np.array([0.485, 0.456, 0.406], np.float32)) / np.array([0.229, 0.224, 0.225], np.float32)
        out = self.depth.run(None, {self.input: x.transpose(2, 0, 1)[None]})[0]
        out = np.asarray(out, np.float32).reshape(out.shape[-2:])
        return cv2.resize(out, (w, h), interpolation=cv2.INTER_CUBIC)

    def lettering(self, bgr):
        h, w = bgr.shape[:2]
        polys, _ = self.text.detect(cv2.resize(bgr, (736, 736)))
        out = []
        for p in polys:
            p = np.array(p, np.float32)
            p[:, 0] *= w / 736
            p[:, 1] *= h / 736
            out.append(p.astype(np.int32))
        return out

    def faces(self, bgr):
        h, w = bgr.shape[:2]
        found = cv2.FaceDetectorYN.create(self.face_path, "", (w, h), 0.75, 0.3, 50).detect(bgr)[1]
        return [] if found is None else [tuple(int(v) for v in f[:4]) for f in found]


def flatness(bgr):
    small = cv2.resize(bgr, (128, 128), interpolation=cv2.INTER_AREA)
    q = (small // 32).reshape(-1, 3).astype(np.int32)
    counts = np.bincount(q[:, 0] * 64 + q[:, 1] * 8 + q[:, 2], minlength=512)
    return float(np.sort(counts)[-3:].sum() / len(q))


def prepare(raw, polys, faces):
    """The depth to move by (0 far to 1 near): smoothed, the near side grown a little so edges
    stretch the background rather than the subject, and each piece of lettering and each face
    flattened to its own depth so it moves whole. Also the focal depth (the lettering's, else
    the faces', else the middle) and how much of the cover is lettering."""
    h, w = raw.shape
    side = min(h, w)
    lo, hi = np.percentile(raw, 1), np.percentile(raw, 99)
    d = np.clip((raw - lo) / max(hi - lo, 1e-6), 0, 1).astype(np.float32)
    d = cv2.GaussianBlur(d, (0, 0), max(1.0, side * 0.004))
    k = max(3, int(side * 0.012) | 1)
    d = cv2.dilate(d, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (k, k)))
    d = cv2.GaussianBlur(d, (0, 0), max(1.0, side * 0.006))
    regions = []
    for p in polys:
        _, _, bw, bh = cv2.boundingRect(p)
        regions.append(("text", p, max(4, int(0.012 * side + 0.25 * min(bw, bh)))))
    for (x, y, fw, fh) in faces:
        g = int(0.3 * max(fw, fh))
        box = np.array([[x - g, y - g], [x + fw + g, y - g], [x + fw + g, y + fh + g], [x - g, y + fh + g]], np.int32)
        regions.append(("face", box, max(4, int(0.25 * max(fw, fh)))))
    text_mask = np.zeros(d.shape, np.uint8)
    depths = {"text": [], "face": []}
    for kind, poly, margin in regions:
        mask = np.zeros(d.shape, np.uint8)
        cv2.fillPoly(mask, [poly], 1)
        if kind == "text":
            cv2.fillPoly(text_mask, [poly], 1)
        n = int(mask.sum())
        if n == 0:
            continue
        value = float(np.median(d[mask > 0]))
        depths[kind].append((value, n))
        grown = cv2.dilate(mask, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2 * margin + 1, 2 * margin + 1)))
        weight = np.maximum(cv2.GaussianBlur(grown.astype(np.float32), (0, 0), margin / 2.0), mask.astype(np.float32))
        d = d * (1 - weight) + value * weight
    focus = depths["text"] or depths["face"]
    d0 = sum(v * n for v, n in focus) / sum(n for _, n in focus) if focus else float(np.median(d))
    text_share = float(cv2.dilate(text_mask, np.ones((9, 9), np.uint8)).mean())
    return d.astype(np.float32), float(d0), text_share


def encoder(width, height, fps, out_path, crf):
    """ffmpeg, taking raw BGR frames on its input and writing an H.264 MP4."""
    return subprocess.Popen(
        ["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "bgr24",
         "-s", f"{width}x{height}", "-r", str(fps), "-i", "-", "-c:v", "libx264", "-preset", "slow", "-crf", str(crf),
         "-pix_fmt", "yuv420p", "-movflags", "+faststart", "-an", "-f", "mp4", out_path],
        stdin=subprocess.PIPE)


def finish(encoders):
    codes = []
    for enc in encoders:
        enc.stdin.close()
        codes.append(enc.wait())
    for code in codes:
        if code != 0:
            raise RuntimeError(f"ffmpeg ended with {code}")


class Square:
    """The square loop's frames: each the cover displaced by (depth - focal depth) as the camera
    sways across it (side to side, and a little up and down, twice as often) while it moves slowly
    in and out. Every motion is a whole number of cycles, so the last frame leads straight back to
    the first."""

    # How far the camera moves in at the deepest, as a share of the cover's half side.
    PUSH = 0.6

    def __init__(self, bgr, d, d0, loop):
        h, w = d.shape
        side = min(h, w)
        gx = cv2.Sobel(d, cv2.CV_32F, 1, 0, ksize=3) / 8
        gy = cv2.Sobel(d, cv2.CV_32F, 0, 1, ksize=3) / 8
        steepest = float(np.percentile(np.hypot(gx, gy), 99.9))
        # The sway and the push together never so far that the warp folds over itself.
        self.amp = min(float(loop["strength"]) * side, 0.5 / max(steepest, 1e-6) / (1 + self.PUSH))
        self.push = self.PUSH * self.amp / (side / 2)
        # Zoomed in just enough that the edges never show what's beyond them.
        edge = max(2, int(side * 0.02))
        border = np.concatenate([d[:edge].ravel(), d[-edge:].ravel(), d[:, :edge].ravel(), d[:, -edge:].ravel()])
        reach = self.amp * (1 + self.PUSH) * float(np.max(np.abs(border - d0)))
        self.zoom = 1 + 2 * (reach + 1) / side
        self.cy, self.cx = (h - 1) / 2, (w - 1) / 2
        yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
        self.bx = (self.cx + (xx - self.cx) / self.zoom).astype(np.float32)
        self.by = (self.cy + (yy - self.cy) / self.zoom).astype(np.float32)
        self.rel = (d - d0).astype(np.float32)
        self.bgr = bgr

    def frame(self, phase):
        ox = np.float32(self.amp * np.sin(phase))
        oy = np.float32(0.35 * self.amp * np.sin(2 * phase))
        # In and out once, easing at both ends: nearer things grow more than farther ones.
        push = np.float32(self.push * (1 - np.cos(phase)) / 2)
        mx, my = self.bx, self.by
        # Where each pixel comes from depends on the depth there; two steps find it.
        for _ in range(2):
            disp = cv2.remap(self.rel, mx, my, cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE)
            mx = (self.bx - (ox + push * (self.bx - self.cx)) * disp).astype(np.float32)
            my = (self.by - (oy + push * (self.by - self.cy)) * disp).astype(np.float32)
        return cv2.remap(self.bgr, mx, my, cv2.INTER_CUBIC, borderMode=cv2.BORDER_REFLECT)


class Tall:
    """The tall loop's frames: the cover large and sharp, a little above the middle where a
    full-screen player shows it, with a soft shadow, over a blurred and darkened copy of itself
    filling the frame and drifting slowly round."""

    COVER = 0.84  # the cover's width, as a share of the frame's
    CENTRE = 0.42  # its middle, as a share of the frame's height from the top
    DIM = 0.62  # how bright the background stays
    DRIFT = 0.04  # how far the background moves, as a share of the frame's width

    def __init__(self, bgr, width, height):
        self.w, self.h = width, height
        ch, cw = bgr.shape[:2]
        # The cover as large as fits the cover's place, keeping its shape.
        fit = self.COVER * width / max(cw, ch)
        self.fw, self.fh = max(2, round(cw * fit)), max(2, round(ch * fit))
        self.x = (width - self.fw) // 2
        self.y = max(0, min(height - self.fh, round(self.CENTRE * height - self.fh / 2)))
        # The background, made small: blurred so much that its size doesn't show once enlarged.
        self.small = 4
        sw, sh = width // self.small, height // self.small
        over = 1 + 2 * self.DRIFT + 0.08
        scale = over * max(sw / cw, sh / ch)
        bg = cv2.resize(bgr, (max(sw, round(cw * scale)), max(sh, round(ch * scale))), interpolation=cv2.INTER_AREA)
        bg = cv2.GaussianBlur(bg, (0, 0), sw * 0.05)
        bg = cv2.GaussianBlur(bg, (0, 0), sw * 0.05)
        self.bg = (bg.astype(np.float32) * self.DIM).astype(np.float32)
        self.sw, self.sh = sw, sh
        # The shadow under the cover: how much of the background shows through, everywhere.
        shadow = np.zeros((height, width), np.float32)
        drop = round(0.015 * height)
        cv2.rectangle(shadow, (self.x, self.y + drop), (self.x + self.fw, self.y + self.fh + drop), 0.55, -1)
        self.keep = (1 - cv2.GaussianBlur(shadow, (0, 0), 0.03 * width))[..., None]
        self.still = cv2.resize(bgr, (self.fw, self.fh), interpolation=cv2.INTER_AREA if fit < 1 else cv2.INTER_CUBIC)

    def frame(self, phase, cover=None):
        bh, bw = self.bg.shape[:2]
        reach = self.DRIFT * self.sw
        # Round an ellipse, the background breathing a little larger and back as it goes.
        tx = (bw - self.sw) / 2 + reach * np.cos(phase)
        ty = (bh - self.sh) / 2 + reach * 1.5 * np.sin(phase)
        grow = 1 + 0.04 * (1 - np.cos(phase)) / 2
        m = np.float32([[1 / grow, 0, tx + (1 - 1 / grow) * self.sw / 2],
                        [0, 1 / grow, ty + (1 - 1 / grow) * self.sh / 2]])
        small = cv2.warpAffine(self.bg, m, (self.sw, self.sh), flags=cv2.INTER_LINEAR | cv2.WARP_INVERSE_MAP,
                               borderMode=cv2.BORDER_REFLECT)
        out = cv2.resize(small, (self.w, self.h), interpolation=cv2.INTER_CUBIC) * self.keep
        out = np.clip(out, 0, 255).astype(np.uint8)
        front = self.still if cover is None else cv2.resize(cover, (self.fw, self.fh), interpolation=cv2.INTER_AREA)
        out[self.y:self.y + self.fh, self.x:self.x + self.fw] = front
        return out


def render(bgr, square, loop, tall, square_path, tall_path):
    """Writes the tall loop, and the square one when `square` is given (a Square), in one pass:
    the tall one shows the square one's frames as they're made."""
    fps, frames = int(loop["fps"]), int(loop["seconds"]) * int(loop["fps"])
    h, w = bgr.shape[:2]
    tall = Tall(bgr, int(tall["width"]), int(tall["height"]))
    encoders = [encoder(tall.w, tall.h, fps, tall_path, 22)]
    if square is not None:
        encoders.append(encoder(w, h, fps, square_path, 20))
    try:
        for i in range(frames):
            phase = 2 * np.pi * i / frames
            moving = square.frame(phase) if square is not None else None
            if moving is not None:
                encoders[1].stdin.write(moving.tobytes())
            encoders[0].stdin.write(tall.frame(phase, moving).tobytes())
    finally:
        finish(encoders)
    out = {"tall_width": tall.w, "tall_height": tall.h}
    if square is not None:
        out.update({"amplitude_px": round(square.amp, 2), "zoom": round(square.zoom, 4)})
    return out


def make(models, data, loop, tall, out_path, tall_path, force=False):
    """One cover: returns (skip reason or None, details). Renders the tall loop, and the square
    one unless skipped."""
    started = time.time()
    if loop.get("path") != "drift":
        raise ValueError(f"unknown camera path {loop.get('path')!r}")
    tall_w, tall_h = int(tall["width"]), int(tall["height"])
    if not (2 <= tall_w <= 2160 and 2 <= tall_h <= 3840 and tall_w % 2 == 0 and tall_h % 2 == 0):
        raise ValueError(f"a tall loop {tall_w}x{tall_h} isn't one to make")
    bgr = cv2.imdecode(np.frombuffer(data, np.uint8), cv2.IMREAD_COLOR)
    if bgr is None:
        raise ValueError("the cover couldn't be decoded")
    h, w = bgr.shape[:2]
    # At its own size, never larger; at most max_side; even sides for H.264's 4:2:0.
    scale = min(1.0, int(loop["max_side"]) / max(h, w))
    nw, nh = max(2, int(w * scale) // 2 * 2), max(2, int(h * scale) // 2 * 2)
    if (nw, nh) != (w, h):
        bgr = cv2.resize(bgr, (nw, nh), interpolation=cv2.INTER_AREA)
    t = time.time()
    raw = models.depth_of(cv2.cvtColor(bgr, cv2.COLOR_BGR2RGB))
    depth_s = time.time() - t
    polys, faces = models.lettering(bgr), models.faces(bgr)
    d, d0, text_share = prepare(raw, polys, faces)
    flat = flatness(bgr)
    p1, p99 = np.percentile(raw, [1, 99])
    depth_range = float((p99 - p1) / max(abs(p99), 1e-6))
    details = {
        "renderer": RENDERER, "width": nw, "height": nh, "lettering": len(polys), "faces": len(faces),
        "text_share": round(text_share, 3), "flat": round(flat, 3), "depth_range": round(depth_range, 3),
        "depth_device": "gpu" if models.providers[0] == "CUDAExecutionProvider" else "cpu",
        "depth_seconds": round(depth_s, 3),
    }
    reason = None
    if text_share >= MOSTLY_TEXT:
        reason = "mostly text"
    elif flat >= FLAT:
        reason = "flat artwork"
    elif depth_range < MIN_DEPTH_RANGE:
        reason = "no depth to move"
    square = Square(bgr, d, d0, loop) if reason is None or force else None
    details.update(render(bgr, square, loop, tall, out_path, tall_path))
    details["seconds"] = round(time.time() - started, 2)
    return reason, details


def answer(job_id, body):
    path = os.path.join(JOBS, "done", f"{job_id}.json")
    with open(path + ".tmp", "w") as f:
        json.dump(body, f)
    os.rename(path + ".tmp", path)


def work(models, job, job_id):
    cover = job.get("cover", "")
    if not cover or "/" in cover or cover.startswith("."):
        raise ValueError("the job names no usable cover")
    with open(os.path.join(JOBS, "queue", cover), "rb") as f:
        data = f.read()
    video, tall = f"{job_id}.mp4", f"{job_id}.tall.mp4"
    partial = os.path.join(JOBS, "done", f".{video}.partial")
    tall_partial = os.path.join(JOBS, "done", f".{tall}.partial")
    try:
        reason, details = make(models, data, job["loop"], job["tall"], partial, tall_partial)
        details["version"] = job.get("version")
        os.rename(tall_partial, os.path.join(JOBS, "done", tall))
        if reason:
            return {"id": job_id, "ok": True, "skipped": reason, "tall": tall, "details": details}
        os.rename(partial, os.path.join(JOBS, "done", video))
        return {"id": job_id, "ok": True, "video": video, "tall": tall, "details": details}
    finally:
        for path in (partial, tall_partial):
            if os.path.exists(path):
                os.remove(path)


def main():
    if len(sys.argv) >= 5 and sys.argv[1] == "--try":
        models = Models(DEVICE)
        loop = {"seconds": 8, "fps": 30, "max_side": 1440, "path": "drift", "strength": 0.03}
        tall = {"width": 1080, "height": 1920}
        with open(sys.argv[2], "rb") as f:
            reason, details = make(models, f.read(), loop, tall, sys.argv[3], sys.argv[4], force="--force" in sys.argv)
        print(json.dumps({"skipped": reason, **details}))
        return 0
    for sub in ("queue", "running", "done"):
        os.makedirs(os.path.join(JOBS, sub), exist_ok=True)
    # A job a restart cut off is taken up again.
    for name in os.listdir(os.path.join(JOBS, "running")):
        os.rename(os.path.join(JOBS, "running", name), os.path.join(JOBS, "queue", name))
    log(f"loading models (downloaded on first use); depth on the {DEVICE}")
    models = Models(DEVICE)
    log(f"ready ({models.providers[0]})")
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
        cover = None
        try:
            with open(running) as f:
                job = json.load(f)
            job_id = str(job.get("id", job_id))
            cover = job.get("cover")
            body = work(models, job, job_id)
            answer(job_id, body)
            log(f"job {job_id}: {body.get('skipped') or 'made'} in {body['details']['seconds']} s")
        except Exception as err:  # Any failure is the job's answer, never the worker's end.
            answer(job_id, {"id": job_id, "ok": False, "error": f"{type(err).__name__}: {err}"[:500]})
            log(f"job {job_id} failed:\n{traceback.format_exc()}")
        finally:
            os.remove(running)
            if cover and "/" not in cover and not cover.startswith("."):
                path = os.path.join(JOBS, "queue", cover)
                if os.path.exists(path):
                    os.remove(path)


if __name__ == "__main__":
    tempfile.tempdir = os.environ.get("TMPDIR", "/tmp")
    sys.exit(main())
