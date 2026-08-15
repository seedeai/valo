/* valo C API — embed the valo renderer from any language that speaks C.
 *
 * Conventions (uniform across the whole surface):
 * - Every object is an OPAQUE HANDLE from a `valo_*_new`-style call,
 *   released by the matching `valo_*_dispose`. Handles are not thread-safe.
 * - Every function is NULL-SAFE: a null handle is a no-op / returns zero.
 * - Geometry and paint travel BY VALUE; strings are UTF-8 (pointer, byte
 *   length), never NUL-terminated; angles are radians; colors are
 *   straight-alpha floats in [0, 1]; text offsets are UTF-8 BYTE offsets.
 *
 * Kept by hand, guarded by valo-capi's symbol-parity test.
 */
#ifndef VALO_H
#define VALO_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ── opaque handles ─────────────────────────────────────────────────── */

typedef struct ValoContext ValoContext;
typedef struct ValoDisplayListBuilder ValoDisplayListBuilder;
typedef struct ValoDisplayList ValoDisplayList;
typedef struct ValoPath ValoPath;
typedef struct ValoImage ValoImage;
typedef struct ValoFontCollection ValoFontCollection;
typedef struct ValoParagraphBuilder ValoParagraphBuilder;
typedef struct ValoParagraph ValoParagraph;
typedef struct ValoSystemFonts ValoSystemFonts;
typedef struct ValoColorFilter ValoColorFilter;

/* ── by-value types ─────────────────────────────────────────────────── */

typedef struct ValoColor {
  float red, green, blue, alpha;
} ValoColor;

typedef struct ValoRect {
  float x, y, width, height;
} ValoRect;

typedef struct ValoPoint {
  float x, y;
} ValoPoint;

/* Row-major 2x3 affine transform (the CSS/Skia 6-tuple). */
typedef struct ValoTransform {
  float a, b, c, d, translate_x, translate_y;
} ValoTransform;

/* Per-corner elliptical radii, clockwise from top-left; circular corners
 * are x == y. */
typedef struct ValoCornerRadii {
  float top_left_x, top_left_y;
  float top_right_x, top_right_y;
  float bottom_right_x, bottom_right_y;
  float bottom_left_x, bottom_left_y;
} ValoCornerRadii;

/* blend_mode — the 29 Skia modes, in this order:
 *  0 clear · 1 src · 2 dst · 3 srcOver · 4 dstOver · 5 srcIn · 6 dstIn ·
 *  7 srcOut · 8 dstOut · 9 srcAtop · 10 dstAtop · 11 xor · 12 plus ·
 *  13 modulate · 14 screen · 15 overlay · 16 darken · 17 lighten ·
 *  18 colorDodge · 19 colorBurn · 20 hardLight · 21 softLight ·
 *  22 difference · 23 exclusion · 24 multiply · 25 hue · 26 saturation ·
 *  27 color · 28 luminosity
 * style: 0 fill · 1 stroke. stroke_cap: 0 butt · 1 round · 2 square.
 * stroke_join: 0 miter · 1 round · 2 bevel. mask_blur_sigma <= 0 = none;
 * mask_blur_style: 0 normal · 1 solid · 2 inner · 3 outer. */
typedef struct ValoPaint {
  ValoColor color;
  int32_t blend_mode;
  int32_t style;
  float stroke_width;
  int32_t stroke_cap;
  int32_t stroke_join;
  float stroke_miter_limit;
  int32_t mask_blur_style;
  float mask_blur_sigma;
  /* Borrowed, or NULL. Recolours what this paint drew, before mask_blur
   * spreads it. The handle only has to outlive the call. */
  const ValoColorFilter *color_filter;
} ValoPaint;

/* families_utf8 is a NEWLINE-separated family list, tried in order per
 * glyph. line_height <= 0 uses the font's own metrics. decoration_kind:
 * -1 none · 0 underline · 1 lineThrough · 2 overline;
 * decoration_color.alpha <= 0 inherits the text color. */
