/* Thin FreeType accessors for kiwi-pil, so Rust needs no FreeType struct layouts. */
#include <stdlib.h>
#include <ft2build.h>
#include FT_FREETYPE_H
#include FT_GLYPH_H
#include FT_BITMAP_H

typedef struct {
    FT_Library lib;
    FT_Bitmap conv;
    int conv_ready;
} kp_lib;

typedef struct {
    int left, top;
    unsigned int rows, width;
    int pitch, pixel_mode, scale;
    const unsigned char *buffer;
} kp_bitmap;

int kp_lib_new(kp_lib **out) {
    kp_lib *l = (kp_lib *)calloc(1, sizeof(kp_lib));
    int e;
    if (!l) {
        return FT_Err_Out_Of_Memory;
    }
    e = FT_Init_FreeType(&l->lib);
    if (e) {
        free(l);
        return e;
    }
    *out = l;
    return 0;
}

void kp_lib_free(kp_lib *l) {
    if (l->conv_ready) {
        FT_Bitmap_Done(l->lib, &l->conv);
    }
    FT_Done_FreeType(l->lib);
    free(l);
}

/* _imagingft.c getfont(): memory face plus a nominal size request of size*64. */
int kp_face_new(kp_lib *l, const unsigned char *data, long len, long index, float size, FT_Face *out) {
    FT_Face face;
    FT_Size_RequestRec req;
    FT_Long width;
    int e = FT_New_Memory_Face(l->lib, data, len, index, &face);
    if (e) {
        return e;
    }
    width = size * 64;
    req.type = FT_SIZE_REQUEST_TYPE_NOMINAL;
    req.width = width;
    req.height = width;
    req.horiResolution = 0;
    req.vertResolution = 0;
    e = FT_Request_Size(face, &req);
    if (e) {
        FT_Done_Face(face);
        return e;
    }
    *out = face;
    return 0;
}

void kp_face_free(FT_Face face) {
    FT_Done_Face(face);
}

int kp_has_kerning(FT_Face face) {
    return FT_HAS_KERNING(face) ? 1 : 0;
}

void kp_size_metrics(FT_Face face, long *ascender, long *descender) {
    *ascender = face->size->metrics.ascender;
    *descender = face->size->metrics.descender;
}

unsigned int kp_char_index(FT_Face face, unsigned long ch) {
    return FT_Get_Char_Index(face, ch);
}

int kp_load_advance(FT_Face face, unsigned int index, int flags, long *advance) {
    int e = FT_Load_Glyph(face, index, flags);
    if (!e) {
        *advance = face->glyph->metrics.horiAdvance;
    }
    return e;
}

int kp_kerning(FT_Face face, unsigned int left, unsigned int right, long *dx, long *dy) {
    FT_Vector delta;
    int e = FT_Get_Kerning(face, left, right, FT_KERNING_DEFAULT, &delta);
    if (!e) {
        *dx = delta.x;
        *dy = delta.y;
    }
    return e;
}

/* FT_Glyph_Get_CBox(FT_GLYPH_BBOX_PIXELS) of the loaded glyph: xMin, yMin, xMax, yMax. */
int kp_cbox(FT_Face face, unsigned int index, int flags, long *box) {
    FT_Glyph glyph;
    FT_BBox bbox;
    int e = FT_Load_Glyph(face, index, flags);
    if (e) {
        return e;
    }
    e = FT_Get_Glyph(face->glyph, &glyph);
    if (e) {
        return e;
    }
    FT_Glyph_Get_CBox(glyph, FT_GLYPH_BBOX_PIXELS, &bbox);
    box[0] = bbox.xMin;
    box[1] = bbox.yMin;
    box[2] = bbox.xMax;
    box[3] = bbox.yMax;
    FT_Done_Glyph(glyph);
    return 0;
}

/* FT_Load_Glyph(flags | FT_LOAD_RENDER); 1/2/4-bit bitmaps are converted to 8-bit
   the way font_render does, with the scale it multiplies by. */
int kp_render(kp_lib *l, FT_Face face, unsigned int index, int flags, kp_bitmap *out) {
    FT_GlyphSlot slot;
    FT_Bitmap bitmap;
    int e = FT_Load_Glyph(face, index, flags | FT_LOAD_RENDER);
    if (e) {
        return e;
    }
    slot = face->glyph;
    bitmap = slot->bitmap;
    out->left = slot->bitmap_left;
    out->top = slot->bitmap_top;
    out->scale = 1;
    if (bitmap.buffer) {
        switch (bitmap.pixel_mode) {
            case FT_PIXEL_MODE_MONO:
                out->scale = 255;
                break;
            case FT_PIXEL_MODE_GRAY2:
                out->scale = 255 / 3;
                break;
            case FT_PIXEL_MODE_GRAY4:
                out->scale = 255 / 15;
                break;
            default:
                break;
        }
        if (bitmap.pixel_mode == FT_PIXEL_MODE_MONO || bitmap.pixel_mode == FT_PIXEL_MODE_GRAY2 ||
            bitmap.pixel_mode == FT_PIXEL_MODE_GRAY4) {
            if (!l->conv_ready) {
                FT_Bitmap_Init(&l->conv);
                l->conv_ready = 1;
            }
            e = FT_Bitmap_Convert(l->lib, &bitmap, &l->conv, 1);
            if (e) {
                return e;
            }
            bitmap = l->conv;
        }
    }
    out->rows = bitmap.rows;
    out->width = bitmap.width;
    out->pitch = bitmap.pitch;
    out->pixel_mode = bitmap.pixel_mode;
    out->buffer = bitmap.buffer;
    return 0;
}
