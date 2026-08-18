//! Filter recipes: multi-pass work over finished textures — separable
//! gaussian blur chains (downsample + horizontal + vertical), colour-filter
//! passes, drop shadows, styled mask combines, and the per-layer effect
//! ordering. Every recipe composes the same primitive: one quad into an
//! exactly-sized 1-sample target, appended to the plan as its own
//! independent pass. [`FilteredTexture`] is the composition contract —
//! which texture, and which corner of it holds real content.
//!
//! Filter passes append to the plan directly (they are independent of any
//! frame's segments); the emitter still makes every step.

use valo_dl::{BlendMode, BlurStyle, ColorFilter, ImageFilter, MaskBlur};
use valo_geometry::{Color, Point, Rect};

use crate::frame::{PassColor, PlannedPass, TextureCopy};
use crate::images::IMAGE_FORMAT;
use crate::pipelines::{blur_style_id, Frag};
use crate::pool::FILTER_SIZE_BUCKET;

use super::emit::{
    encode_color_filter, filter_quad_record, EncodedColorFilter, PAYLOAD_GEOM, PAYLOAD_MISC,
};
use super::layers::{layer_texture_size, LayerInfo};
use super::Planner;

/// `LayerEffects` is what a paint asks of its layer at composite time.
#[derive(Default)]
pub(super) struct LayerEffects {
    pub color_filter: Option<ColorFilter>,
    pub image_filter: Option<ImageFilter>,
    /// σ is in DEVICE px by the time it lands here. Non-Normal styles add a
    /// combine pass merging the blur with the sharp layer.
    pub blur: Option<MaskBlur>,
    /// Set for a recorded `save_layer`; clear for the implicit layers a draw
    /// opens for its own effects.
    pub subpass: bool,
    /// The effect transform's 2×2 basis `[a, b, c, d]`. The whole basis is
    /// kept, not its two axis lengths: an image filter's σ is a VECTOR, and
    /// under rotation its axes have to move with the matrix.
    pub image_basis: [f32; 4],
}

impl LayerEffects {
    /// `of` collects the effects a paint asks of its layer. The transform
    /// converts local blur axes into device-pixel sigma, as Impeller's
    /// effect transform does.
    pub fn of(
        paint: &valo_dl::Paint,
        mask_scale: f32,
        image_transform: &valo_geometry::Matrix,
        subpass: bool,
    ) -> Self {
        let [a, b, c, d, ..] = image_transform.to_affine();
        Self {
            color_filter: paint.color_filter,
            image_filter: paint.effective_image_filter().cloned(),
            blur: paint.mask_blur.map(|mask| MaskBlur {
                sigma: (mask.sigma * mask_scale).max(0.05),
                style: mask.style,
            }),
            subpass,
            image_basis: [a, b, c, d],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.color_filter.is_none() && self.image_filter.is_none() && self.blur.is_none()
    }
}

/// `SharedBlur` is one shared backdrop key's blur.
///
/// Registered by the first tile replayed; later same-key layers in the
/// same target seed from it without another pass break.
pub(super) struct SharedBlur {
    pub view: wgpu::TextureView,
    /// The blurred region, absolute replay coords.
    pub region: Rect,
    pub uv_max: [f32; 2],
    /// Device σ the blur ran at — a tile whose σ differs (a transform can
    /// split record-time-equal σs) blurs independently instead.
    pub sigma: f32,
    /// The target the blur snapshotted. A same-key tile in a DIFFERENT
    /// target (a materialized layer vs the main frame) must not reuse it:
    /// the coords and the pixels both belong to the other texture.
    pub source: wgpu::Texture,
}

/// `FilteredTexture` is a finished filter chain's output.
///
/// Sample `view` up to `uv_max` — the used corner of the bucketed,
/// possibly downsampled target.
#[derive(Clone)]
pub(super) struct FilteredTexture {
    pub view: wgpu::TextureView,
    pub uv_max: [f32; 2],
    /// The target's own dimensions — bucketed, so bigger than the region
    /// actually used. A pass that reads this texture must scale its uv by
    /// THESE, not by the layer's size.
    pub size: [u32; 2],
}

impl FilteredTexture {
    pub fn source(view: wgpu::TextureView, size: [u32; 2], whole: &Rect) -> Self {
        Self {
            view,
            uv_max: [whole.width / size[0] as f32, whole.height / size[1] as f32],
            size,
        }
    }
}

/// `ColorFilterTarget` names where a colour-filter pass writes.
///
/// Needed because targets differ by caller: the image cache writes RGBA8,
/// layer filters write the frame format.
pub(super) struct ColorFilterTarget {
    pub view: wgpu::TextureView,
    pub size: [u32; 2],
    pub format: wgpu::TextureFormat,
}

impl Planner<'_> {
    /// `composite_source` is what a layer's composite samples: the texture
    /// as-is when the paint carried no effects, otherwise the finished
    /// filter chain. UVs always divide by the INTEGER texture extent — the
    /// layer's rect is fractional but its texture is ceil-sized, and mixing
    /// the two stretches content by up to a texel at the right/bottom edges.
    pub(super) fn composite_source(&mut self, info: &LayerInfo) -> (wgpu::TextureView, [f32; 4]) {
        let size = layer_texture_size(&info.rect);
        let sample = Rect::new(info.rect.x, info.rect.y, size[0] as f32, size[1] as f32);
        if info.effects.is_empty() {
            return (info.resolve.clone(), super::emit::full_rect_uv(&sample));
        }
        let whole = Rect::new(0.0, 0.0, size[0] as f32, size[1] as f32);
        let filtered = if info.effects.subpass {
            self.blur_then_recolour(info, size, &whole)
        } else {
            self.recolour_then_blur(info, size, &whole)
        };
        (filtered.view, region_uv(&sample, filtered.uv_max))
    }

