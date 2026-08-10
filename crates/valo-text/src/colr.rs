//! COLR rasterization — the painting half. skrifa owns the
//! hard half (parsing + paint-graph traversal, emitting [`ColorPainter`]
//! callbacks in FONT UNITS); this module rasterizes those callbacks with
//! tiny-skia into the same premultiplied-RGBA [`GlyphImage`] the CBDT path
//! produces — the atlas and the color text pipeline never know the source.
//! References: Skia's COLRv1 implementation and typf-render-color
//! (Apache-2.0); we differ by rendering sweep gradients for real instead
//! of a solid-color fallback.

use skrifa::color::{Brush, ColorGlyph, ColorPainter, ColorStop, CompositeMode};
use skrifa::outline::{DrawSettings, OutlineGlyphCollection, OutlinePen};
use skrifa::prelude::{LocationRef, Size};
use skrifa::raw::types::BoundingBox;
use skrifa::raw::TableProvider;
use skrifa::{GlyphId, MetadataProvider};
use tiny_skia as ts;

use crate::font::Font;
use crate::raster::GlyphImage;

/// Rasterize `glyph` at `px` if the font carries a COLR form for it.
/// (The caller tries swash's CBDT/COLRv0 fast paths first.)
pub(crate) fn raster(font: &Font, glyph: u32, px: f32) -> Option<GlyphImage> {
    let font_ref = skrifa::FontRef::from_index(font.data(), font.face_index()).ok()?;
    let color = font_ref.color_glyphs().get(GlyphId::new(glyph))?;
    let bounds = pixel_bounds(font, &color, px)?;
    let location = LocationRef::from(font.variation_location());
    let mut painter = Painter::new(&font_ref, location, &bounds)?;
    color.paint(location, &mut painter).ok()?;
    Some(painter.into_image(&bounds))
}

/// The glyph's pixel box, y-UP around the origin (swash placement space):
/// the COLRv1 clip box when the font declares one, else the font-wide ink
/// box — always padded a pixel so antialiased edges survive the crop.
struct PixelBounds {
    x_min: f32,
    y_max: f32,
    width: u32,
    height: u32,
    /// font units → pixmap pixels (scale + y-flip + origin shift).
    to_pixmap: ts::Transform,
}

fn pixel_bounds(font: &Font, color: &ColorGlyph, px: f32) -> Option<PixelBounds> {
    let unpadded = color
        .bounding_box(LocationRef::from(font.variation_location()), Size::new(px))
        .map(|b| (b.x_min, b.y_min, b.x_max, b.y_max))
        .or_else(|| font.ink_box_px(px))?;
    let (x_min, y_min, x_max, y_max) = unpadded;
    let (x_min, y_min) = (x_min.floor() - 1.0, y_min.floor() - 1.0);
    let (x_max, y_max) = (x_max.ceil() + 1.0, y_max.ceil() + 1.0);
    let (width, height) = ((x_max - x_min) as u32, (y_max - y_min) as u32);
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return None;
    }
    let scale = px / font.units_per_em();
    let _ = y_min;
    Some(PixelBounds {
        x_min,
        y_max,
        width,
        height,
        to_pixmap: ts::Transform::from_row(scale, 0.0, 0.0, -scale, -x_min, y_max),
    })
}

/// One compositing layer: its pixels and how it merges down on pop.
struct Layer {
    pixmap: ts::Pixmap,
    mode: CompositeMode,
}

/// A [`ColorPainter`] rasterizing onto a tiny-skia pixmap stack: transforms
/// concatenate, clips intersect as masks, layers composite on pop, brushes
/// become shaders (sweep gradients rasterize manually — tiny-skia has none).
struct Painter<'a> {
    outlines: OutlineGlyphCollection<'a>,
    /// The instance's axis position — layer outlines draw at it.
    location: LocationRef<'a>,
    /// CPAL palette 0, resolved to straight-alpha colors once.
    palette: Vec<ts::Color>,
    layers: Vec<Layer>,
    /// Current = last; base entry maps font units into the pixmap.
    transforms: Vec<ts::Transform>,
    /// Current = last; each entry is already intersected with its parents.
    clips: Vec<ts::Mask>,
    width: u32,
    height: u32,
}

