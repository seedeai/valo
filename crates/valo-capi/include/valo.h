/* valo.h is the C API for embedding the valo renderer.
 *
 * Conventions (uniform across the whole surface):
 * - Every object is an OPAQUE HANDLE from a `valo_*_new`-style call,
 *   released by the matching `valo_*_dispose`. Handles are not thread-safe.
 * - Every function is NULL-SAFE: a null handle is a no-op / returns zero.
 * - Geometry and paint travel BY VALUE; there is no retained paint object.
 * - Strings are UTF-8 (pointer, byte length), never NUL-terminated.
 * - Angles are radians; colors are straight-alpha floats in [0, 1];
 *   text offsets are UTF-8 BYTE offsets.
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

/* ValoContext is the GPU renderer handle. Create with valo_context_new
 * (null when no adapter exists); dispose with valo_context_dispose. */
typedef struct ValoContext ValoContext;
/* ValoDisplayListBuilder records drawing commands. Finish with
 * valo_builder_build (consumes the handle) or abandon with
 * valo_builder_dispose. */
typedef struct ValoDisplayListBuilder ValoDisplayListBuilder;
/* ValoDisplayList is an immutable recording of drawing commands. */
typedef struct ValoDisplayList ValoDisplayList;
/* ValoPath is a mutable path. Reset in place with valo_path_reset. */
typedef struct ValoPath ValoPath;
/* ValoImage is a drawable GPU image. Display lists retain it independently
 * of this handle. */
typedef struct ValoImage ValoImage;
/* ValoFontCollection holds registered faces used to shape paragraphs. */
typedef struct ValoFontCollection ValoFontCollection;
/* ValoParagraphBuilder accumulates styled spans until build. */
typedef struct ValoParagraphBuilder ValoParagraphBuilder;
/* ValoParagraph is shaped text. Call valo_paragraph_layout before drawing
 * or reading metrics. */
typedef struct ValoParagraph ValoParagraph;
/* ValoSystemFonts is a scan of the platform's installed fonts. Keep the
 * handle; creating it lazily on the first demand is the intended pattern. */
typedef struct ValoSystemFonts ValoSystemFonts;
/* ValoColorFilter is an immutable colour transform borrowed by ValoPaint.
 * The handle only has to outlive the draw or save_layer call that copies it. */
typedef struct ValoColorFilter ValoColorFilter;

/* ── by-value types ─────────────────────────────────────────────────── */

/* ValoColor is a straight-alpha sRGB color. Components conventionally
 * range from 0 to 1 and are not clamped here. */
typedef struct ValoColor {
  float red, green, blue, alpha;
} ValoColor;

/* ValoRect is an axis-aligned rectangle in Valo's y-down coordinates.
 * Origin is the top-left; width and height are extents in logical pixels. */
typedef struct ValoRect {
  float x, y, width, height;
} ValoRect;

/* ValoPoint is a 2D position or vector in logical pixels. Down is positive. */
typedef struct ValoPoint {
  float x, y;
} ValoPoint;

/* ValoTransform is a row-major 2×3 affine transform (the CSS/Skia 6-tuple). */
typedef struct ValoTransform {
  float a, b, c, d, translate_x, translate_y;
} ValoTransform;

/* ValoCornerRadii holds per-corner elliptical radii, clockwise from
 * top-left. Circular corners are x == y. Radii that would overlap are
 * reduced proportionally when the rounded rect is built. */
typedef struct ValoCornerRadii {
  float top_left_x, top_left_y;
  float top_right_x, top_right_y;
  float bottom_right_x, bottom_right_y;
  float bottom_left_x, bottom_left_y;
} ValoCornerRadii;

/* ValoPaint describes one draw's paint. There is no shader field — C paints
 * are solid color, stroke, blend, mask blur, and an optional borrowed color
 * filter. Unknown integer enums take the valo default (fill, srcOver, butt,
 * miter, normal blur) rather than trapping.
 *
 * blend_mode — the 29 Skia modes, in this order:
 *  0 clear · 1 src · 2 dst · 3 srcOver · 4 dstOver · 5 srcIn · 6 dstIn ·
 *  7 srcOut · 8 dstOut · 9 srcAtop · 10 dstAtop · 11 xor · 12 plus ·
 *  13 modulate · 14 screen · 15 overlay · 16 darken · 17 lighten ·
 *  18 colorDodge · 19 colorBurn · 20 hardLight · 21 softLight ·
 *  22 difference · 23 exclusion · 24 multiply · 25 hue · 26 saturation ·
 *  27 color · 28 luminosity
 * style: 0 fill · 1 stroke. stroke_width 0 is a hairline (one device pixel);
 * only a negative width draws nothing. stroke_cap: 0 butt · 1 round ·
 * 2 square. stroke_join: 0 miter · 1 round · 2 bevel. stroke_miter_limit
 * <= 0 becomes 4. mask_blur_sigma <= 0 = none; mask_blur_style: 0 normal ·
 * 1 solid · 2 inner · 3 outer. color_filter is borrowed or NULL and only
 * has to outlive the call; it recolours what this paint drew, before
 * mask_blur spreads it. Shader and image draws use only color.alpha. */
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
  const ValoColorFilter *color_filter;
} ValoPaint;

