# Three images (compose targets):
#   rust    (trove-rust:latest)    kiwi-server, kiwi-web and kiwi-bot on slim Debian, no Python.
#   builder (trove-builder:latest) the rust image plus SteamCMD and TroveUI's release tooling.
#   python  (trove-app:latest)     the Python fallback services (api-py, web-py, bot-py) until
#                                  they are removed. It keeps the kiwi binaries so the parity
#                                  sandbox can run both sides from one image.

# onnxruntime for OCR: the same version as the onnxruntime pin in requirements.txt.
ARG ORT_VERSION=1.26.0

# Built on the release the runtime uses, so the glibc matches.
FROM debian:trixie-slim AS native
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl build-essential \
    && rm -rf /var/lib/apt/lists/* \
    && curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.97.1
ENV PATH=/root/.cargo/bin:$PATH
COPY native /native
# The target dir is a cache mount, so the binaries are copied out in the same step.
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/native/target \
    cd /native && cargo build --release --locked -p kiwi-server \
    && install -D -t /out target/release/kiwi-server target/release/kiwi-web \
       target/release/kiwi-bot target/release/kiwi-builder

# kiwi-py, the abi3 wheel the Python image imports.
FROM python:3.13-slim-trixie AS wheel
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl build-essential \
    && rm -rf /var/lib/apt/lists/* \
    && curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.97.1
ENV PATH=/root/.cargo/bin:$PATH
RUN pip install --no-cache-dir "maturin>=1.15,<2"
COPY native /native
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/native/target \
    cd /native/kiwi-py && maturin build --release --locked -o /wheels

# Third-party files, each pinned by version AND sha256: these must be exactly the
# builds that were reviewed, not whatever a release page serves on the day of a rebuild.
FROM debian:trixie-slim AS fetch
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*

# JPEXS FFDec, the ActionScript decompiler behind the Mods Hub code view. It runs
# over .swf files strangers upload. Bump bp_cache.SCRIPT_VERSION with it if the new
# build's output is worth rebuilding the cached source for. The jar's manifest
# classpath points at lib/, so the whole archive is extracted.
ARG FFDEC_VERSION=26.2.1
ARG FFDEC_SHA256=0333b56998a55bd83f4e0deb678a811fcdc45607582b4f5dd438309c8c3ad5ce
RUN curl -fsSL -o /tmp/ffdec.zip \
        "https://github.com/jindrapetrik/jpexs-decompiler/releases/download/version${FFDEC_VERSION}/ffdec_${FFDEC_VERSION}.zip" \
    && echo "${FFDEC_SHA256}  /tmp/ffdec.zip" | sha256sum -c - \
    && unzip -q /tmp/ffdec.zip -d /opt/ffdec \
    && rm /tmp/ffdec.zip \
    && test -f /opt/ffdec/ffdec.jar

ARG ORT_VERSION
ARG ORT_SHA256=1254da24fb389cf39dc0ff3451ab48301740ffbfcbaf646849df92f80ee92c57
RUN curl -fsSL -o /tmp/ort.tgz \
        "https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/onnxruntime-linux-x64-${ORT_VERSION}.tgz" \
    && echo "${ORT_SHA256}  /tmp/ort.tgz" | sha256sum -c - \
    && mkdir -p /opt/onnxruntime \
    && tar xzf /tmp/ort.tgz -C /opt/onnxruntime --strip-components=1 \
    && rm /tmp/ort.tgz \
    && test -f "/opt/onnxruntime/lib/libonnxruntime.so.${ORT_VERSION}"

# The det/cls/rec models RapidOCR 3.8.4's RapidOCR() downloads (its default_models.yaml).
ARG RAPIDOCR_URL=https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.8.0/onnx/PP-OCRv4
RUN mkdir -p /opt/rapidocr/models && cd /opt/rapidocr/models \
    && curl -fsSL -o ch_PP-OCRv4_det_mobile.onnx "$RAPIDOCR_URL/det/ch_PP-OCRv4_det_mobile.onnx" \
    && curl -fsSL -o ch_ppocr_mobile_v2.0_cls_mobile.onnx "$RAPIDOCR_URL/cls/ch_ppocr_mobile_v2.0_cls_mobile.onnx" \
    && curl -fsSL -o ch_PP-OCRv4_rec_mobile.onnx "$RAPIDOCR_URL/rec/ch_PP-OCRv4_rec_mobile.onnx" \
    && printf '%s\n' \
        "d2a7720d45a54257208b1e13e36a8479894cb74155a5efe29462512d42f49da9  ch_PP-OCRv4_det_mobile.onnx" \
        "e47acedf663230f8863ff1ab0e64dd2d82b838fceb5957146dab185a89d6215c  ch_ppocr_mobile_v2.0_cls_mobile.onnx" \
        "48fc40f24f6d2a207a2b1091d3437eb3cc3eb6b676dc3ef9c37384005483683b  ch_PP-OCRv4_rec_mobile.onnx" \
       | sha256sum -c -

# The tzdata 2025.2 wheel's zoneinfo tree: Python's zoneinfo falls back to it for
# keys the system tzdata lacks (US/Eastern and the other legacy names).
ARG TZDATA_URL=https://files.pythonhosted.org/packages/5c/23/c7abc0ca0a1526a0774eca151daeb8de62ec457e77262b66b359c3c7679e/tzdata-2025.2-py2.py3-none-any.whl
ARG TZDATA_SHA256=1a403fada01ff9221ca8044d701868fa132215d84beb92242d9acd2147f667a8
RUN curl -fsSL -o /tmp/tzdata.whl "$TZDATA_URL" \
    && echo "${TZDATA_SHA256}  /tmp/tzdata.whl" | sha256sum -c - \
    && unzip -q /tmp/tzdata.whl 'tzdata/zoneinfo/*' -d /opt \
    && rm /tmp/tzdata.whl \
    && test -f /opt/tzdata/zoneinfo/US/Eastern

# ---------------------------------------------------------------------------
# The Python fallback image. Fonts for Pillow's renders (DejaVu: Latin + Cyrillic,
# Noto Sans CJK, Noto Sans Thai in fonts-noto-core), libGL + glib for RapidOCR's
# OpenCV, the JRE for FFDec, git.
FROM python:3.13-slim-trixie AS python

ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PIP_NO_CACHE_DIR=1

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        fonts-dejavu-core fonts-noto-cjk fonts-noto-core libgl1 libglib2.0-0 \
        default-jre-headless git \
    && rm -rf /var/lib/apt/lists/*

COPY requirements.txt constraints.txt ./
RUN pip install --no-cache-dir -r requirements.txt -c constraints.txt

# Bake the RapidOCR models so the OCR endpoint needs no network at runtime.
RUN python -c "from rapidocr import RapidOCR; RapidOCR()"

# kiwi-server in this image (the parity sandbox) runs the wheel's onnxruntime.
ENV KIWI_ORT_DYLIB=/usr/local/lib/python3.13/site-packages/onnxruntime/capi/libonnxruntime.so.1.26.0 \
    KIWI_OCR_MODEL_DIR=/usr/local/lib/python3.13/site-packages/rapidocr/models
RUN test -f "$KIWI_ORT_DYLIB" \
    && for m in ch_PP-OCRv4_det_mobile ch_ppocr_mobile_v2.0_cls_mobile ch_PP-OCRv4_rec_mobile; do \
        test -f "$KIWI_OCR_MODEL_DIR/$m.onnx" || exit 1; done

COPY --from=fetch /opt/ffdec /opt/ffdec

COPY --from=wheel /wheels /tmp/wheels
RUN pip install --no-cache-dir /tmp/wheels/*.whl && rm -rf /tmp/wheels

COPY --from=native /out/ /usr/local/bin/

COPY app ./app

EXPOSE 8000

# --proxy-headers: trust X-Forwarded-For only from $FORWARDED_ALLOW_IPS.
# --timeout-graceful-shutdown: the endless SSE streams would otherwise hold the
# graceful shutdown open until docker's stop timeout SIGKILLs us.
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000", \
     "--proxy-headers", \
     "--timeout-graceful-shutdown", "3"]

# ---------------------------------------------------------------------------
# The Rust image: api (kiwi-server), web (kiwi-web) and bot (kiwi-bot).
# ca-certificates, netbase and tzdata are what python:slim adds to Debian; LANG
# keeps the JVM's file-name encoding UTF-8 like there.
FROM debian:trixie-slim AS rust
ARG ORT_VERSION
ENV LANG=C.UTF-8
RUN mkdir -p /usr/share/man/man1 \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates netbase tzdata \
        fonts-dejavu-core fonts-noto-cjk fonts-noto-core \
        default-jre-headless git \
    && rm -rf /var/lib/apt/lists/*

COPY --from=fetch /opt/ffdec /opt/ffdec
COPY --from=fetch /opt/onnxruntime/lib /opt/onnxruntime/lib
COPY --from=fetch /opt/rapidocr/models /opt/rapidocr/models
COPY --from=fetch /opt/tzdata/zoneinfo /opt/tzdata/zoneinfo
ENV KIWI_ORT_DYLIB=/opt/onnxruntime/lib/libonnxruntime.so.${ORT_VERSION} \
    KIWI_OCR_MODEL_DIR=/opt/rapidocr/models \
    KIWI_TZDATA_DIR=/opt/tzdata/zoneinfo

# The data files the binaries read, relative to /app (assets/). site/ is bind-mounted.
WORKDIR /app
COPY assets assets

COPY --from=native /out/ /usr/local/bin/

EXPOSE 8000
CMD ["kiwi-server"]

# ---------------------------------------------------------------------------
# The custom art worker: the rust image plus SteamCMD and what TroveUI's own
# release scripts need, since the worker runs them (custom_art.py, release.py,
# trovesaurus.py and each mod's build.py): a Python with the pinned Pillow,
# requests, Beautiful Soup and pydantic, and the app.trove.tmod they import.
#
# It is a stage here rather than its own Dockerfile so it cannot be built against
# a stale image. SteamCMD is 32-bit, hence i386 and lib32gcc. The Steam login is
# NOT baked in: it lives in the steamcmd-home volume, read from $HOME/Steam.
FROM rust AS builder

RUN dpkg --add-architecture i386 \
    && apt-get update \
    && apt-get install -y --no-install-recommends curl lib32gcc-s1 python3-venv \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /opt/steamcmd \
    && curl -sSLf https://steamcdn-a.akamaihd.net/client/installer/steamcmd_linux.tar.gz \
       | tar zxf - -C /opt/steamcmd \
    && test -x /opt/steamcmd/steamcmd.sh

COPY constraints.txt /tmp/constraints.txt
RUN python3 -m venv /opt/troveui \
    && /opt/troveui/bin/pip install --no-cache-dir -c /tmp/constraints.txt \
        pillow requests beautifulsoup4 pydantic \
    && rm /tmp/constraints.txt
COPY app/__init__.py app/
COPY app/trove/__init__.py app/trove/tmod.py app/trove/troveio.py app/trove/mod_categories.py app/trove/
ENV KIWI_BUILDER_PYTHON=/opt/troveui/bin/python

CMD ["kiwi-builder"]