    /// `recolour_then_blur` is a draw's own effect layer: the colour filter
    /// runs on the shape's pixels and the blur spreads the filtered result.
    /// A styled blur combines against the filtered layer, not the raw one.
    fn recolour_then_blur(
        &mut self,
        info: &LayerInfo,
        size: [u32; 2],
        whole: &Rect,
    ) -> FilteredTexture {
        if info.effects.image_filter.is_none() {
            return self.recolour_then_mask_blur(info, size, whole);
        }
        let mut output = FilteredTexture::source(info.resolve.clone(), size, whole);
        if let Some(filter) = info.effects.color_filter {
            output = self.push_color_filter_input(&output, whole, filter);
        }
        if let Some(filter) = &info.effects.image_filter.clone() {
            output = self.push_image_filter(&output, whole, filter, info.effects.image_basis);
        }
        if let Some(mask) = info.effects.blur {
            output = self.blur_filtered_layer(&output, whole, mask);
        }
        output
    }

    /// `blur_then_recolour` is a `save_layer` subpass: the blur runs first
    /// and the colour filter recolours the blurred result, so a translating
    /// or clamping matrix acts on the halo's fractional alpha the way
    /// Flutter's does.
    fn blur_then_recolour(
        &mut self,
        info: &LayerInfo,
        size: [u32; 2],
        whole: &Rect,
    ) -> FilteredTexture {
        if info.effects.image_filter.is_none() {
            return self.mask_blur_then_recolour(info, size, whole);
        }
        let mut output = FilteredTexture::source(info.resolve.clone(), size, whole);
        if let Some(filter) = &info.effects.image_filter.clone() {
            output = self.push_image_filter(&output, whole, filter, info.effects.image_basis);
        }
        if let Some(mask) = info.effects.blur {
            output = self.blur_filtered_layer(&output, whole, mask);
        }
        if let Some(filter) = info.effects.color_filter {
            output = self.push_color_filter_input(&output, whole, filter);
        }
        output
    }

    /// `recolour_then_mask_blur` colour-filters the layer, then mask-blurs
    /// the filtered (or raw) pixels — the no-`ImageFilter` half of
    /// [`Planner::recolour_then_blur`]. Empty effects never reach here.
    fn recolour_then_mask_blur(
        &mut self,
        info: &LayerInfo,
        size: [u32; 2],
        whole: &Rect,
    ) -> FilteredTexture {
        let recoloured = info
            .effects
            .color_filter
            .map(|filter| self.push_color_filter(&info.resolve, size, whole, filter));
        let (sharp, sharp_size) = recoloured.as_ref().map_or_else(
            || (info.resolve.clone(), size),
            |output| (output.view.clone(), output.size),
        );
        match info.effects.blur {
            None => recoloured.expect("empty effects returned early"),
            Some(mask) => self.blur_layer(&sharp, sharp_size, whole, mask),
        }
    }

    /// `mask_blur_then_recolour` mask-blurs the layer, then colour-filters
    /// the halo — the no-`ImageFilter` half of
    /// [`Planner::blur_then_recolour`].
    fn mask_blur_then_recolour(
        &mut self,
        info: &LayerInfo,
        size: [u32; 2],
        whole: &Rect,
    ) -> FilteredTexture {
        let blurred = info
            .effects
            .blur
            .map(|mask| self.blur_layer(&info.resolve, size, whole, mask));
        let Some(filter) = info.effects.color_filter else {
            return blurred.expect("empty effects returned early");
        };
        match blurred {
            // A downsampled blur's output is smaller than the layer it stands
            // for. Only the `_input` helper reads it at its own resolution;
            // the raw one takes its source for full semantic size and would
            // shove the halo into the layer's top-left corner.
            Some(blurred) => self.push_color_filter_input(&blurred, whole, filter),
            None => self.push_color_filter(&info.resolve, size, whole, filter),
        }
    }

    /// `blur_layer` gaussian-blurs a full-size layer texture. Non-Normal
    /// styles combine the blur with the sharp source in a second pass.
    fn blur_layer(
        &mut self,
        source: &wgpu::TextureView,
        size: [u32; 2],
        whole: &Rect,
        mask: MaskBlur,
    ) -> FilteredTexture {
        let blurred = self.plan_blur(source, size, whole, mask.sigma, Vec::new());
        match mask.style {
            BlurStyle::Normal => blurred,
            style => self.push_mask_combine(&blurred, source, whole, style),
        }
    }