impl<'a> Painter<'a> {
    fn new(
        font_ref: &skrifa::FontRef<'a>,
        location: LocationRef<'a>,
        bounds: &PixelBounds,
    ) -> Option<Self> {
        Some(Painter {
            outlines: font_ref.outline_glyphs(),
            location,
            palette: read_palette(font_ref),
            layers: vec![Layer {
                pixmap: ts::Pixmap::new(bounds.width, bounds.height)?,
                mode: CompositeMode::SrcOver,
            }],
            transforms: vec![bounds.to_pixmap],
            clips: Vec::new(),
            width: bounds.width,
            height: bounds.height,
        })
    }

    fn into_image(mut self, bounds: &PixelBounds) -> GlyphImage {
        GlyphImage {
            width: self.width,
            height: self.height,
            left: bounds.x_min as i32,
            top: bounds.y_max as i32,
            data: self.layers.swap_remove(0).pixmap.take(),
        }
    }

    fn transform(&self) -> ts::Transform {
        *self
            .transforms
            .last()
            .expect("base transform always present")
    }

    /// Push the current clip ∩ `path` (path in font units, like every
    /// callback — the current transform carries it into pixmap space).
    fn push_clip_path(&mut self, path: &ts::Path) {
        let mut mask = match self.clips.last() {
            Some(top) => top.clone(),
            None => full_mask(self.width, self.height),
        };
        mask.intersect_path(path, ts::FillRule::Winding, true, self.transform());
        self.clips.push(mask);
    }

    /// The glyph's outline as a FONT-UNIT path (unscaled — transforms are
    /// applied where the path is consumed).
    fn glyph_path(&self, glyph_id: GlyphId) -> Option<ts::Path> {
        let outline = self.outlines.get(glyph_id)?;
        let mut pen = TsPathPen {
            builder: ts::PathBuilder::new(),
        };
        outline
            .draw(
                DrawSettings::unhinted(Size::unscaled(), self.location),
                &mut pen,
            )
            .ok()?;
        pen.builder.finish()
    }

    /// Resolve a CPAL reference to a straight-alpha color. `0xFFFF` is the
    /// spec's "foreground" sentinel — a fixed color glyph has no text color
    /// to inherit, so it resolves to opaque black (documented divergence:
    /// Skia takes the paint color).
    fn palette_color(&self, index: u16, alpha: f32) -> ts::Color {
        let mut color = if index == 0xFFFF {
            ts::Color::BLACK
        } else {
            self.palette
                .get(index as usize)
                .copied()
                .unwrap_or(ts::Color::BLACK)
        };
        color.apply_opacity(alpha.clamp(0.0, 1.0));
        color
    }

    /// Stops resolved to (offset, straight color) pairs — kept in a plain
    /// representation because tiny-skia's `GradientStop` is write-only.
    fn resolved_stops(&self, stops: &[ColorStop]) -> Vec<(f32, ts::Color)> {
        stops
            .iter()
            .map(|s| {
                (
                    s.offset.clamp(0.0, 1.0),
                    self.palette_color(s.palette_index, s.alpha),
                )
            })
            .collect()
    }

    /// Fill the current clip with `brush` on the top layer — the one
    /// operation every leaf of the paint graph reduces to.
    fn fill_with_brush(&mut self, brush: &Brush<'_>) {
        let paint = match self.brush_paint(brush) {
            Some(paint) => paint,
            None => return self.fill_sweep_fallback(brush),
        };
        let rect = match ts::Rect::from_xywh(0.0, 0.0, self.width as f32, self.height as f32) {
            Some(rect) => rect,
            None => return,
        };
        let (clip, layer) = (self.clips.last(), self.layers.last_mut());
        if let Some(layer) = layer {
            layer
                .pixmap
                .fill_rect(rect, &paint, ts::Transform::identity(), clip);
        }
    }

