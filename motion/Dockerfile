# Melete's motion cover renderer (decision 124): makes a short loop from an album's still
# cover, taking jobs as files Melete writes. Python because the models run in ONNX Runtime and
# OpenCV; Melete itself stays in Rust. The stem separator's base image, so the two share its
# layers on disk; pinned by digest and by version, as everything is.
FROM pytorch/pytorch:2.7.1-cuda12.8-cudnn9-runtime@sha256:c16f4c749e2d9e96878875cdf6cc45cddda1d1a36fddd371dd6f2360f1b6e2a2

# ffmpeg encodes the loops.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg \
    && rm -rf /var/lib/apt/lists/*
COPY requirements.txt /app/requirements.txt
RUN pip install --no-cache-dir -r /app/requirements.txt
COPY render.py /app/render.py

# Melete's own user, so it can read what's written; the models download once into /models.
RUN useradd --uid 10001 --no-create-home --shell /usr/sbin/nologin melete
USER 10001
ENV HOME=/tmp PYTHONUNBUFFERED=1
ENTRYPOINT ["python", "/app/render.py"]