    /// `blur_filtered_layer` gaussian-blurs a prior filter stage that may
    /// only occupy a bucket corner. Styled combines upsample the sharp
    /// input first so both textures share a coordinate space.
    fn blur_filtered_layer(
        &mut self,
        source: &FilteredTexture,
        whole: &Rect,
        mask: MaskBlur,
    ) -> FilteredTexture {
        let blurred = self.plan_blur_input(source, whole, mask.sigma, mask.sigma);
        match mask.style {
            BlurStyle::Normal => blurred,
            style => {
                let sharp = self.materialize_filter_input(source, whole);
                self.push_mask_combine(&blurred, &sharp.view, whole, style)
            }
        }
    }

    /// `blur_of_target_region` ends the current segment, snapshots `region`
    /// (absolute replay coords) from the live target, and blurs it — the
    /// copy rides the blur chain's FIRST pass (which runs between this
    /// frame's segments), not the frame's next segment.
    pub(super) fn blur_of_target_region(&mut self, region: &Rect, sigma: f32) -> FilteredTexture {
        self.emit_segment();
        self.stats.snapshots += 1;
        let (size, origin, src) = {
            let frame = self.frame();
            (frame.size, frame.origin, frame.src_texture.clone())
        };
        let snapshot = self.pool.take_snapshot(size, self.format);
        let mut copies = Vec::new();
        if let Some((copy_origin, extent)) = super::segments::snapshot_region(region, origin, size)
        {
            copies.push(TextureCopy {
                src,
                dst: snapshot.texture.clone(),
                origin: copy_origin,
                size: extent,
            });
        }
        let local = Rect::new(
            region.x - origin.x,
            region.y - origin.y,
            region.width,
            region.height,
        );
        self.plan_blur(&snapshot.view, size, &local, sigma, copies)
    }

    /// `plan_blur` blurs `region` (px inside `source`, sized `source_size`):
    /// downsample until the effective σ is ≤ ~4, then one horizontal + one
    /// vertical separable pass; the composite's bilinear sampling upscales
    /// for free. Passes append to the plan at the CURRENT position —
    /// callers emit their frame's segment first.
    pub(super) fn plan_blur(
        &mut self,
        source: &wgpu::TextureView,
        source_size: [u32; 2],
        region: &Rect,
        sigma: f32,
        pre_copies: Vec<TextureCopy>,
    ) -> FilteredTexture {
        let scale = blur_scale(sigma);
        let work = [
            (region.width * scale).round().max(1.0),
            (region.height * scale).round().max(1.0),
        ];
        let mut copies = pre_copies;
        let mut src = source.clone();
        let mut src_px = [source_size[0] as f32, source_size[1] as f32];
        // `None` while the source is still the caller's texture, where the
        // content is a sub-rect; `Some(uv_max)` once it is a pooled target
        // whose content starts at the origin.
        let mut src_uv_max: Option<[f32; 2]> = None;
        // Downsample passes (σ=0 blur = plain bilinear resample), halving at
        // most 2× each so bilinear reads every texel — one big jump would
        // alias exactly what the blur is meant to average.
        let mut cur = [region.width, region.height];
        while scale < 1.0 && (cur[0] > work[0] || cur[1] > work[1]) {
            let next = [
                (cur[0] * 0.5).max(work[0]).round().max(1.0),
                (cur[1] * 0.5).max(work[1]).round().max(1.0),
            ];
            let uv = match src_uv_max {
                None => source_region_uv(region, source_size, next),
                Some(uv_max) => resample_uv(uv_max, next),
            };
            let (view, bucket) =
                self.push_filter_pass(&src, uv, next, 0.0, [0.0, 0.0], std::mem::take(&mut copies));
            src = view;
            src_px = bucket;
            src_uv_max = Some([next[0] / bucket[0], next[1] / bucket[1]]);
            cur = next;
        }
        let work_sigma = sigma * scale;
        let src_uv = match src_uv_max {
            None => source_region_uv(region, source_size, work),
            Some(uv_max) => resample_uv(uv_max, work),
        };
        let (h_view, h_bucket) = self.push_filter_pass(
            &src,
            src_uv,
            work,
            work_sigma,
            [1.0 / src_px[0], 0.0],
            std::mem::take(&mut copies),
        );
        let (v_view, v_bucket) = self.push_filter_pass(
            &h_view,
            corner_uv(h_bucket),
            work,
            work_sigma,
            [0.0, 1.0 / h_bucket[1]],
            Vec::new(),
        );
        FilteredTexture {
            view: v_view,
            uv_max: [work[0] / v_bucket[0], work[1] / v_bucket[1]],
            size: [v_bucket[0] as u32, v_bucket[1] as u32],
        }
    }