/* ValoTextStyle is one span's style. families_utf8 is a newline-separated
 * family list, tried in order per glyph (newlines cannot occur in family
 * names). Null/empty uses the collection's fallbacks only. Pointers are
 * borrowed for the duration of the call that consumes this struct.
 * weight is a CSS weight, conventionally 100–900 and clamped to 1..=1000.
 * line_height <= 0 uses the font's own metrics. decoration_kind:
 * -1 none · 0 underline · 1 lineThrough · 2 overline; unknown values mean
 * no decoration. decoration_color.alpha <= 0 inherits the text color.
 * decoration_thickness <= 0 uses 1. */
typedef struct ValoTextStyle {
  const uint8_t *families_utf8;
  size_t families_length;
  float size;
  uint32_t weight;
  bool italic;
  ValoColor color;
  float letter_spacing;
  float word_spacing;
  float line_height;
  int32_t decoration_kind;
  ValoColor decoration_color;
  float decoration_thickness;
} ValoTextStyle;

/* ValoParagraphStyle controls layout for a complete paragraph.
 * align: 0 left · 1 center · 2 right · 3 justify; unknown values become
 * left. max_lines 0 = unlimited. A null/empty ellipsis truncates without
 * one. Base writing direction is inferred from content. */
typedef struct ValoParagraphStyle {
  int32_t align;
  uint32_t max_lines;
  const uint8_t *ellipsis_utf8;
  size_t ellipsis_length;
} ValoParagraphStyle;

/* ValoTextRange is a UTF-8 byte range [start, end) in paragraph text. */
typedef struct ValoTextRange {
  size_t start, end;
} ValoTextRange;

/* ValoLineMetrics measures one laid-out line in paragraph-local logical
 * pixels. start/end are UTF-8 byte offsets; baseline and left are
 * paragraph-local; ascent/descent/width are in logical pixels. */
typedef struct ValoLineMetrics {
  size_t start, end;
  float baseline, ascent, descent, left, width;
} ValoLineMetrics;

/* ── context: GPU bring-up, surfaces, images ────────────────────────── */

/* valo_context_new brings up the GPU and a valo context with no window
 * attached. Bring-up is blocking (instance → adapter → device) and happens
 * once. Returns null when no adapter exists. */
ValoContext *valo_context_new(void);
/* valo_context_dispose releases a context handle. Null is a no-op. */
void valo_context_dispose(ValoContext *context);

/* valo_context_attach_metal_layer attaches a presentable surface over a
 * raw CAMetalLayer* (macOS/iOS). Returns false when surface creation
 * fails; a previous surface is replaced. The layer must outlive the
 * surface. */
bool valo_context_attach_metal_layer(ValoContext *context, void *metal_layer,
                                     uint32_t width, uint32_t height);
/* valo_context_resize resizes the attached surface (no-op without one). */
void valo_context_resize(ValoContext *context, uint32_t width,
                         uint32_t height);

/* valo_context_metal_device returns the Metal device the context renders
 * with (macOS). Hand it to a CAMetalLayer so externally-owned swapchain
 * textures live on the same GPU device. Borrowed: valid while the context
 * lives, not retained. Null context returns null. */
void *valo_context_metal_device(ValoContext *context);

/* valo_context_render_to_metal_texture draws one frame into a caller-owned
 * MTLTexture* (macOS). This is the external-swapchain route: the embedder
 * drives the drawable cycle, valo only draws. format: 0 bgra8unorm ·
 * 1 rgba8unorm, matching the texture. The texture must allow copies (set
 * the layer's framebufferOnly to false) — dst-reading blends snapshot the
 * target. Returns after SUBMISSION: presenting a drawable right after is
 * safe (the display waits for the drawable's GPU writes on its own); call
 * valo_context_wait_for_gpu before reading the texture from the CPU. */