typedef struct ValoTextStyle {
  const uint8_t *families_utf8;
  size_t families_length;
  float size;
  uint32_t weight; /* CSS weight, 100-900 */
  bool italic;
  ValoColor color;
  float letter_spacing;
  float word_spacing;
  float line_height;
  int32_t decoration_kind;
  ValoColor decoration_color;
  float decoration_thickness;
} ValoTextStyle;

/* align: 0 left · 1 center · 2 right · 3 justify. max_lines 0 = unlimited.
 * A null/empty ellipsis truncates without one. */
typedef struct ValoParagraphStyle {
  int32_t align;
  uint32_t max_lines;
  const uint8_t *ellipsis_utf8;
  size_t ellipsis_length;
} ValoParagraphStyle;

typedef struct ValoTextRange {
  size_t start, end;
} ValoTextRange;

typedef struct ValoLineMetrics {
  size_t start, end;
  float baseline, ascent, descent, left, width;
} ValoLineMetrics;

/* ── context: GPU bring-up, surfaces, images ────────────────────────── */

/* Null when no GPU adapter exists. */
ValoContext *valo_context_new(void);
void valo_context_dispose(ValoContext *context);

/* Attach a presentable surface over a raw CAMetalLayer* (macOS/iOS);
 * replaces a previous surface. False on failure. */
bool valo_context_attach_metal_layer(ValoContext *context, void *metal_layer,
                                     uint32_t width, uint32_t height);
void valo_context_resize(ValoContext *context, uint32_t width,
                         uint32_t height);

/* The Metal device the context renders with (macOS only) — hand it to a
 * CAMetalLayer so externally-owned swapchain textures share the GPU
 * device. Borrowed: valid while the context lives, not retained. */
void *valo_context_metal_device(ValoContext *context);

/* Render one frame into a caller-owned MTLTexture* (macOS only) — the
 * external-swapchain route: the embedder drives the drawable cycle, valo
 * only draws. format: 0 bgra8unorm · 1 rgba8unorm, matching the texture.
 * The texture must allow copies (set the layer's framebufferOnly to
 * false) — dst-reading blends snapshot the target. Returns after
 * SUBMISSION: presenting right after is safe (the display waits for the
 * drawable's GPU writes on its own); call valo_context_wait_for_gpu
 * before reading the texture from the CPU. */
bool valo_context_render_to_metal_texture(ValoContext *context,
                                          const ValoDisplayList *list,
                                          ValoColor clear, void *texture,
                                          uint32_t width, uint32_t height,
                                          int32_t format);

/* Block until every submitted frame finished on the GPU — only before
 * CPU reads of a rendered texture (tests, exports); frame loops must NOT
 * call this. */
void valo_context_wait_for_gpu(ValoContext *context);

/* Wrap a caller-owned MTLTexture* as a drawable image, zero-copy (macOS
 * only) — external renderers draw straight into valo frames without a
 * readback. Needs shader-read usage; must outlive the image. format: 0
 * bgra8unorm · 1 rgba8unorm. */
ValoImage *valo_context_import_metal_texture(ValoContext *context,
                                             void *texture, uint32_t width,
                                             uint32_t height, int32_t format);

/* Render one frame onto the attached surface and present. False without a
 * surface or when the swapchain skipped the frame (both recoverable). */
bool valo_context_render(ValoContext *context, const ValoDisplayList *list,
                         ValoColor clear);

/* Headless render into caller-allocated straight-alpha RGBA8 pixels
 * (width * height * 4 bytes) — exports and golden tests. */
bool valo_context_render_to_pixels(ValoContext *context,
                                   const ValoDisplayList *list,
                                   ValoColor clear, uint32_t width,
                                   uint32_t height, uint8_t *out_pixels);

/* Upload straight-alpha RGBA8 pixels as a drawable image (mipmapped). */
ValoImage *valo_context_create_image(ValoContext *context, uint32_t width,
                                     uint32_t height, const uint8_t *pixels);