    /// `plan_blur_input` blurs a semantic full-size filter input whose used
    /// pixels may occupy only a bucket corner. The UV mapping carries that
    /// resolution through an ordered image-filter chain without stretching
    /// intermediate output.
    fn plan_blur_input(
        &mut self,
        source: &FilteredTexture,
        whole: &Rect,
        sigma_x: f32,
        sigma_y: f32,
    ) -> FilteredTexture {
        let scale_x = blur_scale(sigma_x);
        let scale_y = blur_scale(sigma_y);
        let work = [
            (whole.width * scale_x).round().max(1.0),
            (whole.height * scale_y).round().max(1.0),
        ];
        let mut source_view = source.view.clone();
        let mut source_pixels = [source.size[0] as f32, source.size[1] as f32];
        let mut source_uv_max = source.uv_max;
        let mut current = [whole.width, whole.height];
        while current[0] > work[0] || current[1] > work[1] {
            let next = [
                (current[0] * 0.5).max(work[0]).round().max(1.0),
                (current[1] * 0.5).max(work[1]).round().max(1.0),
            ];
            let (view, bucket) = self.push_filter_pass(
                &source_view,
                resample_uv(source_uv_max, next),
                next,
                0.0,
                [0.0, 0.0],
                Vec::new(),
            );
            source_view = view;
            source_pixels = bucket;
            source_uv_max = [next[0] / bucket[0], next[1] / bucket[1]];
            current = next;
        }
        let (horizontal, horizontal_bucket) = self.push_filter_pass(
            &source_view,
            resample_uv(source_uv_max, work),
            work,
            sigma_x * scale_x,
            [1.0 / source_pixels[0], 0.0],
            Vec::new(),
        );
        let (vertical, vertical_bucket) = self.push_filter_pass(
            &horizontal,
            corner_uv(horizontal_bucket),
            work,
            sigma_y * scale_y,
            [0.0, 1.0 / horizontal_bucket[1]],
            Vec::new(),
        );
        FilteredTexture {
            view: vertical,
            uv_max: [work[0] / vertical_bucket[0], work[1] / vertical_bucket[1]],
            size: [vertical_bucket[0] as u32, vertical_bucket[1] as u32],
        }
    }

    /// `push_image_filter` runs a flattened [`ImageFilter`] chain on
    /// `source`. Consecutive blurs merge in device space (quadrature) so a
    /// rotation mixes axes before they combine, matching Impeller's
    /// per-filter `CalculateBlurInfo`.
    fn push_image_filter(
        &mut self,
        source: &FilteredTexture,
        whole: &Rect,
        filter: &ImageFilter,
        basis: [f32; 4],
    ) -> FilteredTexture {
        let mut stages = Vec::new();
        image_filter_stages(filter, &mut stages);
        let mut output = source.clone();
        let mut index = 0;
        while index < stages.len() {
            match stages[index] {
                ImageFilter::Color(filter) => {
                    output = self.push_color_filter_input(&output, whole, *filter);
                    index += 1;
                }
                ImageFilter::Blur { .. } => {
                    // The basis has to reach every stage BEFORE they combine.
                    // Impeller transforms each blur's σ on its own —
                    // `CalculateBlurInfo` runs per filter — and a rotation
                    // MIXES the axes, so combining in local space first and
                    // rotating the total afterwards blurs along the wrong
                    // ones. Successive gaussians still compose in quadrature
                    // once they are on the same axes, so the merged device σ
                    // collapses back to a single pass.
                    let mut sigma_x_squared = 0.0;
                    let mut sigma_y_squared = 0.0;
                    while let Some(ImageFilter::Blur { sigma_x, sigma_y }) = stages.get(index) {
                        let [device_x, device_y] = device_sigma(basis, *sigma_x, *sigma_y);
                        sigma_x_squared += device_x * device_x;
                        sigma_y_squared += device_y * device_y;
                        index += 1;
                    }
                    let (sigma_x, sigma_y) = (sigma_x_squared.sqrt(), sigma_y_squared.sqrt());
                    if sigma_x <= 0.0 && sigma_y <= 0.0 {
                        continue;
                    }
                    output = self.plan_blur_input(&output, whole, sigma_x, sigma_y);
                }
                ImageFilter::DropShadow {
                    offset,
                    sigma_x,
                    sigma_y,
                    color,
                } => {
                    output = self.plan_drop_shadow(
                        &output,
                        whole,
                        *offset,
                        [*sigma_x, *sigma_y],
                        *color,
                        basis,
                    );
                    index += 1;
                }
                ImageFilter::Compose { .. } => unreachable!("composition was flattened"),
            }
        }
        output
    }

    /// `materialize_filter_input` resamples a filter stage onto the layer's
    /// full semantic size when its used corner is smaller (a downsampled
    /// blur). Styled mask-combines need both inputs in the same coordinate
    /// space.
    fn materialize_filter_input(
        &mut self,
        source: &FilteredTexture,
        whole: &Rect,
    ) -> FilteredTexture {
        let expected = [
            whole.width / source.size[0] as f32,
            whole.height / source.size[1] as f32,
        ];
        if (source.uv_max[0] - expected[0]).abs() < 1e-6
            && (source.uv_max[1] - expected[1]).abs() < 1e-6
        {
            return source.clone();
        }
        let work = [whole.width, whole.height];
        let (view, bucket) = self.push_filter_pass(
            &source.view,
            region_uv(whole, source.uv_max),
            work,
            0.0,
            [0.0, 0.0],
            Vec::new(),
        );
        FilteredTexture {
            view,
            uv_max: [work[0] / bucket[0], work[1] / bucket[1]],
            size: [bucket[0] as u32, bucket[1] as u32],
        }
    }

