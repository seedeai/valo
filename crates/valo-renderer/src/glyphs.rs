use rustc_hash::FxHashMap;
use std::sync::Arc;

use valo_geometry::{Cap, Join, Path};
use valo_text::{Font, GlyphImage, GlyphStroke, Rasterizer};

/// Skia's kMaxMultitexturePages: open pages up to this, then GC.
const MAX_PAGES: usize = 4;

/// The text tier thresholds (device px), Skia's defaults: direct masks below
/// `sdf_min` (162, `kLargeDFFontLimit`), SDF up to `path_min` (324,
/// `glyphsAsPathsFontSize`), outlines beyond. Skia's zoom-heavy mode
/// (`kUseDeviceIndependentFonts`) is `sdf_min` ≈ 18–64: near raster-free
/// zoom, softer small text — one knob, no separate mode.
#[derive(Clone, Copy, Debug)]
pub struct TextTiers {
    pub sdf_min: f32,
    pub path_min: f32,
}

impl Default for TextTiers {
    fn default() -> Self {
        Self {
            sdf_min: 162.0,
            path_min: 324.0,
        }
    }
}

/// The SDF strike sizes, ascending (Skia's kSmall/kMedium/kLargeDFFontLimit
/// plus one for the static profile's 162–324 band). One raster serves a
/// whole zoom band; under a text-raster hold the OTHER buckets of a glyph
/// are its stand-in candidates.
pub(crate) const SDF_BUCKETS: [f32; 4] = [32.0, 72.0, 162.0, 256.0];
/// Transparent gutter between entries so linear sampling never bleeds.
const GUTTER: i32 = 1;
const DEFAULT_PAGE_SIZE: u32 = 2048;

/// A packed glyph: where it lives in its page (uv) and how the bitmap hangs
/// off the glyph origin (`left` right of origin, `top` above the baseline —
/// swash placement, y-up).
#[derive(Clone, Copy, Debug)]
pub struct AtlasGlyph {
    pub uv: [f32; 4],
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

/// Which atlas page a glyph landed on. `color` selects the RGBA page family
/// (emoji); mask/SDF glyphs live on R8 pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PageRef {
    pub color: bool,
    pub index: usize,
}

struct PathEntry {
    path: Option<Arc<Path>>,
    last_used: u64,
}

/// Frames an unused no-page-space entry (outline path, whitespace
/// placeholder) survives before the sweep — same policy as ContourCache.
const IDLE_FRAMES: u64 = 3;

/// What an atlas entry holds. The stroke rides along because a stroked
/// glyph is nothing more than another cached image — Impeller hashes the
/// same parameters into `SubpixelGlyph`, Skia into `SkScalerContextRec`.
/// A stroked SDF is deliberately not expressible: an SDF encodes distance
/// from a FILL boundary, so it would be a different field, and Impeller's
/// stroked glyphs go to the regular atlas for the same reason.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Coverage {
    Fill,
    Sdf,
    Stroke(GlyphStroke),
}

/// [`Coverage`] made hashable. Stroke parameters quantize to 1/16 px —
/// finer than an antialiased edge resolves, and coarse enough that a
/// wobbling animated width does not mint an entry per frame. The raster
/// reads its parameters back out of this, so the image an entry holds is
/// always exactly what its key says.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum CoverageKey {
    Fill,
    Sdf,
    Stroke {
        width_16: u32,
        miter_16: u32,
        cap: u8,
        join: u8,
    },
}

/// Sixteenths, saturating — a negative or NaN parameter can only ever have
/// come from a caller, never from the tier policy.
fn sixteenths(value: f32) -> u32 {
    (value * 16.0).round().clamp(0.0, u32::MAX as f32) as u32
}

impl CoverageKey {
    fn of(coverage: Coverage) -> Self {
        match coverage {
            Coverage::Fill => Self::Fill,
            Coverage::Sdf => Self::Sdf,
            Coverage::Stroke(stroke) => Self::Stroke {
                width_16: sixteenths(stroke.width),
                miter_16: sixteenths(stroke.miter_limit),
                cap: match stroke.cap {
                    Cap::Butt => 0,
                    Cap::Round => 1,
                    Cap::Square => 2,
                },
                join: match stroke.join {
                    Join::Miter => 0,
                    Join::Round => 1,
                    Join::Bevel => 2,
                },
            },
        }
    }