void valo_image_dispose(ValoImage *image);

/* ── display-list recording ─────────────────────────────────────────── */

ValoDisplayListBuilder *valo_builder_new(void);
/* Consumes the builder handle. */
ValoDisplayList *valo_builder_build(ValoDisplayListBuilder *builder);
void valo_builder_dispose(ValoDisplayListBuilder *builder);
void valo_display_list_dispose(ValoDisplayList *list);

void valo_builder_save(ValoDisplayListBuilder *builder);
/* A composited layer: the paint's alpha, blend mode, and mask blur apply
 * to the layer as a whole on restore. */
void valo_builder_save_layer(ValoDisplayListBuilder *builder, ValoPaint paint);
void valo_builder_restore(ValoDisplayListBuilder *builder);
void valo_builder_translate(ValoDisplayListBuilder *builder, float x, float y);
void valo_builder_scale(ValoDisplayListBuilder *builder, float x, float y);
void valo_builder_rotate(ValoDisplayListBuilder *builder, float radians);
void valo_builder_transform(ValoDisplayListBuilder *builder,
                            ValoTransform transform);
/* Concatenate a FULL column-major 4x4 (16 floats, the Flutter canvas
 * order) — perspective included; the z output is ignored for painting. */
void valo_builder_transform_matrix(ValoDisplayListBuilder *builder,
                                   const float *matrix);

/* operation: 0 intersect · 1 difference. rule: 0 nonZero · 1 evenOdd. */
void valo_builder_clip_rect(ValoDisplayListBuilder *builder, ValoRect rect,
                            int32_t operation);
void valo_builder_clip_rounded_rect(ValoDisplayListBuilder *builder,
                                    ValoRect rect, ValoCornerRadii radii,
                                    int32_t operation);
void valo_builder_clip_path(ValoDisplayListBuilder *builder, ValoPath *path,
                            int32_t rule, int32_t operation);

void valo_builder_draw_rect(ValoDisplayListBuilder *builder, ValoRect rect,
                            ValoPaint paint);
void valo_builder_draw_rounded_rect(ValoDisplayListBuilder *builder,
                                    ValoRect rect, ValoCornerRadii radii,
                                    ValoPaint paint);
void valo_builder_draw_circle(ValoDisplayListBuilder *builder, float center_x,
                              float center_y, float radius, ValoPaint paint);
void valo_builder_draw_path(ValoDisplayListBuilder *builder, ValoPath *path,
                            int32_t rule, ValoPaint paint);
/* sampling: 0 linear (mipmapped) · 1 nearest. */
void valo_builder_draw_image_rect(ValoDisplayListBuilder *builder,
                                  const ValoImage *image, ValoRect source,
                                  ValoRect destination, int32_t sampling,
                                  ValoPaint paint);
void valo_builder_draw_display_list(ValoDisplayListBuilder *builder,
                                    const ValoDisplayList *list);
void valo_builder_draw_paragraph(ValoDisplayListBuilder *builder,
                                 const ValoParagraph *paragraph, float x,
                                 float y);
/* Draw a paragraph with `paint` overriding every run's own fill — stroked or
 * blended text. Colour, style, stroke, blend mode, mask blur and colour
 * filter all apply; shadows and decorations still come from the paragraph's
 * styles. ValoPaint carries no shader, so gradient-filled text is not
 * reachable from C. A stroke width of 0 is a hairline (one device pixel);
 * only a negative width draws nothing. */
void valo_builder_draw_paragraph_with(ValoDisplayListBuilder *builder,
                                      const ValoParagraph *paragraph, float x,
                                      float y, ValoPaint paint);

/* ── colour filters ─────────────────────────────────────────────────── */

/* 20 row-major floats, a 4x5 over UNPREMULTIPLIED colour in 0..1:
 * out[row] = clamp(row . [r, g, b, a, 1]).
 *
 * Flutter's ColorFilter.matrix passes the translation column (entries 4, 9,
 * 14, 19) in unnormalized 0..255 space — divide those four by 255 first, or
 * every offset lands 255x too strong. Returns NULL for a NULL matrix. */