    /// Solid and linear/radial gradients map onto tiny-skia shaders (the
    /// shader's transform carries font units → pixels). Sweep returns
    /// `None` — no such shader exists; it rasterizes manually.
    fn brush_paint(&self, brush: &Brush<'_>) -> Option<ts::Paint<'static>> {
        let mut paint = ts::Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.shader = match *brush {
            Brush::Solid {
                palette_index,
                alpha,
            } => ts::Shader::SolidColor(self.palette_color(palette_index, alpha)),
            Brush::LinearGradient {
                p0,
                p1,
                color_stops,
                extend,
            } => ts::LinearGradient::new(
                ts::Point::from_xy(p0.x, p0.y),
                ts::Point::from_xy(p1.x, p1.y),
                shader_stops(&self.resolved_stops(color_stops)),
                spread_mode(extend),
                self.transform(),
            )
            .unwrap_or(ts::Shader::SolidColor(last_stop_color(self, color_stops)?)),
            Brush::RadialGradient {
                c0,
                r0,
                c1,
                r1,
                color_stops,
                extend,
            } => radial_shader(self, c0, r0, c1, r1, color_stops, extend)?,
            Brush::SweepGradient { .. } => return None,
        };
        Some(paint)
    }

    /// Sweep gradients, rasterized per pixel: inverse-map the pixel into
    /// font units, take the clockwise angle around the center, normalize
    /// into the stop line, sample. Bounded by the glyph box, so the naive
    /// loop is fine (COLR sweeps are rare and small).
    fn fill_sweep_fallback(&mut self, brush: &Brush<'_>) {
        let Brush::SweepGradient {
            c0,
            start_angle,
            end_angle,
            color_stops,
            extend,
        } = *brush
        else {
            return;
        };
        if color_stops.is_empty() || (end_angle - start_angle).abs() < f32::EPSILON {
            return;
        }
        let Some(inverse) = self.transform().invert() else {
            return;
        };
        let stops = self.resolved_stops(color_stops);
        let clip = self.clips.last().cloned();
        let (width, height) = (self.width, self.height);
        let Some(layer) = self.layers.last_mut() else {
            return;
        };
        let pixels = layer.pixmap.pixels_mut();
        for y in 0..height {
            for x in 0..width {
                let index = (y * width + x) as usize;
                let coverage = clip.as_ref().map_or(255, |m| m.data()[index]);
                if coverage == 0 {
                    continue;
                }
                let mut point = ts::Point::from_xy(x as f32 + 0.5, y as f32 + 0.5);
                inverse.map_point(&mut point);
                // Clockwise angle in degrees, matching the callback's
                // contract; only 0..360 from the start angle is drawn.
                let angle = (point.y - c0.y).atan2(point.x - c0.x).to_degrees();
                let t = (angle - start_angle) / (end_angle - start_angle);
                let Some(t) = apply_extend(t, extend) else {
                    continue;
                };
                let color = sample_stops(&stops, t);
                blend_src_over(&mut pixels[index], color, coverage);
            }
        }
    }
}

impl ColorPainter for Painter<'_> {
    fn push_transform(&mut self, transform: skrifa::color::Transform) {
        let t = ts::Transform::from_row(
            transform.xx,
            transform.yx,
            transform.xy,
            transform.yy,
            transform.dx,
            transform.dy,
        );
        self.transforms.push(self.transform().pre_concat(t));
    }

    fn pop_transform(&mut self) {
        if self.transforms.len() > 1 {
            self.transforms.pop();
        }
    }

    fn push_clip_glyph(&mut self, glyph_id: GlyphId) {
        match self.glyph_path(glyph_id) {
            Some(path) => self.push_clip_path(&path),
            // An unresolvable clip glyph must still push: clip everything
            // (an empty mask) so the matching pop stays balanced.
            None => self
                .clips
                .push(ts::Mask::new(self.width, self.height).expect("mask dims match pixmap")),
        }
    }