    fn coverage(self) -> Coverage {
        match self {
            Self::Fill => Coverage::Fill,
            Self::Sdf => Coverage::Sdf,
            Self::Stroke {
                width_16,
                miter_16,
                cap,
                join,
            } => Coverage::Stroke(GlyphStroke {
                width: width_16 as f32 / 16.0,
                miter_limit: miter_16 as f32 / 16.0,
                cap: match cap {
                    1 => Cap::Round,
                    2 => Cap::Square,
                    _ => Cap::Butt,
                },
                join: match join {
                    1 => Join::Round,
                    2 => Join::Bevel,
                    _ => Join::Miter,
                },
            }),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    /// The font INSTANCE's stable raster identity ([`Font::uid`]).
    font: u64,
    glyph: u32,
    /// EXACT raster size (f32 bits) — quantization is the tier policy's job.
    px_bits: u32,
    /// Quarter-pixel x phase (0..4), mask tier only — Skia/Impeller's
    /// subpixel positioning.
    phase: u8,
    coverage: CoverageKey,
}

impl GlyphKey {
    fn new(font: u64, glyph: u32, px: f32, coverage: Coverage, phase: u8) -> Self {
        Self {
            font,
            glyph,
            px_bits: px.to_bits(),
            phase,
            coverage: CoverageKey::of(coverage),
        }
    }

    fn px(&self) -> f32 {
        f32::from_bits(self.px_bits)
    }
}

struct Page {
    allocator: etagere::AtlasAllocator,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind: Option<wgpu::BindGroup>,
    /// CPU copy of the page. New glyphs land here and the union of their
    /// rects uploads as ONE `write_texture` per frame — Impeller and Skia
    /// batch atlas uploads the same way, and per-glyph calls are the wasm
    /// frame killer: each one is a browser API crossing.
    shadow: Option<Box<[u8]>>,
    /// Dirty region awaiting flush: [x0, y0, x1, y1), texels.
    dirty: Option<[u32; 4]>,
}

/// A glyph's page space: where it lives plus the allocator handle that can
/// give that exact rectangle back (WebRender's texture-cache shape — its
/// eviction is entry-granular on this same allocator crate).
#[derive(Clone, Copy)]
struct Resident {
    page: PageRef,
    glyph: AtlasGlyph,
    slot: etagere::AllocId,
}

/// Rects freed per eviction attempt — enough to make progress, small enough
/// that a lucky early fit doesn't over-evict.
const EVICT_BATCH: usize = 64;

/// The renderer's glyph cache: rasterize misses via valo-text,
/// pack with etagere, upload the region, hand out page + uv. Pages grow to
/// [`MAX_PAGES`] per family; a full family evicts its least-recently-USED
/// entries one rect at a time (glyphs drawn this frame are
/// untouchable, so hot SDFs outlive any churn of stale mask scales). The
/// wholesale GC survives only as the last-resort defragmenter. Mid-frame
/// eviction is safe: already-emitted steps keep their pages alive through
/// their bind groups (wgpu refcounts), so their uvs stay valid until submit.
pub struct GlyphStore {
    device: wgpu::Device,
    queue: wgpu::Queue,
    raster: Rasterizer,
    page_size: u32,
    mask_pages: Vec<Page>,
    color_pages: Vec<Page>,
    entries: FxHashMap<GlyphKey, (Option<Resident>, u64)>,
    /// Outline tier cache: (font, glyph, px bits) → baseline-origin path.
    paths: FxHashMap<(u64, u32, u32), PathEntry>,
    /// Frame counter for the idle sweeps (see `end_frame`).
    frame: u64,
    /// Bumps on every GC — visible in tests and debugging.
    pub generation: u64,
    pub tiers: TextTiers,
    /// This frame's cache misses (each = one swash raster) — the stats
    /// line's cache-health number. A warm frame reads 0.
    rasters: u32,
    /// Wholesale family GCs this frame; nonzero on a warm frame means the
    /// live set no longer fits the pages (thrash).
    gcs: u32,
    /// The host-owned gesture switch (OPT-IN, default off):
    /// while held, a text-raster miss (SDF bucket OR bitmap scale) whose
    /// glyph is resident at ANOTHER size skips rasterizing — the planner
    /// draws the stand-in scaled (soft, mid-pinch Chrome). Misses with no
    /// stand-in still raster: soft beats invisible. The host clears the
    /// hold when input goes idle and re-renders; valo keeps no clocks.
    hold: bool,
    /// Rasters skipped by the hold this frame — the HUD number that proves
    /// gesture frames are raster-free (and exposes a stuck hold).
    held: u32,
    /// OPT-IN (default off = Skia/browser behavior): skip
    /// glyph 0 (`.notdef`) in every text tier, so unresolved chars render
    /// blank instead of tofu boxes. Pair with [`FontDemand`] reporting —
    /// hiding the box without watching the demand silently loses text.
    hide_missing_glyphs: bool,
    /// Resident (px, phase) per (font, glyph, coverage) — the stand-in
    /// lookup. Bitmap scales are continuous (size × 1/200-quantized zoom),
    /// so "nearest resident size" needs an index; SDF rides the same one.
    /// A stroked entry's width is in raster pixels and so scales with `px`,
    /// which puts every size in its own coverage bucket: stroked runs
    /// simply find no stand-in and raster through a text-raster hold.
    sizes: FxHashMap<(u64, u32, CoverageKey), Vec<(f32, u8)>>,
}

impl GlyphStore {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self::with_page_size(device, queue, DEFAULT_PAGE_SIZE)
    }

    /// Test seam: tiny pages force the page-add and GC paths.
    pub fn with_page_size(device: &wgpu::Device, queue: &wgpu::Queue, page_size: u32) -> Self {
        Self {
            device: device.clone(),
            queue: queue.clone(),
            raster: Rasterizer::new(),
            page_size,
            mask_pages: Vec::new(),
            color_pages: Vec::new(),
            entries: FxHashMap::default(),
            frame: 0,
            paths: FxHashMap::default(),
            generation: 0,
            tiers: TextTiers::default(),
            rasters: 0,
            gcs: 0,
            hold: false,
            held: 0,
            hide_missing_glyphs: false,
            sizes: FxHashMap::default(),
        }
    }

    /// See the `hold` field: the host's gesture switch for text rasters.
    pub fn set_text_raster_hold(&mut self, held: bool) {
        self.hold = held;
    }

    /// See the `hide_missing_glyphs` field: blank instead of tofu, opt-in.
    pub fn set_hide_missing_glyphs(&mut self, hide: bool) {
        self.hide_missing_glyphs = hide;
    }

    /// The planner reads this once per run (a closure calling into the
    /// store would hold the borrow across the mutating batch loop).
    pub fn hides_missing_glyphs(&self) -> bool {
        self.hide_missing_glyphs
    }

    /// (cache-miss rasters, wholesale GCs, hold-skipped rasters) this frame.
    pub fn frame_counters(&self) -> (u32, u32, u32) {
        (self.rasters, self.gcs, self.held)
    }

    /// The registered collection, for overlays that lay text out against
    /// Rasterize/pack a whole run BEFORE anyone batches page references:
    /// packing may GC a family (all its pages drop), which would invalidate
    /// any `PageRef` taken earlier. A pass that completes without a GC
    /// proves every key coexists in the atlas (Impeller's collect-then-
    /// build, scoped to the run); a run too large to EVER fit (an emoji
    /// wall) stops retrying and drops its overflow glyphs for the frame —
    /// resident entries always point at live pages either way.
    pub fn ensure_run(&mut self, font: &Font, px: f32, coverage: Coverage, keys: &[(u32, u8)]) {
        for _ in 0..2 {
            let generation = self.generation;
            for &(glyph, phase) in keys {
                self.ensure(font, glyph, px, coverage, phase);
            }
            if self.generation == generation {
                return;
            }
        }
    }

    /// Read-only lookup after [`Self::ensure_run`] — never rasterizes, so
    /// it can never evict (page references stay valid while batching).
    pub fn entry(
        &self,
        font: u64,
        glyph: u32,
        px: f32,
        coverage: Coverage,
        phase: u8,
    ) -> Option<(PageRef, AtlasGlyph)> {
        let (slot, _) = self
            .entries
            .get(&GlyphKey::new(font, glyph, px, coverage, phase))?;
        slot.map(|r| (r.page, r.glyph))
    }

    /// The atlas slot for (font, glyph, px, coverage) — rasterizing,
    /// packing, and uploading on first sight. Color glyphs (emoji) win over
    /// the requested coverage and land on the RGBA family.
    fn ensure(&mut self, font: &Font, glyph: u32, px: f32, coverage: Coverage, phase: u8) {
        let key = GlyphKey::new(font.uid().0, glyph, px, coverage, phase);
        if let Some((_, last_used)) = self.entries.get_mut(&key) {
            *last_used = self.frame;
            return;
        }
        // Held misses with a stand-in skip the raster; NO entry lands, so
        // the key stays a miss and rasters on the first un-held frame.
        if self.hold
            && self
                .find_stand_in(key.font, key.glyph, key.coverage, key.px())
                .is_some()
        {
            self.held += 1;
            return;
        }
        let entry = self.rasterize_and_pack(key, font);
        if entry.is_some() {
            self.sizes
                .entry((key.font, key.glyph, key.coverage))
                .or_default()
                .push((key.px(), key.phase));
        }
        self.entries.insert(key, (entry, self.frame));
    }

    /// The nearest OTHER resident size of this glyph — what a held frame
    /// draws through, scaled. Marks the stand-in USED THIS FRAME: a later
    /// run's packing may evict idle rects, and an evicted-then-overwritten
    /// rect would corrupt quads already batched against it.
    pub fn resident_stand_in(
        &mut self,
        font: u64,
        glyph: u32,
        coverage: Coverage,
        wanted_px: f32,
    ) -> Option<(f32, PageRef, AtlasGlyph)> {
        let (key, resident) =
            self.find_stand_in(font, glyph, CoverageKey::of(coverage), wanted_px)?;
        if let Some((_, last_used)) = self.entries.get_mut(&key) {
            *last_used = self.frame;
        }
        Some((f32::from_bits(key.px_bits), resident.page, resident.glyph))
    }

    /// Read-only search over the glyph's resident sizes, nearest px first.
    fn find_stand_in(
        &self,
        font: u64,
        glyph: u32,
        coverage: CoverageKey,
        wanted_px: f32,
    ) -> Option<(GlyphKey, Resident)> {
        let mut best: Option<(GlyphKey, Resident, f32)> = None;
        for &(px, phase) in self.sizes.get(&(font, glyph, coverage))? {
            if px == wanted_px {
                continue;
            }
            let distance = (px - wanted_px).abs();
            if best.as_ref().is_some_and(|(.., d)| *d <= distance) {
                continue;
            }
            let key = GlyphKey {
                font,
                glyph,
                px_bits: px.to_bits(),
                phase,
                coverage,
            };
            if let Some((Some(resident), _)) = self.entries.get(&key) {
                best = Some((key, *resident, distance));
            }
        }
        best.map(|(key, resident, _)| (key, resident))
    }

    fn rasterize_and_pack(&mut self, key: GlyphKey, font: &Font) -> Option<Resident> {
        self.rasters += 1;
        let px = key.px();
        if let Some(image) = self.raster.color(font, key.glyph, px) {
            return self.pack(true, &image);
        }
        let dx = key.phase as f32 * 0.25;
        let image = match key.coverage.coverage() {
            Coverage::Sdf => self.raster.sdf(font, key.glyph, px),
            Coverage::Fill => self.raster.alpha(font, key.glyph, px, dx),
            Coverage::Stroke(stroke) => self.raster.stroked(font, key.glyph, px, dx, &stroke),
        }?;
        self.pack(false, &image)
    }

    /// Allocate on the family's pages: existing space → a new page → evict
    /// the coldest idle rects → wholesale GC as the last-resort defrag
    /// (fragmentation can starve a fit even with idle space freed).
    fn pack(&mut self, color: bool, image: &GlyphImage) -> Option<Resident> {
        if image.width == 0 || image.height == 0 {
            return None;
        }
        if image.width + 2 > self.page_size || image.height + 2 > self.page_size {
            debug_assert!(false, "glyph larger than an atlas page");
            return None;
        }
        loop {
            if let Some(hit) = self.try_pages(color, image) {
                return Some(hit);
            }
            if self.pages(color).len() < MAX_PAGES {
                self.add_page(color);
            } else if !self.evict_lru(color) {
                self.gc(color);
            }
        }
    }

    /// Free the least-recently-USED resident rects of the family until the
    /// packer can make progress. Entries drawn THIS frame are untouchable —
    /// so a run ensured earlier in the pass can never be invalidated by a
    /// later one, and evicted glyphs re-raster on their next sighting.
    /// `false` = nothing idle left (one frame truly outgrew the pages).
    fn evict_lru(&mut self, color: bool) -> bool {
        let mut idle: Vec<(u64, GlyphKey)> = self
            .entries
            .iter()
            .filter_map(|(key, (resident, used))| {
                let r = resident.as_ref()?;
                (r.page.color == color && *used < self.frame).then_some((*used, *key))
            })
            .collect();
        if idle.is_empty() {
            return false;
        }
        idle.sort_unstable_by_key(|(used, _)| *used);
        for (_, key) in idle.into_iter().take(EVICT_BATCH) {
            let Some((Some(r), _)) = self.entries.remove(&key) else {
                continue;
            };
            self.pages_mut(color)[r.page.index]
                .allocator
                .deallocate(r.slot);
            if let Some(list) = self.sizes.get_mut(&(key.font, key.glyph, key.coverage)) {
                list.retain(|&(px, phase)| px.to_bits() != key.px_bits || phase != key.phase);
            }
        }
        true
    }

    fn try_pages(&mut self, color: bool, image: &GlyphImage) -> Option<Resident> {
        let size = etagere::size2(
            image.width as i32 + GUTTER * 2,
            image.height as i32 + GUTTER * 2,
        );
        for index in 0..self.pages(color).len() {
            let Some(slot) = self.pages_mut(color)[index].allocator.allocate(size) else {
                continue;
            };
            let (x, y) = (
                (slot.rectangle.min.x + GUTTER) as u32,
                (slot.rectangle.min.y + GUTTER) as u32,
            );
            let page_size = self.page_size;
            stage(
                &mut self.pages_mut(color)[index],
                page_size,
                color,
                x,
                y,
                image,
            );
            let s = 1.0 / self.page_size as f32;
            return Some(Resident {
                page: PageRef { color, index },
                glyph: AtlasGlyph {
                    uv: [
                        x as f32 * s,
                        y as f32 * s,
                        (x + image.width) as f32 * s,
                        (y + image.height) as f32 * s,
                    ],
                    left: image.left as f32,
                    top: image.top as f32,
                    width: image.width as f32,
                    height: image.height as f32,
                },
                slot: slot.id,
            });
        }
        None
    }

    fn add_page(&mut self, color: bool) {
        let page = create_page(&self.device, self.page_size, color);
        self.pages_mut(color).push(page);
    }

    /// Last-resort defrag (eviction found nothing idle, or freed space too
    /// fragmented to fit): drop the family's pages ALL at once and restart
    /// with a fresh one. This frame's still-needed glyphs re-rasterize on
    /// demand; older steps keep the dropped textures alive via bind groups
    /// until submit.
    fn gc(&mut self, color: bool) {
        self.generation += 1;
        self.gcs += 1;
        self.pages_mut(color).clear();
        self.add_page(color);
        self.entries
            .retain(|_, (slot, _)| slot.is_none_or(|r| r.page.color != color));
        // Rare and wholesale — rebuilding the size index beats tracking it.
        self.sizes.clear();
        for (key, (slot, _)) in &self.entries {
            if slot.is_some() {
                self.sizes
                    .entry((key.font, key.glyph, key.coverage))
                    .or_default()
                    .push((key.px(), key.phase));
            }
        }
    }

    fn pages(&self, color: bool) -> &Vec<Page> {
        if color {
            &self.color_pages
        } else {
            &self.mask_pages
        }
    }

    fn pages_mut(&mut self, color: bool) -> &mut Vec<Page> {
        if color {
            &mut self.color_pages
        } else {
            &mut self.mask_pages
        }
    }

    /// Outline tier: the glyph as a path at `px`, cached until
    /// the size goes idle (animating text size would otherwise grow this
    /// forever — the audit's one unbounded cache).
    pub fn path(&mut self, font: &Font, glyph: u32, px: f32) -> Option<Arc<Path>> {
        let frame = self.frame;
        let entry = self
            .paths
            .entry((font.uid().0, glyph, px.to_bits()))
            .or_insert_with(|| PathEntry {
                path: valo_text::glyph_path(font, glyph, px),
                last_used: frame,
            });
        entry.last_used = frame;
        entry.path.clone()
    }

    /// Frame boundary: age-sweep the entries that hold NO page space —
    /// outline paths and whitespace placeholders. Rasters on pages stay
    /// until a full family evicts its coldest — idleness
    /// alone never frees page space, demand does.
    pub fn end_frame(&mut self) {
        self.frame += 1;
        self.rasters = 0;
        self.gcs = 0;
        self.held = 0;
        let now = self.frame;
        let expired = |last: u64| now.saturating_sub(last) > IDLE_FRAMES;
        self.paths.retain(|_, e| !expired(e.last_used));
        self.entries
            .retain(|_, (slot, last)| slot.is_some() || !expired(*last));
    }

    /// Push every dirty page region to the GPU — ONE `write_texture` per
    /// page per frame regardless of how many glyphs landed.
    /// The renderer calls this after planning, before encoding.
    pub fn flush_uploads(&mut self) {
        let (page_size, queue) = (self.page_size, self.queue.clone());
        for (pages, color) in [(&mut self.mask_pages, false), (&mut self.color_pages, true)] {
            for page in pages.iter_mut() {
                let (Some([x0, y0, x1, y1]), Some(shadow)) = (page.dirty.take(), &page.shadow)
                else {
                    continue;
                };
                let bpp = if color { 4u32 } else { 1 };
                let stride = page_size * bpp;
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &page.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: x0, y: y0, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &shadow[(y0 * stride + x0 * bpp) as usize..],
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(stride),
                        rows_per_image: None,
                    },
                    wgpu::Extent3d {
                        width: x1 - x0,
                        height: y1 - y0,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
    }

    /// The page's bind group (texture + linear sampler), built lazily.
    pub fn bind_group(&mut self, layout: &wgpu::BindGroupLayout, page: PageRef) -> wgpu::BindGroup {
        let device = self.device.clone();
        let entry = &mut self.pages_mut(page.color)[page.index];
        if entry.bind.is_none() {
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("valo.glyph_atlas"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            entry.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("valo.glyph_atlas"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&entry.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            }));
        }
        entry.bind.clone().expect("just built")
    }
}

fn create_page(device: &wgpu::Device, size: u32, color: bool) -> Page {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(if color {
            "valo.glyph_atlas.color"
        } else {
            "valo.glyph_atlas.mask"
        }),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: if color {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            wgpu::TextureFormat::R8Unorm
        },
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    Page {
        allocator: etagere::AtlasAllocator::new(etagere::size2(size as i32, size as i32)),
        view: texture.create_view(&Default::default()),
        texture,
        bind: None,
        shadow: None,
        dirty: None,
    }
}

/// Copy the glyph into the page's CPU shadow and grow the dirty region —
/// the GPU sees it at `flush_uploads`, one call per page per frame.
fn stage(page: &mut Page, page_size: u32, color: bool, x: u32, y: u32, image: &GlyphImage) {
    let bpp = if color { 4u32 } else { 1 };
    let stride = (page_size * bpp) as usize;
    let shadow = page
        .shadow
        .get_or_insert_with(|| vec![0u8; stride * page_size as usize].into_boxed_slice());
    for row in 0..image.height {
        let src = (row * image.width * bpp) as usize;
        let dst = (y + row) as usize * stride + (x * bpp) as usize;
        shadow[dst..dst + (image.width * bpp) as usize]
            .copy_from_slice(&image.data[src..src + (image.width * bpp) as usize]);
    }
    let (x1, y1) = (x + image.width, y + image.height);
    page.dirty = Some(match page.dirty {
        None => [x, y, x1, y1],
        Some([dx0, dy0, dx1, dy1]) => [dx0.min(x), dy0.min(y), dx1.max(x1), dy1.max(y1)],
    });
}

impl GlyphStore {
    /// Atlas families: [mask/SDF (R8), color (RGBA8)].
    pub(crate) fn report_atlas(&self) -> [crate::AtlasReport; 2] {
        let page_px = self.page_size as u64 * self.page_size as u64;
        let family = |pages: &Vec<Page>, color: bool| crate::AtlasReport {
            pages: pages.len() as u32,
            bytes: pages.len() as u64 * page_px * if color { 4 } else { 1 },
            entries: self
                .entries
                .values()
                .filter(|(slot, _)| slot.is_some_and(|r| r.page.color == color))
                .count() as u32,
        };
        [
            family(&self.mask_pages, false),
            family(&self.color_pages, true),
        ]
    }

    /// The outline-tier path cache (point bytes; verbs are noise).
    pub(crate) fn report_paths(&self) -> crate::PoolReport {
        let bytes: usize = self
            .paths
            .values()
            .filter_map(|e| e.path.as_ref())
            .map(|p| p.heap_bytes())
            .sum();
        crate::PoolReport {
            count: self.paths.len() as u32,
            bytes: bytes as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .ok()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
    }

    use valo_text::FontCollection;

    fn fira() -> FontCollection {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/fonts/fira_sans.ttf"
        );
        let mut c = FontCollection::new();
        c.register("Fira Sans", std::fs::read(path).unwrap())
            .unwrap();
        c
    }

    /// Tiny pages force the whole policy: fill page 1, open pages 2..4,
    /// then GC (generation bumps, entries purge, packing continues).
    #[test]
    fn pages_grow_then_gc_wholesale() {
        let Some((device, queue)) = headless() else {
            eprintln!("SKIP pages_grow_then_gc_wholesale: no GPU adapter");
            return;
        };
        let fonts = fira();
        let mut store = GlyphStore::with_page_size(&device, &queue, 64);
        let font = fonts.family("Fira Sans").unwrap();

        let mut packed = 0;
        for ch in ('A'..='Z').chain('a'..='z').chain('0'..='9') {
            let Some(glyph) = fonts.get(font).glyph_for(ch) else {
                continue;
            };
            store.ensure(fonts.get(font), glyph, 30.0, Coverage::Fill, 0);
            if store
                .entry(fonts.get(font).uid().0, glyph, 30.0, Coverage::Fill, 0)
                .is_some()
            {
                packed += 1;
            }
        }
        assert!(
            packed >= 50,
            "everything packs despite tiny pages: {packed}"
        );
        // Everything landed in ONE frame, so nothing was idle to evict —
        // overflow had to take the wholesale last-resort path.
        assert!(
            store.generation >= 1,
            "the page budget forced at least one GC"
        );

        // Post-GC, an early (purged) glyph re-ensures fine.
        let a = fonts.get(font).glyph_for('A').unwrap();
        store.ensure(fonts.get(font), a, 30.0, Coverage::Fill, 0);
        assert!(store
            .entry(fonts.get(font).uid().0, a, 30.0, Coverage::Fill, 0)
            .is_some());
    }

    /// Across frames, overflow evicts the least-recently-USED
    /// rects instead of wiping the family — the new set packs, the coldest
    /// old rects leave, and the wholesale GC never fires.
    #[test]
    fn overflow_evicts_coldest_instead_of_wiping() {
        let Some((device, queue)) = headless() else {
            eprintln!("SKIP overflow_evicts_coldest_instead_of_wiping: no GPU adapter");
            return;
        };
        let fonts = fira();
        let mut store = GlyphStore::with_page_size(&device, &queue, 64);
        let font = fonts.family("Fira Sans").unwrap();
        let glyph = |ch: char| fonts.get(font).glyph_for(ch).unwrap();

        // Frame 0: fill most of the budget, without overflowing it.
        let old: Vec<char> = ('a'..='r').collect();
        for &ch in &old {
            store.ensure(fonts.get(font), glyph(ch), 30.0, Coverage::Fill, 0);
        }
        let generation_before = store.generation;
        store.end_frame();

        // Frame 1: a same-sized new set overflows — idle frame-0 rects go.
        let new: Vec<char> = ('A'..='R').collect();
        for &ch in &new {
            store.ensure(fonts.get(font), glyph(ch), 30.0, Coverage::Fill, 0);
        }
        assert_eq!(
            store.generation, generation_before,
            "eviction sufficed; the wholesale GC never fired"
        );
        for &ch in &new {
            assert!(
                store
                    .entry(fonts.get(font).uid().0, glyph(ch), 30.0, Coverage::Fill, 0)
                    .is_some(),
                "'{ch}' of the hot set is resident"
            );
        }
        let evicted = old
            .iter()
            .filter(|&&ch| {
                store
                    .entry(fonts.get(font).uid().0, glyph(ch), 30.0, Coverage::Fill, 0)
                    .is_none()
            })
            .count();
        assert!(evicted > 0, "some cold frame-0 rects were evicted");

        // An evicted glyph re-ensures on demand (and may evict in turn).
        store.end_frame();
        store.ensure(fonts.get(font), glyph('a'), 30.0, Coverage::Fill, 0);
        assert!(store
            .entry(fonts.get(font).uid().0, glyph('a'), 30.0, Coverage::Fill, 0)
            .is_some());
    }

    /// While the host holds text rasters, an SDF miss with a
    /// resident other-bucket stand-in skips the raster (drawn scaled by the
    /// planner); a miss with NO stand-in still rasters; releasing the hold
    /// rasters the wanted bucket on the next sighting.
    #[test]
    fn text_raster_hold_reuses_resident_buckets() {
        let Some((device, queue)) = headless() else {
            eprintln!("SKIP text_raster_hold_reuses_resident_buckets: no GPU adapter");
            return;
        };
        let fonts = fira();
        let mut store = GlyphStore::new(&device, &queue);
        let font = fonts.family("Fira Sans").unwrap();
        let glyph = fonts.get(font).glyph_for('H').unwrap();

        // Warm bucket 32, then zoom crosses to 72 under a hold.
        store.ensure(fonts.get(font), glyph, 32.0, Coverage::Sdf, 0);
        store.end_frame();
        store.set_text_raster_hold(true);

        store.ensure(fonts.get(font), glyph, 72.0, Coverage::Sdf, 0);
        assert!(
            store
                .entry(fonts.get(font).uid().0, glyph, 72.0, Coverage::Sdf, 0)
                .is_none(),
            "held: the wanted bucket was not rasterized"
        );
        let (px, ..) = store
            .resident_stand_in(fonts.get(font).uid().0, glyph, Coverage::Sdf, 72.0)
            .expect("the warm bucket stands in");
        assert_eq!(px, 32.0);
        assert_eq!(store.frame_counters().2, 1, "one held raster counted");

        // First sight of a glyph with no stand-in rasters even while held.
        let fresh = fonts.get(font).glyph_for('Q').unwrap();
        store.ensure(fonts.get(font), fresh, 72.0, Coverage::Sdf, 0);
        assert!(store
            .entry(fonts.get(font).uid().0, fresh, 72.0, Coverage::Sdf, 0)
            .is_some());

        // Release: the wanted bucket rasters on the next sighting.
        store.end_frame();
        store.set_text_raster_hold(false);
        store.ensure(fonts.get(font), glyph, 72.0, Coverage::Sdf, 0);
        assert!(store
            .entry(fonts.get(font).uid().0, glyph, 72.0, Coverage::Sdf, 0)
            .is_some());
    }

    /// The mask tier re-rasters per 1/200 zoom step — under a hold, those
    /// misses reuse the nearest resident SCALE of the glyph instead
    /// (continuous px, so the stand-in comes from the size index, not
    /// fixed buckets).
    #[test]
    fn text_raster_hold_covers_bitmap_scales() {
        let Some((device, queue)) = headless() else {
            eprintln!("SKIP text_raster_hold_covers_bitmap_scales: no GPU adapter");
            return;
        };
        let fonts = fira();
        let mut store = GlyphStore::new(&device, &queue);
        let font = fonts.family("Fira Sans").unwrap();
        let glyph = fonts.get(font).glyph_for('H').unwrap();

        // Warm one quantized scale, then crawl one step under a hold.
        store.ensure(fonts.get(font), glyph, 11.83, Coverage::Fill, 0);
        store.end_frame();
        store.set_text_raster_hold(true);

        store.ensure(fonts.get(font), glyph, 11.96, Coverage::Fill, 0);
        assert!(
            store
                .entry(fonts.get(font).uid().0, glyph, 11.96, Coverage::Fill, 0)
                .is_none(),
            "held: the fresh scale was not rasterized"
        );
        let (px, ..) = store
            .resident_stand_in(fonts.get(font).uid().0, glyph, Coverage::Fill, 11.96)
            .expect("the previous scale stands in");
        assert_eq!(px, 11.83);
        assert_eq!(store.frame_counters().2, 1);

        store.end_frame();
        store.set_text_raster_hold(false);
        store.ensure(fonts.get(font), glyph, 11.96, Coverage::Fill, 0);
        assert!(store
            .entry(fonts.get(font).uid().0, glyph, 11.96, Coverage::Fill, 0)
            .is_some());
    }

    /// The B1 invariant: after `ensure_run`, EVERY glyph of the run is
    /// resident simultaneously — even when packing the run forced a GC
    /// mid-way (which used to leave earlier page references stale).
    #[test]
    fn ensure_run_survives_a_mid_run_gc() {
        let Some((device, queue)) = headless() else {
            eprintln!("SKIP ensure_run_survives_a_mid_run_gc: no GPU adapter");
            return;
        };
        let fonts = fira();
        let mut store = GlyphStore::with_page_size(&device, &queue, 64);
        let font = fonts.family("Fira Sans").unwrap();

        // 26 one-per-page glyphs against a 4-page budget: GC pressure is
        // guaranteed, wherever exactly the collections land.
        let glyph = |ch: char| fonts.get(font).glyph_for(ch).unwrap();
        for ch in 'a'..='z' {
            store.ensure(fonts.get(font), glyph(ch), 52.0, Coverage::Fill, 0);
        }
        assert!(store.generation >= 1, "junk fill forced GCs");

        // The postcondition batching relies on: after ensure_run, EVERY key
        // of a run that fits the atlas is resident simultaneously, on live
        // pages — regardless of how many GCs the run itself triggered.
        let keys: Vec<(u32, u8)> = ['M', 'N', 'H'].map(|ch| (glyph(ch), 0)).into();
        store.ensure_run(fonts.get(font), 26.0, Coverage::Fill, &keys);
        let pages = store.pages(false).len();
        for &(g, phase) in &keys {
            let (page, _) = store
                .entry(fonts.get(font).uid().0, g, 26.0, Coverage::Fill, phase)
                .expect("resident after ensure_run");
            assert!(page.index < pages, "page reference is live");
        }
    }

    /// The stroke is part of the address. Same font, glyph and size, three
    /// different coverages — three entries, three distinct rasters. Without
    /// the stroke in the key a stroked run would silently draw the filled
    /// mask that got there first.
    #[test]
    fn the_stroke_is_part_of_the_atlas_key() {
        let Some((device, queue)) = headless() else {
            eprintln!("SKIP the_stroke_is_part_of_the_atlas_key: no GPU adapter");
            return;
        };
        let fonts = fira();
        let mut store = GlyphStore::new(&device, &queue);
        let font = fonts.family("Fira Sans").unwrap();
        let glyph = fonts.get(font).glyph_for('M').unwrap();
        let stroke = |width: f32| {
            Coverage::Stroke(GlyphStroke {
                width,
                cap: Cap::Butt,
                join: Join::Miter,
                miter_limit: 4.0,
            })
        };

        let coverages = [Coverage::Fill, stroke(2.0), stroke(6.0)];
        for coverage in coverages {
            store.ensure(fonts.get(font), glyph, 48.0, coverage, 0);
        }
        assert_eq!(store.frame_counters().0, 3, "one raster per coverage");

        let cells: Vec<[f32; 4]> = coverages
            .iter()
            .map(|&coverage| {
                let (_, entry) = store
                    .entry(fonts.get(font).uid().0, glyph, 48.0, coverage, 0)
                    .expect("resident");
                [entry.left, entry.top, entry.width, entry.height]
            })
            .collect();
        for (wider, narrower) in [(cells[1], cells[0]), (cells[2], cells[1])] {
            assert!(
                wider[2] > narrower[2] && wider[3] > narrower[3],
                "a wider stroke needs a bigger cell: {wider:?} vs {narrower:?}"
            );
        }

        // Re-ensuring the same coverages hits the cache — no fresh rasters.
        store.end_frame();
        for coverage in coverages {
            store.ensure(fonts.get(font), glyph, 48.0, coverage, 0);
        }
        assert_eq!(store.frame_counters().0, 0, "all three were cache hits");
    }

    /// The pathological case: a run larger than the WHOLE atlas degrades to
    /// dropped glyphs — but never to a stale page reference.
    #[test]
    fn oversized_run_degrades_without_stale_pages() {
        let Some((device, queue)) = headless() else {
            eprintln!("SKIP oversized_run_degrades_without_stale_pages: no GPU adapter");
            return;
        };
        let fonts = fira();
        let mut store = GlyphStore::with_page_size(&device, &queue, 64);
        let font = fonts.family("Fira Sans").unwrap();

        let keys: Vec<(u32, u8)> = ('A'..='Z')
            .chain('a'..='z')
            .chain('0'..='9')
            .filter_map(|ch| fonts.get(font).glyph_for(ch))
            .map(|g| (g, 0))
            .collect();
        store.ensure_run(fonts.get(font), 30.0, Coverage::Fill, &keys);

        let pages = store.pages(false).len();
        let resident = keys
            .iter()
            .filter_map(|&(g, phase)| {
                store.entry(fonts.get(font).uid().0, g, 30.0, Coverage::Fill, phase)
            })
            .inspect(|(page, _)| assert!(page.index < pages, "live page"))
            .count();
        assert!(resident > 0, "the surviving subset still renders");
    }
}