ValoColorFilter *valo_color_filter_matrix(const float *matrix);
/* Blend `color` AS THE SOURCE over what was drawn (Flutter's
 * ColorFilter.mode); `mode` indexes the same 29 modes as ValoPaint. */
ValoColorFilter *valo_color_filter_blend(ValoColor color, int32_t mode);
void valo_color_filter_dispose(ValoColorFilter *filter);

/* ── paths ──────────────────────────────────────────────────────────── */

ValoPath *valo_path_new(void);
void valo_path_dispose(ValoPath *path);
void valo_path_move_to(ValoPath *path, float x, float y);
void valo_path_line_to(ValoPath *path, float x, float y);
void valo_path_quadratic_to(ValoPath *path, float control_x, float control_y,
                            float x, float y);
void valo_path_cubic_to(ValoPath *path, float control1_x, float control1_y,
                        float control2_x, float control2_y, float x, float y);
void valo_path_close(ValoPath *path);
void valo_path_add_rect(ValoPath *path, ValoRect rect);
void valo_path_add_rounded_rect(ValoPath *path, ValoRect rect,
                                ValoCornerRadii radii);
void valo_path_add_circle(ValoPath *path, ValoPoint center, float radius);
/* Angles are radians from the +x axis; a positive sweep turns toward +y,
 * which is clockwise on screen. An open contour is joined to the arc's first
 * point by a line, a closed one starts there. */
void valo_path_add_arc(ValoPath *path, ValoPoint center, float radius,
                       float start_angle, float sweep_angle);
void valo_path_add_ellipse(ValoPath *path, ValoPoint center, float radius_x,
                           float radius_y, float x_axis_rotation,
                           float start_angle, float sweep_angle);
/* The circle of `radius` tangent to both the segment from the current point
 * to `corner` and the one from `corner` to `next`, reached by a line.
 * Degenerate input falls back to a line to `corner`. */
void valo_path_arc_to(ValoPath *path, ValoPoint corner, ValoPoint next,
                      float radius);
void valo_path_reset(ValoPath *path);
/* Hit test: `rule` 0 non-zero, 1 even-odd. */
bool valo_path_contains(ValoPath *path, ValoPoint point, int32_t rule);

/* ── text ───────────────────────────────────────────────────────────── */

ValoFontCollection *valo_fonts_new(void);
void valo_fonts_dispose(ValoFontCollection *fonts);
/* Register a face from TTF/OTF bytes; the face id, or -1 when the bytes
 * don't parse. Rebuild paragraphs to see newly added faces. */
int64_t valo_fonts_add(ValoFontCollection *fonts, const uint8_t *bytes,
                       size_t length);
void valo_fonts_add_fallback(ValoFontCollection *fonts, int64_t face_id);

/* Register every face a font file offers: static = one face; variable =
 * its NAMED INSTANCES, so weights select like a static multi-weight
 * family. add_as_fallbacks also appends each to the fallback chain
 * (default UI fonts). Returns faces added; 0 when the bytes don't parse. */
int32_t valo_fonts_add_instances(ValoFontCollection *fonts,
                                 const uint8_t *bytes, size_t length,
                                 bool add_as_fallbacks);

/* The family name of a registered face: writes up to `capacity` UTF-8
 * bytes, returns the TOTAL length (size with capacity 0); 0 if unknown. */
size_t valo_fonts_family_name(const ValoFontCollection *fonts,
                              int64_t face_id, uint8_t *out_utf8,
                              size_t capacity);

ValoParagraphBuilder *valo_paragraph_builder_new(
    const ValoFontCollection *fonts, ValoParagraphStyle style);
void valo_paragraph_builder_dispose(ValoParagraphBuilder *builder);
/* One styled span; embedders with style stacks resolve them first. */
void valo_paragraph_builder_add_text(ValoParagraphBuilder *builder,
                                     const uint8_t *text_utf8,
                                     size_t text_length, ValoTextStyle style);
