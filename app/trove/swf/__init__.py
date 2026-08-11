"""SWF asset extraction - pull the embedded artwork out of a Flash movie.

Trove's interface ships as ``ui/*.swf``. Every icon, panel and background in
those screens is a bitmap tag inside the movie, so the file is really an
undocumented art bundle. :mod:`app.trove.swf.extract` reads the tag stream and
hands back those bitmaps as PNGs.
"""

from app.trove.swf.extract import (
    SwfError,
    SwfHeader,
    SwfImage,
    extract_images,
    read_header,
    summarize,
)

__all__ = [
    "SwfError",
    "SwfHeader",
    "SwfImage",
    "extract_images",
    "read_header",
    "summarize",
]
