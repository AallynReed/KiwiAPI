FROM python:3.13-slim

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
        default-jre-headless \
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
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Bake the RapidOCR models into the image so the OCR endpoint needs no network
# (and no writable cache) at runtime - otherwise the first /v1/ocr/character call
# would download ~15 MB of ONNX models. Constructing RapidOCR() fetches them.
RUN python -c "from rapidocr import RapidOCR; RapidOCR()"

COPY app ./app

EXPOSE 8000

# --proxy-headers / --forwarded-allow-ips: trust the reverse proxy in front of us
# so request.client.host and the URL scheme reflect the real client, not the proxy.
# --timeout-graceful-shutdown: the endless SSE streams would otherwise hold the
# graceful shutdown open until docker's stop timeout SIGKILLs us (see compose).
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000", \
     "--proxy-headers", "--forwarded-allow-ips", "*", \
     "--timeout-graceful-shutdown", "3"]