/* Consumes the builder handle; layout before drawing or querying. The
 * collection resolves any missing families/codepoints from its own
 * sources during this call, and may grow. */
ValoParagraph *valo_paragraph_builder_build(ValoParagraphBuilder *builder,
                                            ValoFontCollection *fonts);
void valo_paragraph_dispose(ValoParagraph *paragraph);

/* Pass INFINITY for unconstrained width. */
void valo_paragraph_layout(ValoParagraph *paragraph, float max_width);
float valo_paragraph_width(const ValoParagraph *paragraph);
float valo_paragraph_height(const ValoParagraph *paragraph);
float valo_paragraph_longest_line(const ValoParagraph *paragraph);
float valo_paragraph_min_intrinsic_width(const ValoParagraph *paragraph);
float valo_paragraph_max_intrinsic_width(const ValoParagraph *paragraph);
size_t valo_paragraph_line_count(const ValoParagraph *paragraph);
bool valo_paragraph_did_exceed_max_lines(const ValoParagraph *paragraph);

ValoRect valo_paragraph_caret_for_offset(const ValoParagraph *paragraph,
                                         size_t byte_offset);
/* The byte offset nearest a paragraph-local point; out_downstream
 * (nullable) receives the caret affinity. */
size_t valo_paragraph_byte_offset_at(const ValoParagraph *paragraph, float x,
                                     float y, bool *out_downstream);
/* Writes up to `capacity` rects, returns the TOTAL count (size with
 * capacity 0, then call again). */
size_t valo_paragraph_rects_for_range(const ValoParagraph *paragraph,
                                      size_t start, size_t end,
                                      ValoRect *out_rects, size_t capacity);
ValoTextRange valo_paragraph_word_boundary(const ValoParagraph *paragraph,
                                           size_t byte_offset);
/* Metrics of one line; false past the last. */
bool valo_paragraph_line_metrics(const ValoParagraph *paragraph, size_t index,
                                 ValoLineMetrics *out_metrics);

/* The paragraph's unanswered font demand — families the collection has no
 * face for (NEWLINE-joined UTF-8) and codepoints nothing present covers
 * (UTF-32). Two-call like the rects: null/0 sizes, returns the total. */
size_t valo_paragraph_demand_families(const ValoParagraph *paragraph,
                                      uint8_t *out_utf8, size_t capacity);
size_t valo_paragraph_demand_codepoints(const ValoParagraph *paragraph,
                                        uint32_t *out_codepoints,
                                        size_t capacity);

/* ── system fonts: answering demands from the OS ────────────────────── */

/* Scan the platform's font directories — the expensive step, so keep the
 * handle (lazily on the first demand is the intended pattern). */
ValoSystemFonts *valo_system_fonts_new(void);
void valo_system_fonts_dispose(ValoSystemFonts *system_fonts);
/* Installed faces the scan found (0 = nothing to answer with). */
size_t valo_system_fonts_face_count(const ValoSystemFonts *system_fonts);

/* Register every installed face of the named family (all weights and
 * styles — nearest-variant matching picks per span). Returns the number
 * of faces added; 0 when the family is not installed. */
int32_t valo_fonts_add_system_family(ValoFontCollection *fonts,
                                     ValoSystemFonts *system_fonts,
                                     const uint8_t *name_utf8,
                                     size_t name_length);

/* Answer a paragraph's font demand from the installed fonts: missing
 * families register under their own names, still-uncovered codepoints
 * extend the fallback chain. True when the collection grew — rebuild the
 * affected paragraphs to pick it up. Hosts that install a source on the
 * collection need none of this: resolution happens during the build. */
bool valo_fonts_satisfy_demand(ValoFontCollection *fonts,
                               ValoSystemFonts *system_fonts);

#ifdef __cplusplus
}
#endif

#endif /* VALO_H */
