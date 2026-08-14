FROM python:3.13-slim

# Keep Python lean and unbuffered for clean container logs.
ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PIP_NO_CACHE_DIR=1

WORKDIR /app

# Fonts for the server-rendered images (Pillow ships no bundled font): DejaVu for
# Latin + Cyrillic, Noto Sans CJK for Japanese/Chinese (localized board/announcement
# banners). Without the CJK font, JA/ZH glyphs render as tofu boxes.
# Fonts (above) plus the OpenCV runtime libs RapidOCR/onnxruntime need for the
# self-hosted character-stat OCR (libGL + glib); without them the import fails with
# "libGL.so.1: cannot open shared object file".
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        fonts-dejavu-core fonts-noto-cjk libgl1 libglib2.0-0 \
    && rm -rf /var/lib/apt/lists/*

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