    fn push_clip_box(&mut self, clip_box: BoundingBox<f32>) {
        let rect = ts::Rect::from_ltrb(
            clip_box.x_min,
            clip_box.y_min,
            clip_box.x_max,
            clip_box.y_max,
        )
        .map(ts::PathBuilder::from_rect);
        match rect {
            Some(path) => self.push_clip_path(&path),
            None => self
                .clips
                .push(ts::Mask::new(self.width, self.height).expect("mask dims match pixmap")),
        }
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn fill(&mut self, brush: Brush<'_>) {
        self.fill_with_brush(&brush);
    }

    fn push_layer(&mut self, composite_mode: CompositeMode) {
        if let Some(pixmap) = ts::Pixmap::new(self.width, self.height) {
            self.layers.push(Layer {
                pixmap,
                mode: composite_mode,
            })
        }
    }

    fn pop_layer(&mut self) {
        if self.layers.len() <= 1 {
            return;
        }
        let layer = self.layers.pop().expect("checked non-base");
        let paint = ts::PixmapPaint {
            blend_mode: blend_mode(layer.mode),
            ..Default::default()
        };
        if let Some(below) = self.layers.last_mut() {
            below.pixmap.draw_pixmap(
                0,
                0,
                layer.pixmap.as_ref(),
                &paint,
                ts::Transform::identity(),
                None,
            );
        }
    }
}

/// Two-point conical via tiny-skia, which models `r0 = 0`: when the inner
/// radius is real, the stop line is remapped onto `[r0/r1, 1]` — exact for
/// concentric circles, a close approximation otherwise (typf punts to `r1`
/// unremapped; Skia solves the full quadratic).
fn radial_shader(
    painter: &Painter,
    c0: skrifa::raw::types::Point<f32>,
    r0: f32,
    c1: skrifa::raw::types::Point<f32>,
    r1: f32,
    color_stops: &[ColorStop],
    extend: skrifa::color::Extend,
) -> Option<ts::Shader<'static>> {
    if r1 <= 0.0 {
        return Some(ts::Shader::SolidColor(last_stop_color(
            painter,
            color_stops,
        )?));
    }
    let mut stops = painter.resolved_stops(color_stops);
    if r0 > 0.0 && r0 < r1 {
        let base = r0 / r1;
        for (offset, _) in &mut stops {
            *offset = base + *offset * (1.0 - base);
        }
    }
    ts::RadialGradient::new(
        ts::Point::from_xy(c0.x, c0.y),
        ts::Point::from_xy(c1.x, c1.y),
        r1,
        shader_stops(&stops),
        spread_mode(extend),
        painter.transform(),
    )
    .or_else(|| {
        Some(ts::Shader::SolidColor(last_stop_color(
            painter,
            color_stops,
        )?))
    })
}

fn last_stop_color(painter: &Painter, stops: &[ColorStop]) -> Option<ts::Color> {
    stops
        .last()
        .map(|s| painter.palette_color(s.palette_index, s.alpha))
}

/// Normalize a sweep parameter through the gradient's extend mode; `None`
/// outside the drawn range for `Pad` beyond [0, 1] is NOT possible (pad
/// clamps), only non-finite values bail.
fn apply_extend(t: f32, extend: skrifa::color::Extend) -> Option<f32> {
    if !t.is_finite() {
        return None;
    }
    Some(match extend {
        skrifa::color::Extend::Repeat => t.rem_euclid(1.0),
        skrifa::color::Extend::Reflect => {
            let cycle = t.rem_euclid(2.0);
            if cycle > 1.0 {
                2.0 - cycle
            } else {
                cycle
            }
        }
        _ => t.clamp(0.0, 1.0),
    })
}

fn shader_stops(stops: &[(f32, ts::Color)]) -> Vec<ts::GradientStop> {
    stops
        .iter()
        .map(|&(offset, color)| ts::GradientStop::new(offset, color))
        .collect()
}

/// Piecewise-linear sample of a sorted stop line at `t` ∈ [0, 1].
fn sample_stops(stops: &[(f32, ts::Color)], t: f32) -> ts::Color {
    let Some(&(first_offset, first_color)) = stops.first() else {
        return ts::Color::TRANSPARENT;
    };
    if t <= first_offset {
        return first_color;
    }
    for pair in stops.windows(2) {
        let ((a_off, a_color), (b_off, b_color)) = (pair[0], pair[1]);
        if t <= b_off {
            let span = (b_off - a_off).max(f32::EPSILON);
            return lerp_color(a_color, b_color, (t - a_off) / span);
        }
    }
    stops
        .last()
        .map(|&(_, c)| c)
        .unwrap_or(ts::Color::TRANSPARENT)
}

fn lerp_color(a: ts::Color, b: ts::Color, k: f32) -> ts::Color {
    let mix = |x: f32, y: f32| x + (y - x) * k;
    ts::Color::from_rgba(
        mix(a.red(), b.red()),
        mix(a.green(), b.green()),
        mix(a.blue(), b.blue()),
        mix(a.alpha(), b.alpha()),
    )
    .unwrap_or(ts::Color::TRANSPARENT)
}