bool valo_context_render_to_metal_texture(ValoContext *context,
                                          const ValoDisplayList *list,
                                          ValoColor clear, void *texture,
                                          uint32_t width, uint32_t height,
                                          int32_t format);

/* valo_context_wait_for_gpu blocks until every submitted frame has
 * finished on the GPU. Needed only before CPU reads of a rendered texture
 * (tests, exports); frame loops must NOT call this. Null is a no-op. */
void valo_context_wait_for_gpu(ValoContext *context);

/* valo_context_import_metal_texture wraps a caller-owned MTLTexture* as a
 * drawable image, zero-copy (macOS). The texture must have shader-read
 * usage and stay alive while the image is drawn. format: 0 bgra8unorm ·
 * 1 rgba8unorm. Returns null on a null handle, null texture, or zero size. */
ValoImage *valo_context_import_metal_texture(ValoContext *context,
                                             void *texture, uint32_t width,
                                             uint32_t height, int32_t format);

/* valo_context_render draws one frame onto the attached surface and
 * presents it. Returns false without a surface or when the swapchain
 * skipped the frame (occluded window) — both are recoverable. */
bool valo_context_render(ValoContext *context, const ValoDisplayList *list,
                         ValoColor clear);

/* valo_context_render_to_pixels renders headless into caller-allocated
 * straight-alpha RGBA8 pixels (width * height * 4 bytes). This is the
 * export and golden-test route. Returns false on a null handle, null
 * buffer, or zero size. */
bool valo_context_render_to_pixels(ValoContext *context,
                                   const ValoDisplayList *list,
                                   ValoColor clear, uint32_t width,
                                   uint32_t height, uint8_t *out_pixels);

/* valo_context_create_image uploads straight-alpha RGBA8 pixels as a
 * drawable image (mipmapped). The pixel buffer is copied; it only has to
 * outlive this call. Returns null on a null handle, null pixels, or zero
 * size. */
ValoImage *valo_context_create_image(ValoContext *context, uint32_t width,
                                     uint32_t height, const uint8_t *pixels);
/* valo_image_dispose releases an image handle. Null is a no-op. */
void valo_image_dispose(ValoImage *image);

/* ── display-list recording ─────────────────────────────────────────── */

/* valo_builder_new creates an empty display-list recorder. Recording is
 * GPU-free. */
ValoDisplayListBuilder *valo_builder_new(void);
/* valo_builder_build finishes recording: consumes the builder handle and
 * returns the display list. Null builder returns null. */
ValoDisplayList *valo_builder_build(ValoDisplayListBuilder *builder);
/* valo_builder_dispose releases a builder that was not built. */
void valo_builder_dispose(ValoDisplayListBuilder *builder);
/* valo_display_list_dispose releases a finished display list. */
void valo_display_list_dispose(ValoDisplayList *list);

/* valo_builder_save preserves the current transform and clip until the
 * matching restore. */
void valo_builder_save(ValoDisplayListBuilder *builder);
/* valo_builder_save_layer begins a composited layer. The paint's alpha,
 * blend mode, and mask blur apply to the layer as a whole on restore. */
void valo_builder_save_layer(ValoDisplayListBuilder *builder, ValoPaint paint);
/* valo_builder_restore closes the most recent save or layer scope. An
 * unmatched restore triggers a debug assertion and is ignored in release. */
void valo_builder_restore(ValoDisplayListBuilder *builder);
/* valo_builder_translate offsets subsequent drawing and clipping, in
 * logical pixels. */
void valo_builder_translate(ValoDisplayListBuilder *builder, float x, float y);
/* valo_builder_scale scales subsequent drawing and clipping. */
void valo_builder_scale(ValoDisplayListBuilder *builder, float x, float y);
/* valo_builder_rotate rotates subsequent drawing and clipping clockwise,
 * in radians. */
void valo_builder_rotate(ValoDisplayListBuilder *builder, float radians);
/* valo_builder_transform concatenates a 2×3 affine transform. */
void valo_builder_transform(ValoDisplayListBuilder *builder,
                            ValoTransform transform);
/* valo_builder_transform_matrix concatenates a full column-major 4×4.
 * The 16 floats are the Flutter canvas order — perspective rows included;
 * the z output is ignored for painting. */
void valo_builder_transform_matrix(ValoDisplayListBuilder *builder,
                                   const float *matrix);

