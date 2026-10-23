# Rust (native/): the abi3 wheel the Python app imports, and kiwi-server, the
# front door that answers ported routes and proxies the rest to Python.
FROM python:3.13-slim AS native
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
# The target dir is a cache mount, so the binary is copied out in the same step.
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/native/target \
    cd /native && cargo build --release --locked -p kiwi-server \
    && install -D target/release/kiwi-server /out/kiwi-server \
    && install -D target/release/kiwi-builder /out/kiwi-builder \
    && install -D target/release/kiwi-bot /out/kiwi-bot

FROM python:3.13-slim AS app

# Keep Python lean and unbuffered for clean container logs.
ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PIP_NO_CACHE_DIR=1

WORKDIR /app

# Fonts for the server-rendered images (Pillow ships no bundled font): DejaVu for
# Latin + Cyrillic, Noto Sans CJK for Japanese/Chinese and Noto Sans Thai (in
# fonts-noto-core) for Thai (localized board/announcement banners). Without them,
# JA/ZH/TH glyphs render as tofu boxes.
# Fonts (above) plus the OpenCV runtime libs RapidOCR/onnxruntime need for the
# self-hosted character-stat OCR (libGL + glib); without them the import fails with
# "libGL.so.1: cannot open shared object file".
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        fonts-dejavu-core fonts-noto-cjk fonts-noto-core libgl1 libglib2.0-0 \
        default-jre-headless git \
    && rm -rf /var/lib/apt/lists/*

# JPEXS FFDec, the ActionScript decompiler behind the Mods Hub code view (see
# app/trove/swf/decompile.py). The JRE above is here for this and nothing else.
#
# Pinned by version AND checksum: this runs over .swf files strangers upload, so
# what lands in the image has to be exactly the build that was reviewed, not
# whatever the release page serves on the day of a rebuild. Bump both together -
# and bump bp_cache.SCRIPT_VERSION with them if the new build's output is worth
# rebuilding the cached source for.
#
# It ships as a jar plus a lib/ tree its manifest classpath points at, so the whole
# archive is extracted, not just the jar.
ARG FFDEC_VERSION=26.2.1
ARG FFDEC_SHA256=0333b56998a55bd83f4e0deb678a811fcdc45607582b4f5dd438309c8c3ad5ce
RUN python -c 'import hashlib,io,sys,urllib.request,zipfile;v=sys.argv[1];w=sys.argv[2];u="https://github.com/jindrapetrik/jpexs-decompiler/releases/download/version"+v+"/ffdec_"+v+".zip";b=urllib.request.urlopen(u,timeout=300).read();g=hashlib.sha256(b).hexdigest();sys.exit("ffdec checksum mismatch: "+g) if g!=w else None;zipfile.ZipFile(io.BytesIO(b)).extractall("/opt/ffdec")' \
        "$FFDEC_VERSION" "$FFDEC_SHA256" \
    && test -f /opt/ffdec/ffdec.jar

# Install dependencies first so they cache independently of source changes.
COPY requirements.txt constraints.txt ./
RUN pip install --no-cache-dir -r requirements.txt -c constraints.txt

# Bake the RapidOCR models into the image so the OCR endpoint needs no network
# (and no writable cache) at runtime - otherwise the first /v1/ocr/character call
# would download ~15 MB of ONNX models. Constructing RapidOCR() fetches them.
RUN python -c "from rapidocr import RapidOCR; RapidOCR()"

# kiwi-server's OCR (ort, load-dynamic) runs the very onnxruntime build and ONNX
# files Python uses: the pinned wheel's own libonnxruntime and the models above.
ENV KIWI_ORT_DYLIB=/usr/local/lib/python3.13/site-packages/onnxruntime/capi/libonnxruntime.so.1.26.0 \
    KIWI_OCR_MODEL_DIR=/usr/local/lib/python3.13/site-packages/rapidocr/models
RUN test -f "$KIWI_ORT_DYLIB" \
    && for m in ch_PP-OCRv4_det_mobile ch_ppocr_mobile_v2.0_cls_mobile ch_PP-OCRv4_rec_mobile; do \
        test -f "$KIWI_OCR_MODEL_DIR/$m.onnx" || exit 1; done

COPY --from=native /wheels /tmp/wheels
RUN pip install --no-cache-dir /tmp/wheels/*.whl && rm -rf /tmp/wheels

COPY --from=native /out/kiwi-server /usr/local/bin/kiwi-server
COPY --from=native /out/kiwi-bot /usr/local/bin/kiwi-bot

COPY app ./app

EXPOSE 8000

# The Python app on its own (compose runs kiwi-server as `api` and this as `api-py`).
# --proxy-headers: trust X-Forwarded-For only from $FORWARDED_ALLOW_IPS (uvicorn reads
# it; default 127.0.0.1), so request.client.host is the real client, not the proxy.
# --timeout-graceful-shutdown: the endless SSE streams would otherwise hold the
# graceful shutdown open until docker's stop timeout SIGKILLs us (see compose).
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000", \
     "--proxy-headers", \
     "--timeout-graceful-shutdown", "3"]

# ---------------------------------------------------------------------------
# The custom art worker, which is the app image plus SteamCMD.
#
# It is a stage here rather than its own Dockerfile so it cannot be built against
# a stale trove-app:latest: a separate file would have to FROM the tag the api
# service publishes, and nothing orders the two builds.
#
# SteamCMD is a 32-bit binary, hence the i386 architecture and lib32gcc. The
# login is NOT baked in - it lives in the steamcmd-home volume, put there once by
# hand, and is read from $HOME/Steam at run time.
FROM app AS builder

RUN dpkg --add-architecture i386 \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl lib32gcc-s1 \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /opt/steamcmd \
    && curl -sSLf https://steamcdn-a.akamaihd.net/client/installer/steamcmd_linux.tar.gz \
       | tar zxf - -C /opt/steamcmd \
    && test -x /opt/steamcmd/steamcmd.sh

COPY --from=native /out/kiwi-builder /usr/local/bin/kiwi-builder

# KIWI_BUILDER=rust runs the Rust worker; anything else keeps the Python one.
CMD ["sh", "-c", "if [ \"$KIWI_BUILDER\" = rust ]; then exec kiwi-builder; fi; exec python -m app.custom_art.builder"]