    /// `push_filter_pass` is one quad filling an exactly-sized 1-sample
    /// target; fs_blur taps `source` along `step` (radius 0 = plain
    /// resample).
    fn push_filter_pass(
        &mut self,
        source: &wgpu::TextureView,
        source_uv: [f32; 4],
        work: [f32; 2],
        sigma: f32,
        step: [f32; 2],
        pre_copies: Vec<TextureCopy>,
    ) -> (wgpu::TextureView, [f32; 2]) {
        let extent = [exact_extent(work[0]), exact_extent(work[1])];
        let target = self.pool.take_filter(extent, self.format);
        let quad = Rect::new(0.0, 0.0, work[0], work[1]);
        let radius = if sigma > 0.0 {
            (sigma * 2.5).ceil().min(48.0)
        } else {
            0.0
        };
        let mut record = filter_quad_record(&quad, extent);
        record.set_payload(PAYLOAD_GEOM, source_uv);
        record.set_payload(PAYLOAD_MISC, [sigma, radius, step[0], step[1]]);
        let bind = self.emit.texture_bind(source);
        self.push_filter(target.view.clone(), Frag::Blur, record, bind, pre_copies);
        (target.view, [extent[0] as f32, extent[1] as f32])
    }

    /// `push_color_filter` recolours a raw layer texture in one filter pass.
    fn push_color_filter(
        &mut self,
        source: &wgpu::TextureView,
        source_size: [u32; 2],
        whole: &Rect,
        filter: ColorFilter,
    ) -> FilteredTexture {
        let work = [whole.width, whole.height];
        let bucket = [filter_bucket(work[0]), filter_bucket(work[1])];
        let target = self.pool.take_filter(bucket, self.format);
        self.push_color_filter_to(
            source,
            source_size,
            whole,
            filter,
            ColorFilterTarget {
                view: target.view.clone(),
                size: bucket,
                format: self.format,
            },
        );
        FilteredTexture {
            view: target.view,
            uv_max: [work[0] / bucket[0] as f32, work[1] / bucket[1] as f32],
            size: bucket,
        }
    }

    /// `push_color_filter_input` recolours a layer texture while preserving
    /// the used corner of a bucketed or downsampled prior stage.
    fn push_color_filter_input(
        &mut self,
        source: &FilteredTexture,
        whole: &Rect,
        filter: ColorFilter,
    ) -> FilteredTexture {
        let work = [whole.width, whole.height];
        let bucket = [filter_bucket(work[0]), filter_bucket(work[1])];
        let target = self.pool.take_filter(bucket, self.format);
        let mut record = filter_quad_record(whole, bucket);
        record.set_payload(PAYLOAD_GEOM, region_uv(whole, source.uv_max));
        let fragment = match encode_color_filter(&mut record, filter) {
            EncodedColorFilter::Matrix => Frag::ColorMatrix,
            EncodedColorFilter::Blend => Frag::ColorBlend,
        };
        let bind = self.emit.texture_bind(&source.view);
        self.push_filter(target.view.clone(), fragment, record, bind, Vec::new());
        FilteredTexture {
            view: target.view,
            uv_max: [work[0] / bucket[0] as f32, work[1] / bucket[1] as f32],
            size: bucket,
        }
    }

    /// `push_color_filter_to` recolours `source` into a caller-owned target
    /// (the image-atlas path writes RGBA8; layer filters write the frame
    /// format).
    pub(super) fn push_color_filter_to(
        &mut self,
        source: &wgpu::TextureView,
        source_size: [u32; 2],
        whole: &Rect,
        filter: ColorFilter,
        target: ColorFilterTarget,
    ) {
        let mut record = filter_quad_record(whole, target.size);
        record.set_payload(
            PAYLOAD_GEOM,
            source_region_uv(whole, source_size, [whole.width, whole.height]),
        );
        let frag = match encode_color_filter(&mut record, filter) {
            EncodedColorFilter::Matrix => Frag::ColorMatrix,
            EncodedColorFilter::Blend => Frag::ColorBlend,
        };
        let bind = self.emit.texture_bind(source);
        self.push_filter_with_format(target.view, target.format, frag, record, bind, Vec::new());
    }

    /// `plan_drop_shadow` is Skia's `SkImageFilters::DropShadow` lowering:
    /// tint the input's alpha with the shadow colour, blur it, displace it,
    /// and put the untouched input back on top. The blur runs on the tinted
    /// copy rather than the input so a translucent shadow colour spreads at
    /// its own alpha.
    fn plan_drop_shadow(
        &mut self,
        source: &FilteredTexture,
        whole: &Rect,
        offset: Point,
        sigma: [f32; 2],
        color: Color,
        basis: [f32; 4],
    ) -> FilteredTexture {
        let tint = ColorFilter::Blend(color, BlendMode::SrcIn);
        let tinted = self.push_color_filter_input(source, whole, tint);
        let [device_x, device_y] = skia_sigma(basis, sigma[0], sigma[1]);
        let shadow = if device_x > 0.0 || device_y > 0.0 {
            self.plan_blur_input(&tinted, whole, device_x, device_y)
        } else {
            tinted
        };
        self.push_drop_shadow_combine(&shadow, source, whole, device_offset(basis, offset))
    }

