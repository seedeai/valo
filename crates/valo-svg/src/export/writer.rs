//! The second pass: a list's ops as SVG elements, with each region the first pass found drawn as
//! its picture.

use std::collections::HashMap;
use std::fmt::Write;

use valo::{
    AlphaType, Backdrop, Bounds, Cap, ClipOp, DisplayList, DisplayListWalker, FillRule,
    GradientStop, Join, MaskKind, Matrix, MatrixClipState, Op, Paint, PaintStyle, Path,
    PathBuilder, PixelBuffer, PixelFormat, PixelLayout, Rect, RunFont, Shader, SpreadMode,
};
use valo_codec_software::Encoding;
use valo_dl::GlyphPos;
use valo_text::{FontUid, GlyphImage, Rasterizer};

use super::markup::{hex, matrix, path_data, Number, PreciseNumber};
use super::native::{
    backdrop_blur, blend_name, draw_filter, holds_a_mask, layer_filter, rasterized, Filter,
};
use super::{encoded_href, Picture, SvgOptions, SvgWriteError};

/// `EVERYWHERE` is a rectangle past any document, which a clip outside a shape cuts the shape
/// from.
const EVERYWHERE: &str = "M-1000000 -1000000H1000000V1000000H-1000000Z";

/// `document` is `list` as an SVG document of `size` units, with the first pass's `pictures` in
/// place.
pub(super) fn document(
    list: &DisplayList,
    size: [f32; 2],
    options: SvgOptions,
    pictures: &[Option<Picture>],
) -> Result<String, SvgWriteError> {
    let mut writer = Writer {
        options,
        area: Rect::new(0.0, 0.0, size[0], size[1]),
        defs: String::new(),
        writing: vec![String::new()],
        scopes: vec![Scope::default()],
        masked_layers: Vec::new(),
        glyphs: HashMap::new(),
        rasterizer: Rasterizer::new(),
        next_id: 0,
        pictures: pictures.iter(),
    };
    let mut ops = list.walk();
    while let Some(op) = ops.next() {
        writer.op(op, &mut ops)?;
    }
    Ok(writer.finish(size))
}

/// `Scope` is a scope open in the document: the `<g>` elements it opened, which its `Restore`
/// closes, whether it is a masked layer, and whether it fills a mask rather than the document.
#[derive(Default)]
struct Scope {
    groups: usize,
    masked: bool,
    fills_mask: bool,
}

/// `GlyphKey` is a glyph as drawn: the same glyph of the same face at the same size and fake
/// bold and italic.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    face: FontUid,
    glyph: u32,
    size: u32,
    embolden: u32,
    skew: u32,
}

/// `GlyphDefinition` is a glyph defined once in the document, by its id: its outline, filled
/// with the run's paint, or the image of a colour glyph, which keeps its own colours as valo's
/// renderer draws it.
#[derive(Clone)]
enum GlyphDefinition {
    Outline(String),
    Color(String),
}

/// `MaskedLayer` is a layer open whose mask a layer inside it fills: the mask's id, and the
/// transform where the layer opened, which the mask is drawn in.
#[derive(Clone)]
struct MaskedLayer {
    mask_id: String,
    transform: Matrix,
}

/// `Writer` writes a document as a walk gives a list's ops. `writing` holds what is being
/// written: the document, and over it any mask being filled.
struct Writer<'p> {
    options: SvgOptions,
    area: Rect,
    defs: String,
    writing: Vec<String>,
    scopes: Vec<Scope>,
    masked_layers: Vec<MaskedLayer>,
    glyphs: HashMap<GlyphKey, Option<GlyphDefinition>>,
    rasterizer: Rasterizer,
    next_id: u32,
    pictures: std::slice::Iter<'p, Option<Picture>>,
}