/* valo_builder_clip_rect applies a rectangular clip until the current
 * scope ends. operation: 0 intersect · 1 difference. */
void valo_builder_clip_rect(ValoDisplayListBuilder *builder, ValoRect rect,
                            int32_t operation);
/* valo_builder_clip_rounded_rect applies a rounded-rectangle clip until
 * the current scope ends. operation: 0 intersect · 1 difference. */
void valo_builder_clip_rounded_rect(ValoDisplayListBuilder *builder,
                                    ValoRect rect, ValoCornerRadii radii,
                                    int32_t operation);
/* valo_builder_clip_path applies a path clip until the current scope
 * ends. rule: 0 nonZero · 1 evenOdd. operation: 0 intersect · 1 difference. */
void valo_builder_clip_path(ValoDisplayListBuilder *builder, ValoPath *path,
                            int32_t rule, int32_t operation);

/* valo_builder_draw_rect fills or strokes an axis-aligned rectangle. */
void valo_builder_draw_rect(ValoDisplayListBuilder *builder, ValoRect rect,
                            ValoPaint paint);
/* valo_builder_draw_rounded_rect fills or strokes a rounded rectangle. */
void valo_builder_draw_rounded_rect(ValoDisplayListBuilder *builder,
                                    ValoRect rect, ValoCornerRadii radii,
                                    ValoPaint paint);
/* valo_builder_draw_circle fills or strokes a circle in logical pixels. */
void valo_builder_draw_circle(ValoDisplayListBuilder *builder, float center_x,
                              float center_y, float radius, ValoPaint paint);
/* valo_builder_draw_path fills or strokes a path. rule: 0 nonZero ·
 * 1 evenOdd. */
void valo_builder_draw_path(ValoDisplayListBuilder *builder, ValoPath *path,
                            int32_t rule, ValoPaint paint);
/* Draws source (texel rect) of the image into destination. The paint's
 * alpha, blend mode, and mask blur apply; its RGB channels are ignored.
 * sampling: 0 linear (mipmapped) · 1 nearest. */
void valo_builder_draw_image_rect(ValoDisplayListBuilder *builder,
                                  const ValoImage *image, ValoRect source,
                                  ValoRect destination, int32_t sampling,
                                  ValoPaint paint);
/* valo_builder_draw_display_list replays a finished display list here
 * (nesting). */
void valo_builder_draw_display_list(ValoDisplayListBuilder *builder,
                                    const ValoDisplayList *list);
/* valo_builder_draw_paragraph draws a laid-out paragraph with its styles'
 * own colors. Top-left is (x, y). Call valo_paragraph_layout first. */
void valo_builder_draw_paragraph(ValoDisplayListBuilder *builder,
                                 const ValoParagraph *paragraph, float x,
                                 float y);
/* valo_builder_draw_paragraph_with draws a laid-out paragraph with paint
 * overriding every run's own fill — stroked or blended text. Colour,
 * style, stroke, blend mode, mask blur and colour filter all apply;
 * shadows and decorations still come from the paragraph's styles.
 * ValoPaint carries no shader, so gradient-filled text is not reachable
 * from C. A stroke width of 0 is a hairline (one device pixel); only a
 * negative width draws nothing. */
void valo_builder_draw_paragraph_with(ValoDisplayListBuilder *builder,
                                      const ValoParagraph *paragraph, float x,
                                      float y, ValoPaint paint);

/* ── colour filters ─────────────────────────────────────────────────── */

/* valo_color_filter_matrix builds a 4×5 colour matrix from 20 row-major
 * floats over unpremultiplied colour in 0..1: each output channel is
 * clamp(row · [r, g, b, a, 1]). Flutter's ColorFilter.matrix gives the
 * translation column (entries 4, 9, 14, 19) in unnormalized 0..255 space
 * — divide those four by 255 first, or every offset lands 255× too
 * strong. Null matrix returns null. Non-finite entries become 0. */
ValoColorFilter *valo_color_filter_matrix(const float *matrix);
/* valo_color_filter_blend blends color as the source over what was drawn
 * (Flutter's ColorFilter.mode). mode indexes the same 29 modes as
 * ValoPaint. */
ValoColorFilter *valo_color_filter_blend(ValoColor color, int32_t mode);
void valo_color_filter_dispose(ValoColorFilter *filter);

/* ── paths ──────────────────────────────────────────────────────────── */