    /// `push_drop_shadow_combine` merges the offset shadow with the sharp
    /// layer into one texture (fs_drop_shadow), so the result composites
    /// like any other layer.
    fn push_drop_shadow_combine(
        &mut self,
        shadow: &FilteredTexture,
        sharp: &FilteredTexture,
        whole: &Rect,
        offset: [f32; 2],
    ) -> FilteredTexture {
        let work = [whole.width, whole.height];
        let bucket = [filter_bucket(work[0]), filter_bucket(work[1])];
        let target = self.pool.take_filter(bucket, self.format);
        let mut record = filter_quad_record(whole, bucket);
        record.set_payload(
            PAYLOAD_GEOM,
            [
                shadow.uv_max[0] / work[0],
                shadow.uv_max[1] / work[1],
                sharp.uv_max[0] / work[0],
                sharp.uv_max[1] / work[1],
            ],
        );
        record.set_payload(
            PAYLOAD_MISC,
            [offset[0], offset[1], 1.0 / work[0], 1.0 / work[1]],
        );
        let bind = self.emit.blend_bind(&shadow.view, &sharp.view);
        self.push_filter(
            target.view.clone(),
            Frag::DropShadow,
            record,
            bind,
            Vec::new(),
        );
        FilteredTexture {
            view: target.view,
            uv_max: [work[0] / bucket[0] as f32, work[1] / bucket[1] as f32],
            size: bucket,
        }
    }

    /// `push_mask_combine` merges blur B with the SHARP layer M into one
    /// texture (fs_mask_combine) so styled masks composite like any other
    /// draw — advanced blends too.
    fn push_mask_combine(
        &mut self,
        blur: &FilteredTexture,
        sharp: &wgpu::TextureView,
        whole: &Rect,
        style: BlurStyle,
    ) -> FilteredTexture {
        let work = [whole.width, whole.height];
        let bucket = [filter_bucket(work[0]), filter_bucket(work[1])];
        let target = self.pool.take_filter(bucket, self.format);
        let mut record = filter_quad_record(whole, bucket);
        record.set_payload(
            PAYLOAD_GEOM,
            [blur.uv_max[0] / work[0], blur.uv_max[1] / work[1], 0.0, 0.0],
        );
        record.set_payload(
            PAYLOAD_MISC,
            [
                blur_style_id(style) as f32,
                0.0,
                1.0 / work[0],
                1.0 / work[1],
            ],
        );
        let bind = self.emit.blend_bind(&blur.view, sharp);
        self.push_filter(
            target.view.clone(),
            Frag::MaskCombine,
            record,
            bind,
            Vec::new(),
        );
        FilteredTexture {
            view: target.view,
            uv_max: [work[0] / bucket[0] as f32, work[1] / bucket[1] as f32],
            size: bucket,
        }
    }

    /// `push_filter` is one-quad filter pass appended to the plan at the
    /// current position.
    fn push_filter(
        &mut self,
        target: wgpu::TextureView,
        frag: Frag,
        record: super::emit::UniformRecord,
        bind: wgpu::BindGroup,
        pre_copies: Vec<TextureCopy>,
    ) {
        self.push_filter_with_format(target, self.format, frag, record, bind, pre_copies);
    }

    /// `push_filter_with_format` appends a filter pass, allowing a target
    /// format other than the frame's (filtered-image cache entries are
    /// [`IMAGE_FORMAT`], not the surface).
    fn push_filter_with_format(
        &mut self,
        target: wgpu::TextureView,
        target_format: wgpu::TextureFormat,
        frag: Frag,
        record: super::emit::UniformRecord,
        bind: wgpu::BindGroup,
        pre_copies: Vec<TextureCopy>,
    ) {
        let step = self.emit.filter_step(target_format, frag, record, bind);
        self.passes.push(PlannedPass {
            color: PassColor::Filter { view: target },
            depth: None,
            clear: Some(Color::TRANSPARENT),
            clear_depth: false,
            store: true, // the next pass samples this
            pre_copies,
            steps: vec![step],
        });
        self.stats.filter_passes += 1;
    }

    /// `filtered_image` is a cached colour-filtered copy of `source`. On a
    /// cache miss the filter pass is appended now — the current segment is
    /// emitted first so the new independent pass does not land in the middle
    /// of in-flight steps.
    pub(super) fn filtered_image(
        &mut self,
        source: &valo_dl::Image,
        filter: ColorFilter,
    ) -> valo_dl::Image {
        let (filtered, created) = self.emit.filtered_image_entry(source, filter);
        if created {
            // Preserve ordering with any frame segment already accumulated;
            // the new independent pass then produces the cached texture.
            self.emit_segment();
            let whole = Rect::new(0.0, 0.0, source.width(), source.height());
            self.push_color_filter_to(
                source.view(),
                source.size(),
                &whole,
                filter,
                ColorFilterTarget {
                    view: filtered.view().clone(),
                    size: source.size(),
                    format: IMAGE_FORMAT,
                },
            );
        }
        filtered
    }
}

