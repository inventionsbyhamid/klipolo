# Relay image: the API plus the MMS forced aligner on CPU.
FROM python:3.12-slim-bookworm

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PIP_NO_CACHE_DIR=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    TORCH_HOME=/opt/torch \
    PYTHONPATH=/app
WORKDIR /app

# Every package is pinned in the lock files and installed with --no-deps,
# so a rebuild installs exactly the same versions.
COPY requirements-torch.lock ./
RUN pip install --no-deps --index-url https://download.pytorch.org/whl/cpu -r requirements-torch.lock

# The MMS_FA weights (1.2 GB) go into the image, so the server never
# downloads anything at runtime.
RUN python -c "import torchaudio; torchaudio.pipelines.MMS_FA.get_model(with_star=False)" \
    && chmod -R a+rX /opt/torch

COPY requirements.lock ./
RUN pip install --no-deps -r requirements.lock && pip check

COPY relay ./relay
RUN python -m compileall -q relay

RUN useradd --uid 10001 --no-create-home --shell /usr/sbin/nologin relay \
    && mkdir -p /data \
    && chown relay /data
USER relay

ENV RELAY_DB_PATH=/data/relay.db \
    ALIGN_WARMUP=1
EXPOSE 8080
# One worker: in-flight limits and the alignment queue live in the process.
CMD ["uvicorn", "--factory", "relay.app:create_app", "--host", "0.0.0.0", "--port", "8080", "--workers", "1"]