ValoPath *valo_path_new(void);
void valo_path_dispose(ValoPath *path);
/* valo_path_move_to starts a new contour at (x, y). */
void valo_path_move_to(ValoPath *path, float x, float y);
/* valo_path_line_to adds a straight segment to (x, y). */
void valo_path_line_to(ValoPath *path, float x, float y);
/* valo_path_quadratic_to adds a quadratic Bézier through a control point
 * to (x, y). */
void valo_path_quadratic_to(ValoPath *path, float control_x, float control_y,
                            float x, float y);
/* valo_path_cubic_to adds a cubic Bézier through two control points to
 * (x, y). */
void valo_path_cubic_to(ValoPath *path, float control1_x, float control1_y,
                        float control2_x, float control2_y, float x, float y);
/* valo_path_close adds a segment back to the current contour's start. It
 * has no effect when no contour is open. */
void valo_path_close(ValoPath *path);
void valo_path_add_rect(ValoPath *path, ValoRect rect);
void valo_path_add_rounded_rect(ValoPath *path, ValoRect rect,
                                ValoCornerRadii radii);
void valo_path_add_circle(ValoPath *path, ValoPoint center, float radius);
/* valo_path_add_arc adds a circular arc. Angles are radians from the +x
 * axis; a positive sweep turns toward +y, which is clockwise on screen.
 * An open contour is joined to the arc's first point by a line, a closed
 * one starts there. */
void valo_path_add_arc(ValoPath *path, ValoPoint center, float radius,
                       float start_angle, float sweep_angle);
/* valo_path_add_ellipse adds an elliptical arc. x_axis_rotation and the
 * start/sweep angles are radians; a positive sweep is clockwise on
 * screen. An open contour is joined to the arc's first point by a line. */
void valo_path_add_ellipse(ValoPath *path, ValoPoint center, float radius_x,
                           float radius_y, float x_axis_rotation,
                           float start_angle, float sweep_angle);
/* valo_path_arc_to rounds the corner between the current point, corner,
 * and next. The circle of radius is tangent to both the segment from the
 * current point to corner and the one from corner to next, reached by a
 * line. Degenerate input falls back to a line to corner. */
void valo_path_arc_to(ValoPath *path, ValoPoint corner, ValoPoint next,
                      float radius);
/* valo_path_reset clears the path so it can be rebuilt. */
void valo_path_reset(ValoPath *path);
/* valo_path_contains reports whether point lies inside the filled path.
 * rule: 0 non-zero · 1 even-odd. A null path contains nothing. */
bool valo_path_contains(ValoPath *path, ValoPoint point, int32_t rule);

/* ── text ───────────────────────────────────────────────────────────── */

ValoFontCollection *valo_fonts_new(void);
void valo_fonts_dispose(ValoFontCollection *fonts);
/* valo_fonts_add registers a face from TTF/OTF bytes. Returns the face
 * id, or -1 when the bytes don't parse. The buffer is copied. Rebuild
 * paragraphs to see newly added faces. */
int64_t valo_fonts_add(ValoFontCollection *fonts, const uint8_t *bytes,
                       size_t length);
/* valo_fonts_add_fallback appends a registered face to the fallback
 * chain. Codepoints no styled family covers try the chain in order. A
 * negative id is a no-op. */
void valo_fonts_add_fallback(ValoFontCollection *fonts, int64_t face_id);

/* valo_fonts_add_instances registers every face a font file offers: a
 * static font is one face; a variable font is its named instances, so
 * weights and styles select like a static multi-weight family.
 * add_as_fallbacks also appends each face to the fallback chain (default
 * UI fonts). Returns faces added; 0 when the bytes don't parse. */
int32_t valo_fonts_add_instances(ValoFontCollection *fonts,
                                 const uint8_t *bytes, size_t length,
                                 bool add_as_fallbacks);

/* valo_fonts_family_name writes the family name of a registered face:
 * writes up to capacity UTF-8 bytes, returns the TOTAL length (size with
 * capacity 0); 0 if unknown. */
size_t valo_fonts_family_name(const ValoFontCollection *fonts,
                              int64_t face_id, uint8_t *out_utf8,
                              size_t capacity);

/* valo_paragraph_builder_new creates a paragraph builder with style.
 * fonts must be non-null (returns null otherwise) but the collection is
 * used at build, not construction. Embedders with style stacks resolve
 * them before adding spans. */
ValoParagraphBuilder *valo_paragraph_builder_new(
    const ValoFontCollection *fonts, ValoParagraphStyle style);
