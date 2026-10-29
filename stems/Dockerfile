# Melete's stem separator (decision 84): splits each music track into voice, drums, bass,
# guitar and other on the GPU, taking jobs as files Melete writes. Python because the models
# run in PyTorch; Melete itself stays in Rust. Pinned by digest and by version, as everything is.
FROM pytorch/pytorch:2.7.1-cuda12.8-cudnn9-runtime@sha256:c16f4c749e2d9e96878875cdf6cc45cddda1d1a36fddd371dd6f2360f1b6e2a2

# ffmpeg decodes every format the library holds; a compiler is needed once, to build diffq.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg build-essential \
    && rm -rf /var/lib/apt/lists/*
COPY requirements.txt /app/requirements.txt
RUN pip install --no-cache-dir -r /app/requirements.txt \
    && apt-get purge -y build-essential && apt-get autoremove -y
COPY separate.py /app/separate.py

# Melete's own user, so it can read what's written; the models download once into /models.
RUN useradd --uid 10001 --no-create-home --shell /usr/sbin/nologin melete
USER 10001
ENV HOME=/tmp PYTHONUNBUFFERED=1
ENTRYPOINT ["python", "/app/separate.py"]