/// `blur_scale` is Impeller's CalculateScale: σ ≤ 4 runs full resolution;
/// past that, downsample until the effective σ is ~4. No floor — a fixed
/// one truncated the gaussian tail past device σ ≈ 300 (boxy,
/// bright-edged); the downsample chain halves per pass, so depth stays
/// log₂ regardless.
fn blur_scale(sigma: f32) -> f32 {
    if sigma <= 4.0 {
        return 1.0;
    }
    (4.0 / sigma).log2().round().exp2()
}

/// `filter_bucket` snaps filter targets up to the pool bucket so chains
/// share textures.
fn filter_bucket(px: f32) -> u32 {
    (px.ceil().max(1.0) as u32).div_ceil(FILTER_SIZE_BUCKET) * FILTER_SIZE_BUCKET
}

/// `exact_extent` sizes a blur pass's target to exactly the texels it
/// writes, the way Impeller allocates `subpass_size`. Bucketing would leave
/// cleared texels beside the used corner; a blur's ±radius taps and the
/// composite's linear upscale both read past that corner, so gutter texels
/// would fade the far borders and weaken the blur (half its out-of-range
/// taps returning transparent instead of the edge). The cost, paid
/// knowingly: an animating blur can allocate a fresh target per frame
/// (bounded by the pool's eviction).
fn exact_extent(px: f32) -> u32 {
    px.ceil().max(1.0) as u32
}

/// `source_region_uv` maps local (0..work px) → uv spanning `region` inside
/// a `source_size` texture (the blur chain's first read; work < region px
/// when downsampling).
fn source_region_uv(region: &Rect, source_size: [u32; 2], work: [f32; 2]) -> [f32; 4] {
    let sw = source_size[0] as f32;
    let sh = source_size[1] as f32;
    [
        region.width / (sw * work[0]),
        region.height / (sh * work[1]),
        region.x / sw,
        region.y / sh,
    ]
}

/// `corner_uv` maps local (0..work px) → uv in a bucketed intermediate
/// whose used corner starts at the origin.
fn corner_uv(bucket: [f32; 2]) -> [f32; 4] {
    [1.0 / bucket[0], 1.0 / bucket[1], 0.0, 0.0]
}

/// `resample_uv` maps a filter pass's local quad (0..`work`) onto the whole
/// used corner of its source, whose content ends at `source_uv_max`.
///
/// The TARGET size belongs in this mapping, not just the source's: a
/// downsample pass draws a smaller quad and still has to cover the entire
/// source. Dividing by the source's own extent instead reads only the
/// top-left `work/source` fraction of it, which is why a blur whose σ
/// crosses the downsample threshold used to lose its left and top spread.
fn resample_uv(source_uv_max: [f32; 2], work: [f32; 2]) -> [f32; 4] {
    [
        source_uv_max[0] / work[0],
        source_uv_max[1] / work[1],
        0.0,
        0.0,
    ]
}

/// `image_filter_stages` flattens `Compose` nodes into inner-to-outer order.
fn image_filter_stages<'a>(filter: &'a ImageFilter, stages: &mut Vec<&'a ImageFilter>) {
    match filter {
        ImageFilter::Compose { outer, inner } => {
            image_filter_stages(inner, stages);
            image_filter_stages(outer, stages);
        }
        stage => stages.push(stage),
    }
}

/// `device_sigma` maps a local-space blur σ onto the device axes, as
/// Impeller's `GaussianBlurFilterContents` does it: transform σ as a VECTOR
/// by the effect transform's basis, then take the component-wise absolute
/// value. Collapsing the basis to its two axis LENGTHS instead would lose
/// the rotation — a quarter turn has unit-length axes, so an anisotropic σ
/// would pass through unswapped and blur along the wrong axis.
fn device_sigma(basis: [f32; 4], sigma_x: f32, sigma_y: f32) -> [f32; 2] {
    let [a, b, c, d] = basis;
    [
        (sigma_x * a + sigma_y * c).abs(),
        (sigma_x * b + sigma_y * d).abs(),
    ]
}

/// `skia_sigma` is the same mapping as Skia's, for the filters whose
/// reference is Skia rather than Impeller: each axis BASIS maps separately,
/// taking its length (`SkImageFilterTypes.cpp`'s `mapSize`). The two rules
/// disagree under a 45° rotation — drop shadow follows Skia because
/// `SkImageFilters::DropShadow` is what CSS `drop-shadow()` lowers to;
/// plain [`ImageFilter::Blur`] keeps Impeller's rule.
fn skia_sigma(basis: [f32; 4], sigma_x: f32, sigma_y: f32) -> [f32; 2] {
    let [a, b, c, d] = basis;
    [sigma_x * a.hypot(b), sigma_y * c.hypot(d)]
}

/// `device_offset` maps a local-space filter offset onto the device axes.
/// Sign-preserving, unlike [`device_sigma`] — a shadow that leans right has
/// to keep leaning right after the basis mirrors or rotates it.
fn device_offset(basis: [f32; 4], offset: Point) -> [f32; 2] {
    let [a, b, c, d] = basis;
    [offset.x * a + offset.y * c, offset.x * b + offset.y * d]
}