impl Writer<'_> {
    /// `op` writes `op`, or the picture of the region it opens when the document leaves it to
    /// one, skipping what it has written that way.
    fn op(&mut self, op: &Op, ops: &mut DisplayListWalker) -> Result<(), SvgWriteError> {
        if rasterized(op, ops.scope(), self.options.native) {
            self.picture(ops.matrix_clip());
            ops.skip_scope();
            return Ok(());
        }
        match op {
            Op::Save | Op::DrawDisplayList { .. } => self.scopes.push(Scope::default()),
            Op::SaveLayer {
                mask_composite: Some(kind),
                ..
            } => {
                if !self.open_mask(*kind, ops.matrix_clip()) {
                    ops.skip_scope();
                }
            }
            Op::SaveLayer {
                paint, backdrop, ..
            } => self.open_layer(op, paint, backdrop.as_ref(), ops.scope(), ops.matrix_clip()),
            Op::Restore => self.close(),
            Op::Transform(m) => {
                if let Some(transform) = matrix(m) {
                    self.group(&format!(r#"transform="{transform}""#));
                }
            }
            Op::ClipPath {
                path,
                fill_rule,
                op,
                ..
            } => self.clip(path, *fill_rule, *op),
            Op::DrawPaint { paint, .. } => {
                // The whole document, in the space it is drawn in: the clips around it cut it.
                let cover = self.local_region(ops.matrix_clip(), Bounds::Unbounded);
                self.rect(&cover, paint);
            }
            Op::DrawRect { rect, paint, .. } => self.rect(rect, paint),
            Op::DrawPath {
                path,
                fill_rule,
                paint,
                content_bounds,
                ..
            } => self.path(path, *fill_rule, paint, *content_bounds),
            Op::RRectBlur {
                rect, radii, paint, ..
            } => {
                let mut b = PathBuilder::new();
                b.rrect_radii(*rect, *radii);
                self.path(&b.build(), FillRule::NonZero, paint, *rect);
            }
            Op::GlyphRun {
                font,
                paint,
                glyphs,
                content_bounds,
                ..
            } => self.glyph_run(font, paint, glyphs, *content_bounds)?,
            Op::DrawImage { .. } | Op::DrawShadow { .. } => {
                unreachable!("images and shadows are drawn as pictures")
            }
        }
        Ok(())
    }

    /// `rect` writes `rect` filled or stroked with `paint`.
    fn rect(&mut self, rect: &Rect, paint: &Paint) {
        let attributes = self.paint_attributes(paint, FillRule::NonZero, *rect);
        self.element(&format!(
            r#"<rect x="{}" y="{}" width="{}" height="{}"{attributes}/>"#,
            Number(rect.x),
            Number(rect.y),
            Number(rect.width),
            Number(rect.height)
        ));
    }

    /// `open_layer` opens a layer as a group isolated from what is under it, with its opacity,
    /// blend mode, filter, backdrop blur and the mask a layer inside it fills.
    fn open_layer(
        &mut self,
        op: &Op,
        paint: &Paint,
        backdrop: Option<&Backdrop>,
        scope: &[Op],
        state: &MatrixClipState,
    ) {
        let mut attributes = String::new();
        if paint.color.a < 1.0 {
            let _ = write!(attributes, r#" opacity="{}""#, Number(paint.color.a));
        }
        let mut style = String::from("isolation:isolate");
        if let Some(name) = blend_name(paint.blend_mode).filter(|name| *name != "normal") {
            let _ = write!(style, ";mix-blend-mode:{name}");
        }
        let native = self.options.native;
        if let Some(sigma) = backdrop.and_then(|backdrop| backdrop_blur(backdrop, native)) {
            let _ = write!(style, ";backdrop-filter:blur({}px)", Number(sigma));
        }
        let _ = write!(attributes, r#" style="{style}""#);
        if let Some(Some(filter)) = layer_filter(paint, native) {
            let region = self.local_region(state, state.bounds(op));
            let id = self.filter(&filter, region);
            let _ = write!(attributes, r#" filter="url(#{id})""#);
        }
        let masked = holds_a_mask(scope);
        if masked {
            let id = self.id("m");
            let _ = write!(attributes, r#" mask="url(#{id})""#);
            self.masked_layers.push(MaskedLayer {
                mask_id: id,
                transform: state.transform(),
            });
        }
        self.scopes.push(Scope {
            masked,
            ..Scope::default()
        });
        self.group(attributes.trim_start());
    }

    /// `open_mask` starts filling the mask of the masked layer it is in, drawn in that layer's
    /// space, and returns whether it did: a mask outside a masked layer masks nothing the
    /// document holds.
    fn open_mask(&mut self, kind: MaskKind, state: &MatrixClipState) -> bool {
        let Some(MaskedLayer {
            mask_id: id,
            transform,
        }) = self.masked_layers.last().cloned()
        else {
            return false;
        };
        let Some(back) = transform.invert() else {
            return false;
        };
        let region = back.map_rect(&self.area);
        let kind = match kind {
            MaskKind::Luminance => "luminance",
            MaskKind::Alpha => "alpha",
        };
        self.writing.push(format!(
            r#"<mask id="{id}" mask-type="{kind}" maskUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}">"#,
            Number(region.x),
            Number(region.y),
            Number(region.width),
            Number(region.height)
        ));
        self.scopes.push(Scope {
            fills_mask: true,
            ..Scope::default()
        });
        if let Some(transform) = matrix(&back.then(&state.transform())) {
            self.group(&format!(r#"transform="{transform}""#));
        }
        true
    }

    /// `close` ends the innermost scope: its groups, and the mask it filled.
    fn close(&mut self) {
        let Some(scope) = self.scopes.pop() else {
            return;
        };
        for _ in 0..scope.groups {
            self.element("</g>");
        }
        if scope.masked {
            self.masked_layers.pop();
        }
        if scope.fills_mask {
            self.element("</mask>");
            if let Some(mask) = self.writing.pop() {
                self.defs.push_str(&mask);
            }
        }
    }

    /// `group` opens a `<g>` the innermost scope closes.
    fn group(&mut self, attributes: &str) {
        self.element(&format!("<g {attributes}>"));
        if let Some(scope) = self.scopes.last_mut() {
            scope.groups += 1;
        }
    }

    fn clip(&mut self, path: &Path, rule: FillRule, op: ClipOp) {
        let id = self.id("c");
        let (d, rule) = match op {
            ClipOp::Intersect => (path_data(path), rule_name(rule)),
            // A clip outside a shape is everywhere with the shape cut out, exact when the
            // shape's even-odd and non-zero insides agree, as an outline's do.
            ClipOp::Difference => (format!("{EVERYWHERE}{}", path_data(path)), "evenodd"),
        };
        let _ = write!(
            self.defs,
            r#"<clipPath id="{id}" clipPathUnits="userSpaceOnUse"><path d="{d}" clip-rule="{rule}"/></clipPath>"#
        );
        self.group(&format!(r#"clip-path="url(#{id})""#));
    }

    fn path(&mut self, path: &Path, rule: FillRule, paint: &Paint, local_bounds: Rect) {
        let attributes = self.paint_attributes(paint, rule, local_bounds);
        self.element(&format!(r#"<path d="{}"{attributes}/>"#, path_data(path)));
    }

    /// `glyph_run` writes each glyph as a use of its definition: the outlines filled with the
    /// run's paint, the colour glyphs in their own colours, faded by the paint's alpha.
    fn glyph_run(
        &mut self,
        font: &RunFont,
        paint: &Paint,
        glyphs: &[GlyphPos],
        bounds: Rect,
    ) -> Result<(), SvgWriteError> {
        let mut outlines = String::new();
        let mut colored = String::new();
        for glyph in glyphs {
            let (uses, id) = match self.glyph(font, glyph.id)? {
                Some(GlyphDefinition::Outline(id)) => (&mut outlines, id),
                Some(GlyphDefinition::Color(id)) => (&mut colored, id),
                None => continue,
            };
            let _ = write!(
                uses,
                r##"<use xlink:href="#{id}" x="{}" y="{}"/>"##,
                Number(glyph.x),
                Number(glyph.y)
            );
        }
        if !outlines.is_empty() {
            let attributes = self.paint_attributes(paint, FillRule::NonZero, bounds);
            self.element(&format!("<g{attributes}>{outlines}</g>"));
        }
        if !colored.is_empty() {
            let mut attributes = self.effect_attributes(paint, bounds, 0.0);
            if paint.color.a < 1.0 {
                let _ = write!(attributes, r#" opacity="{}""#, Number(paint.color.a));
            }
            self.element(&format!("<g{attributes}>{colored}</g>"));
        }
        Ok(())
    }

    /// `glyph` is a glyph's definition, made the first time it is used: its colour image when it
    /// has one, as valo's renderer prefers, else its outline; `None` for a glyph with neither,
    /// such as a space.
    fn glyph(
        &mut self,
        font: &RunFont,
        glyph: u32,
    ) -> Result<Option<GlyphDefinition>, SvgWriteError> {
        let key = GlyphKey {
            face: font.face.uid(),
            glyph,
            size: font.size.to_bits(),
            embolden: font.synthesis.embolden.to_bits(),
            skew: font.synthesis.skew.to_bits(),
        };
        if let Some(def) = self.glyphs.get(&key) {
            return Ok(def.clone());
        }
        let scale = self.options.pixels_per_unit;
        let def = if let Some(image) = self.rasterizer.color(&font.face, glyph, font.size * scale) {
            Some(GlyphDefinition::Color(self.color_glyph(&image, scale)?))
        } else {
            valo_text::glyph_path(&font.face, glyph, font.size, font.synthesis).map(|path| {
                let id = self.id("t");
                let _ = write!(self.defs, r#"<path id="{id}" d="{}"/>"#, path_data(&path));
                GlyphDefinition::Outline(id)
            })
        };
        self.glyphs.insert(key, def.clone());
        Ok(def)
    }

    /// `color_glyph` defines a colour glyph's image, drawn at `scale` pixels per unit, around
    /// the glyph's origin, and returns its id.
    fn color_glyph(&mut self, image: &GlyphImage, scale: f32) -> Result<String, SvgWriteError> {
        let layout = PixelLayout::packed(
            [image.width, image.height],
            PixelFormat::Rgba8,
            AlphaType::Premultiplied,
        );
        let pixels = PixelBuffer::new(layout, image.data.clone())
            .map_err(|error| SvgWriteError::Encode(error.to_string()))?;
        let href = encoded_href(&pixels, Encoding::Png, "image/png")?;
        let id = self.id("e");
        let _ = write!(
            self.defs,
            r#"<image id="{id}" x="{}" y="{}" width="{}" height="{}" xlink:href="{href}"/>"#,
            Number(image.left as f32 / scale),
            Number(-image.top as f32 / scale),
            Number(image.width as f32 / scale),
            Number(image.height as f32 / scale)
        );
        Ok(id)
    }

    /// `paint_attributes` are a draw's fill or stroke, and its blend mode and filter, which
    /// spreads from `local_bounds`, its ink before any effect.
    fn paint_attributes(&mut self, paint: &Paint, rule: FillRule, local_bounds: Rect) -> String {
        let colour = self.paint_value(paint);
        let mut attributes = String::new();
        let alpha = paint.color.a;
        let mut reach = 0.0;
        match &paint.style {
            PaintStyle::Fill => {
                let _ = write!(attributes, r#" fill="{colour}""#);
                if alpha < 1.0 {
                    let _ = write!(attributes, r#" fill-opacity="{}""#, Number(alpha));
                }
                if rule == FillRule::EvenOdd {
                    attributes.push_str(r#" fill-rule="evenodd""#);
                }
            }
            PaintStyle::Stroke(stroke) => {
                let _ = write!(
                    attributes,
                    r#" fill="none" stroke="{colour}" stroke-width="{}""#,
                    Number(stroke.width)
                );
                if alpha < 1.0 {
                    let _ = write!(attributes, r#" stroke-opacity="{}""#, Number(alpha));
                }
                match stroke.cap {
                    Cap::Butt => {}
                    Cap::Round => attributes.push_str(r#" stroke-linecap="round""#),
                    Cap::Square => attributes.push_str(r#" stroke-linecap="square""#),
                }
                match stroke.join {
                    Join::Miter => {
                        let _ = write!(
                            attributes,
                            r#" stroke-miterlimit="{}""#,
                            Number(stroke.miter_limit)
                        );
                    }
                    Join::Round => attributes.push_str(r#" stroke-linejoin="round""#),
                    Join::Bevel => attributes.push_str(r#" stroke-linejoin="bevel""#),
                }
                if let Some(dash) = &stroke.dash {
                    let intervals: Vec<String> = dash
                        .intervals
                        .iter()
                        .map(|v| Number(*v).to_string())
                        .collect();
                    let _ = write!(
                        attributes,
                        r#" stroke-dasharray="{}" stroke-dashoffset="{}""#,
                        intervals.join(" "),
                        Number(dash.phase)
                    );
                }
                reach = stroke.width / 2.0 * stroke.miter_limit.max(1.0);
            }
        }
        attributes.push_str(&self.effect_attributes(paint, local_bounds, reach));
        attributes
    }

    /// `effect_attributes` are a draw's blend mode and filter, which spreads from
    /// `local_bounds` grown by `reach`.
    fn effect_attributes(&mut self, paint: &Paint, local_bounds: Rect, reach: f32) -> String {
        let mut attributes = String::new();
        if let Some(name) = blend_name(paint.blend_mode).filter(|name| *name != "normal") {
            let _ = write!(attributes, r#" style="mix-blend-mode:{name}""#);
        }
        if let Some(Some(filter)) = draw_filter(paint, self.options.native) {
            let region = local_bounds.expand(reach + filter.reach());
            let id = self.filter(&filter, region);
            let _ = write!(attributes, r#" filter="url(#{id})""#);
        }
        attributes
    }

    /// `paint_value` is what a paint fills or strokes with: its colour, or its gradient.
    fn paint_value(&mut self, paint: &Paint) -> String {
        match &paint.shader {
            Some(shader) => match self.gradient(shader) {
                Some(id) => format!("url(#{id})"),
                None => hex(paint.color),
            },
            None => hex(paint.color),
        }
    }

    /// `gradient` defines a linear or radial gradient and returns its id.
    fn gradient(&mut self, shader: &Shader) -> Option<String> {
        let id = self.id("g");
        let (element, geometry, stops, spread, local) = match shader {
            Shader::Linear {
                start,
                end,
                stops,
                spread,
                local,
            } => (
                "linearGradient",
                format!(
                    r#"x1="{}" y1="{}" x2="{}" y2="{}""#,
                    Number(start.x),
                    Number(start.y),
                    Number(end.x),
                    Number(end.y)
                ),
                stops,
                spread,
                local,
            ),
            Shader::Radial {
                center,
                radius,
                stops,
                spread,
                focus,
                local,
            } => {
                let mut geometry = format!(
                    r#"cx="{}" cy="{}" r="{}""#,
                    Number(center.x),
                    Number(center.y),
                    Number(*radius)
                );
                if let Some(focus) = focus {
                    let _ = write!(
                        geometry,
                        r#" fx="{}" fy="{}" fr="{}""#,
                        Number(focus.center.x),
                        Number(focus.center.y),
                        Number(focus.radius)
                    );
                }
                ("radialGradient", geometry, stops, spread, local)
            }
            Shader::Sweep { .. } | Shader::Image { .. } | Shader::Runtime(_) => return None,
        };
        let mut attributes = format!(r#"id="{id}" gradientUnits="userSpaceOnUse" {geometry}"#);
        if let Some(transform) = matrix(local) {
            let _ = write!(attributes, r#" gradientTransform="{transform}""#);
        }
        match spread {
            SpreadMode::Pad => {}
            SpreadMode::Repeat => attributes.push_str(r#" spreadMethod="repeat""#),
            SpreadMode::Reflect => attributes.push_str(r#" spreadMethod="reflect""#),
        }
        let _ = write!(
            self.defs,
            "<{element} {attributes}>{}</{element}>",
            stops_of(stops)
        );
        Some(id)
    }

    /// `filter` defines a filter over `region`, in the user space of what it filters, and
    /// returns its id. It filters in sRGB, as valo does.
    fn filter(&mut self, filter: &Filter, region: Rect) -> String {
        let id = self.id("f");
        let primitives = match filter {
            Filter::Blur([x, y]) => {
                format!(
                    r#"<feGaussianBlur stdDeviation="{} {}"/>"#,
                    Number(*x),
                    Number(*y)
                )
            }
            Filter::Shadow([x, y], colour) => format!(
                r#"<feGaussianBlur in="SourceGraphic" stdDeviation="{} {}" result="blur"/><feFlood flood-color="{}" flood-opacity="{}"/><feComposite in2="blur" operator="in"/>"#,
                Number(*x),
                Number(*y),
                hex(*colour),
                Number(colour.a)
            ),
            Filter::Matrix(values) => {
                let values: Vec<String> = values
                    .iter()
                    .map(|v| PreciseNumber(*v).to_string())
                    .collect();
                format!(
                    r#"<feColorMatrix type="matrix" values="{}"/>"#,
                    values.join(" ")
                )
            }
            Filter::Flood(colour) => format!(
                r#"<feFlood flood-color="{}" flood-opacity="{}"/><feComposite in2="SourceGraphic" operator="in"/>"#,
                hex(*colour),
                Number(colour.a)
            ),
        };
        let _ = write!(
            self.defs,
            r#"<filter id="{id}" filterUnits="userSpaceOnUse" x="{}" y="{}" width="{}" height="{}" color-interpolation-filters="sRGB">{primitives}</filter>"#,
            Number(region.x),
            Number(region.y),
            Number(region.width),
            Number(region.height)
        );
        id
    }

    /// `picture` draws the next region's picture where it was drawn in the document, from where
    /// `state` places what is drawn now.
    fn picture(&mut self, state: &MatrixClipState) {
        let Some(Some(picture)) = self.pictures.next() else {
            return;
        };
        let Some(back) = state.transform().invert() else {
            return;
        };
        let bounds = picture.bounds;
        let transform = matrix(&back)
            .map(|transform| format!(r#" transform="{transform}""#))
            .unwrap_or_default();
        self.element(&format!(
            r#"<image{transform} x="{}" y="{}" width="{}" height="{}" preserveAspectRatio="none" xlink:href="{}"/>"#,
            Number(bounds.x),
            Number(bounds.y),
            Number(bounds.width),
            Number(bounds.height),
            picture.href
        ));
    }

    /// `local_region` is `bounds`, in the document's space, in the space `state` draws in; the
    /// whole document for bounds that only a clip bounds.
    fn local_region(&self, state: &MatrixClipState, bounds: Bounds) -> Rect {
        let rect = bounds.rect().unwrap_or(self.area);
        state
            .transform()
            .invert()
            .map_or(rect, |back| back.map_rect(&rect))
    }

    fn element(&mut self, element: &str) {
        if let Some(writing) = self.writing.last_mut() {
            writing.push_str(element);
        }
    }

    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    /// `finish` is the whole SVG, a unit of the list to a pixel.
    fn finish(mut self, size: [f32; 2]) -> String {
        while !self.scopes.is_empty() {
            self.close();
        }
        let body = self.writing.swap_remove(0);
        let (width, height) = (Number(size[0]), Number(size[1]));
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><defs>{}</defs>{body}</svg>"#,
            self.defs
        )
    }
}

fn stops_of(stops: &[GradientStop]) -> String {
    let mut out = String::new();
    for stop in stops {
        let _ = write!(
            out,
            r#"<stop offset="{}" stop-color="{}""#,
            Number(stop.offset),
            hex(stop.color)
        );
        if stop.color.a < 1.0 {
            let _ = write!(out, r#" stop-opacity="{}""#, Number(stop.color.a));
        }
        out.push_str("/>");
    }
    out
}

fn rule_name(rule: FillRule) -> &'static str {
    match rule {
        FillRule::NonZero => "nonzero",
        FillRule::EvenOdd => "evenodd",
    }
}