void valo_paragraph_builder_dispose(ValoParagraphBuilder *builder);
/* valo_paragraph_builder_add_text appends one styled span of UTF-8 text.
 * Style family pointers only have to outlive this call. Null builder,
 * null text, or invalid UTF-8 is a no-op. */
void valo_paragraph_builder_add_text(ValoParagraphBuilder *builder,
                                     const uint8_t *text_utf8,
                                     size_t text_length, ValoTextStyle style);
/* valo_paragraph_builder_build consumes the builder and returns a shaped
 * paragraph. The collection resolves any missing families/codepoints from
 * its own sources during this call, and may grow. Call
 * valo_paragraph_layout before drawing or querying. Null builder or fonts
 * returns null. */
ValoParagraph *valo_paragraph_builder_build(ValoParagraphBuilder *builder,
                                            ValoFontCollection *fonts);
void valo_paragraph_dispose(ValoParagraph *paragraph);

/* valo_paragraph_layout wraps the paragraph to max_width. Pass INFINITY
 * for unconstrained width. Repeating the same width reuses the existing
 * layout. */
void valo_paragraph_layout(ValoParagraph *paragraph, float max_width);
/* Metrics below are zero / false before valo_paragraph_layout. */
float valo_paragraph_width(const ValoParagraph *paragraph);
float valo_paragraph_height(const ValoParagraph *paragraph);
float valo_paragraph_longest_line(const ValoParagraph *paragraph);
float valo_paragraph_min_intrinsic_width(const ValoParagraph *paragraph);
float valo_paragraph_max_intrinsic_width(const ValoParagraph *paragraph);
size_t valo_paragraph_line_count(const ValoParagraph *paragraph);
bool valo_paragraph_did_exceed_max_lines(const ValoParagraph *paragraph);

/* valo_paragraph_caret_for_offset returns a zero-width caret rectangle
 * for a UTF-8 byte offset, in paragraph-local logical pixels. Returns a
 * zero rect when there are no lines (including before layout). */
ValoRect valo_paragraph_caret_for_offset(const ValoParagraph *paragraph,
                                         size_t byte_offset);
/* valo_paragraph_byte_offset_at returns the byte offset nearest a
 * paragraph-local point. out_downstream (nullable) receives the caret
 * affinity. An empty paragraph returns offset 0. */
size_t valo_paragraph_byte_offset_at(const ValoParagraph *paragraph, float x,
                                     float y, bool *out_downstream);
/* valo_paragraph_rects_for_range writes selection rectangles for a UTF-8
 * byte range. Writes up to capacity rects, returns the TOTAL count (size
 * with capacity 0, then call again). */
size_t valo_paragraph_rects_for_range(const ValoParagraph *paragraph,
                                      size_t start, size_t end,
                                      ValoRect *out_rects, size_t capacity);
/* valo_paragraph_word_boundary returns the word range around a UTF-8
 * byte offset (double-click selection). */
ValoTextRange valo_paragraph_word_boundary(const ValoParagraph *paragraph,
                                           size_t byte_offset);
/* valo_paragraph_line_metrics writes metrics of line index. False past
 * the last line, before layout, or when a pointer is null. */
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

/* valo_system_fonts_new scans the platform's font directories — the
 * expensive step, so keep the handle (lazily on the first demand is the
 * intended pattern). */
ValoSystemFonts *valo_system_fonts_new(void);
void valo_system_fonts_dispose(ValoSystemFonts *system_fonts);
/* valo_system_fonts_face_count returns how many installed faces the scan
 * found (0 = nothing to answer with). */
size_t valo_system_fonts_face_count(const ValoSystemFonts *system_fonts);

/* valo_fonts_add_system_family registers every installed face of the
 * named family (all weights and styles — nearest-variant matching picks
 * per span). Returns the number of faces added; 0 when the family is not
 * installed. */
int32_t valo_fonts_add_system_family(ValoFontCollection *fonts,
                                     ValoSystemFonts *system_fonts,
                                     const uint8_t *name_utf8,
                                     size_t name_length);

/* valo_fonts_satisfy_demand answers a paragraph's font demand from the
 * installed fonts: missing families register under their own names,
 * still-uncovered codepoints extend the fallback chain. True when the
 * collection grew — rebuild the affected paragraphs to pick it up. Hosts
 * that install a source on the collection need none of this: resolution
 * happens during valo_paragraph_builder_build. */
bool valo_fonts_satisfy_demand(ValoFontCollection *fonts,
                               ValoSystemFonts *system_fonts);

#ifdef __cplusplus
}
#endif

#endif /* VALO_H */