/// `region_uv` maps local (absolute px) → uv into a filtered texture
/// holding `region`, whose used corner ends at `uv_max`.
pub(super) fn region_uv(region: &Rect, uv_max: [f32; 2]) -> [f32; 4] {
    let sx = uv_max[0] / region.width.max(1e-6);
    let sy = uv_max[1] / region.height.max(1e-6);
    [sx, sy, -region.x * sx, -region.y * sy]
}

#[cfg(test)]
mod tests {
    use super::{device_sigma, skia_sigma};
    use valo_geometry::Matrix;

    fn basis_of(matrix: &Matrix) -> [f32; 4] {
        let [a, b, c, d, ..] = matrix.to_affine();
        [a, b, c, d]
    }

    fn assert_sigma(actual: [f32; 2], expected: [f32; 2]) {
        assert!(
            (actual[0] - expected[0]).abs() < 1e-4 && (actual[1] - expected[1]).abs() < 1e-4,
            "sigma {actual:?} != {expected:?}"
        );
    }

    /// Drop shadow follows `SkImageFilters::Blur`'s `SkSize` mapping, so a
    /// rotation must leave an isotropic σ isotropic. Impeller's vector rule
    /// turns the same input into a directional smear.
    #[test]
    fn skia_sigma_keeps_a_rotated_blur_round() {
        let basis = basis_of(&Matrix::rotation(std::f32::consts::FRAC_PI_4));
        assert_sigma(skia_sigma(basis, 10.0, 10.0), [10.0, 10.0]);
        assert_sigma(device_sigma(basis, 10.0, 10.0), [0.0, 14.142136]);
    }

    #[test]
    fn skia_sigma_still_scales_each_axis() {
        let basis = basis_of(&Matrix::scale(2.0, 3.0));
        assert_sigma(skia_sigma(basis, 4.0, 5.0), [8.0, 15.0]);
    }

    #[test]
    fn device_sigma_scales_each_axis() {
        let basis = basis_of(&Matrix::scale(2.0, 3.0));
        assert_sigma(device_sigma(basis, 4.0, 5.0), [8.0, 15.0]);
    }

    /// A quarter turn maps local x onto device y. Reducing the basis to its
    /// axis LENGTHS would give [1, 1] here and leave σ unswapped — the bug
    /// this function exists to prevent.
    #[test]
    fn device_sigma_swaps_axes_under_a_quarter_turn() {
        let basis = basis_of(&Matrix::rotation(std::f32::consts::FRAC_PI_2));
        assert_sigma(device_sigma(basis, 12.0, 3.0), [3.0, 12.0]);
    }

    /// Impeller takes the component-wise absolute value, so a half turn is
    /// indistinguishable from no rotation at all.
    #[test]
    fn device_sigma_is_sign_agnostic() {
        let basis = basis_of(&Matrix::rotation(std::f32::consts::PI));
        assert_sigma(device_sigma(basis, 7.0, 2.0), [7.0, 2.0]);
    }

    /// Rotation composed with non-uniform scale: each device axis takes a
    /// contribution from BOTH local sigmas, which is the case an axis-length
    /// reduction cannot express at any angle.
    #[test]
    fn device_sigma_mixes_axes_under_rotation_and_scale() {
        let matrix = Matrix::scale(2.0, 3.0).then(&Matrix::rotation(std::f32::consts::FRAC_PI_4));
        let [a, b, c, d, ..] = matrix.to_affine();
        let expected = [(6.0 * a + 4.0 * c).abs(), (6.0 * b + 4.0 * d).abs()];
        assert_sigma(device_sigma(basis_of(&matrix), 6.0, 4.0), expected);
        // Both axes genuinely mix — a degenerate basis would pass vacuously.
        assert!(expected[0] > 1.0 && expected[1] > 1.0);
    }

    /// A mirror has a negative determinant; `.Abs()` makes it equivalent to
    /// its unmirrored twin.
    #[test]
    fn device_sigma_ignores_a_mirror() {
        let mirrored = basis_of(&Matrix::scale(-2.0, 3.0));
        assert_sigma(device_sigma(mirrored, 4.0, 5.0), [8.0, 15.0]);
    }

    /// Two blurs under a 45° rotation. Transforming each stage and THEN
    /// combining leaves both device axes equal; combining in local space
    /// first and rotating the total collapses σx to zero and piles
    /// everything onto y — the divergence from Impeller's per-filter
    /// `CalculateBlurInfo` that this ordering exists to avoid.
    #[test]
    fn composed_blurs_transform_before_they_combine() {
        let basis = basis_of(&Matrix::rotation(std::f32::consts::FRAC_PI_4));
        let stages = [(10.0f32, 0.0f32), (0.0f32, 10.0f32)];

        let (mut x_squared, mut y_squared) = (0.0, 0.0);
        for (sigma_x, sigma_y) in stages {
            let [x, y] = device_sigma(basis, sigma_x, sigma_y);
            x_squared += x * x;
            y_squared += y * y;
        }
        assert_sigma([x_squared.sqrt(), y_squared.sqrt()], [10.0, 10.0]);

        let combined = device_sigma(basis, 10.0, 10.0);
        assert!(
            combined[0] < 0.001 && combined[1] > 14.0,
            "combining first must be the WRONG answer, got {combined:?}"
        );
    }
}