/// src-over one straight-alpha color onto a premultiplied pixel, scaled by
/// the clip's coverage — the sweep path's single blending need.
fn blend_src_over(dst: &mut ts::PremultipliedColorU8, color: ts::Color, coverage: u8) {
    let a = color.alpha() * (coverage as f32 / 255.0);
    let (sr, sg, sb) = (color.red() * a, color.green() * a, color.blue() * a);
    let inv = 1.0 - a;
    let to_u8 = |v: f32| (v * 255.0 + 0.5) as u8;
    *dst = ts::PremultipliedColorU8::from_rgba(
        to_u8(sr + dst.red() as f32 / 255.0 * inv),
        to_u8(sg + dst.green() as f32 / 255.0 * inv),
        to_u8(sb + dst.blue() as f32 / 255.0 * inv),
        to_u8(a + dst.alpha() as f32 / 255.0 * inv),
    )
    .unwrap_or(*dst);
}

/// CPAL palette 0 as straight-alpha colors (records are BGRA bytes).
fn read_palette(font_ref: &skrifa::FontRef) -> Vec<ts::Color> {
    let Ok(cpal) = font_ref.cpal() else {
        return Vec::new();
    };
    let Some(Ok(records)) = cpal.color_records_array() else {
        return Vec::new();
    };
    let count = cpal.num_palette_entries() as usize;
    records
        .iter()
        .take(count)
        .map(|r| ts::Color::from_rgba8(r.red, r.green, r.blue, r.alpha))
        .collect()
}

fn full_mask(width: u32, height: u32) -> ts::Mask {
    let mut mask = ts::Mask::new(width, height).expect("mask dims match pixmap");
    if let Some(rect) = ts::Rect::from_xywh(0.0, 0.0, width as f32, height as f32) {
        mask.fill_path(
            &ts::PathBuilder::from_rect(rect),
            ts::FillRule::Winding,
            false,
            ts::Transform::identity(),
        );
    }
    mask
}

fn spread_mode(extend: skrifa::color::Extend) -> ts::SpreadMode {
    match extend {
        skrifa::color::Extend::Repeat => ts::SpreadMode::Repeat,
        skrifa::color::Extend::Reflect => ts::SpreadMode::Reflect,
        _ => ts::SpreadMode::Pad,
    }
}

/// The full CompositeMode set maps 1:1 onto tiny-skia's blend modes.
fn blend_mode(mode: CompositeMode) -> ts::BlendMode {
    use ts::BlendMode as B;
    use CompositeMode as C;
    match mode {
        C::Clear => B::Clear,
        C::Src => B::Source,
        C::Dest => B::Destination,
        C::SrcOver => B::SourceOver,
        C::DestOver => B::DestinationOver,
        C::SrcIn => B::SourceIn,
        C::DestIn => B::DestinationIn,
        C::SrcOut => B::SourceOut,
        C::DestOut => B::DestinationOut,
        C::SrcAtop => B::SourceAtop,
        C::DestAtop => B::DestinationAtop,
        C::Xor => B::Xor,
        C::Plus => B::Plus,
        C::Screen => B::Screen,
        C::Overlay => B::Overlay,
        C::Darken => B::Darken,
        C::Lighten => B::Lighten,
        C::ColorDodge => B::ColorDodge,
        C::ColorBurn => B::ColorBurn,
        C::HardLight => B::HardLight,
        C::SoftLight => B::SoftLight,
        C::Difference => B::Difference,
        C::Exclusion => B::Exclusion,
        C::Multiply => B::Multiply,
        C::HslHue => B::Hue,
        C::HslSaturation => B::Saturation,
        C::HslColor => B::Color,
        C::HslLuminosity => B::Luminosity,
        _ => B::SourceOver,
    }
}

/// skrifa outline pen → tiny-skia path, in raw font units (y-up; the
/// consumer's transform does scaling and the flip).
struct TsPathPen {
    builder: ts::PathBuilder,
}

impl OutlinePen for TsPathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(x, y);
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.builder.quad_to(cx, cy, x, y);
    }

    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        self.builder.cubic_to(c0x, c0y, c1x, c1y, x, y);
    }

    fn close(&mut self) {
        self.builder.close();
    }
}
