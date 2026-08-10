//! The list raster cache: persistent textures for embedded
//! display lists the EMBEDDER hinted as cacheable (`draw_display_list_cached`
//! — frame/artboard boundaries in a design document). One entry per list
//! identity; a re-recorded list has a new id, so invalidation needs no code
//! at all — stale entries simply idle out.
//!
//! Policy lives with the embedder, admission here — and the split is
//! total: the embedder VOUCHES that a hinted list is stable
//! (it un-hints boards under live editing sessions, knowledge only it
//! has), so this cache carries no frame history at all — no warm-up, no
//! fill quota, no work debt. A hinted list fills on FIRST sight, in the
//! same frame that shows it; a fill costs about what the inline paint it
//! replaces would have (the same rasterization, redirected), and the
//! total per frame self-limits to roughly one viewport of texels because
//! the embedder's boards tile. State: entries, the gesture hold, and the
//! idle sweep every other valo cache uses. No LRU, no byte budget — the
//! tiling working set self-bounds; the MemoryReport line watches it.

use rustc_hash::FxHashMap;
use valo_dl::DisplayList;
use valo_geometry::Rect;

use crate::report::PoolReport;

/// Below this many draws a subtree is cheaper to replay than to raster:
/// the texture + quad would cost more GPU than the draws they replace,
/// and a board of small card-frames would pay a texture each. Economics
/// only — cached frames render byte-identically either way.
pub const MIN_CACHED_DRAWS: u32 = 16;
/// The serve band: a texture keeps serving while `needed/entry` sits
/// inside it. Lower edge = the safe minification limit (linear sampling
/// holds to half density — the line Impeller's blur downsampler also
/// draws; past it, refill SMALLER, cheaper and alias-free). Upper edge =
/// jitter tolerance (composed camera matrices carry float noise; a 2%
/// wiggle is not a zoom-in and must not trigger refill storms).
const SCALE_SERVE_BAND: std::ops::RangeInclusive<f32> = 0.5..=1.02;

/// What the planner should do with one hinted embed.
pub enum RasterVerdict {
    /// Replay inline, as an unhinted embed would.
    Inline,
    /// Sample the cached texture as one quad.
    Quad(QuadSource),
    /// Render the list into a fresh entry as a pass planned BEFORE the
    /// current segment, then sample it immediately — flutter's shape
    /// (evict → TryToRasterCache → paint-from-cache, one frame), riding
    /// the same order that lets every save-layer render-then-sample
    /// within a frame.
    Fill(FillTarget),
}

/// Everything the composite quad needs (clones are cheap — views are Arcs).
pub struct QuadSource {
    pub view: wgpu::TextureView,
    pub size: [u32; 2],
    /// Device px per list unit the texture was rendered at.
    pub content_scale: f32,
    /// List bounds at raster time (list-root space).
    pub content_bounds: Rect,
}

/// Everything a fill pass needs to render into the new entry.
pub struct FillTarget {
    pub view: wgpu::TextureView,
    pub texture: wgpu::Texture,
    pub size: [u32; 2],
    pub content_scale: f32,
    pub content_bounds: Rect,
}

impl FillTarget {
    /// The same entry, seen from the sampling side — the quad that draws
    /// in the very frame that fills.
    pub fn quad_source(&self) -> QuadSource {
        QuadSource {
            view: self.view.clone(),
            size: self.size,
            content_scale: self.content_scale,
            content_bounds: self.content_bounds,
        }
    }
}

struct Entry {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    content_scale: f32,
    content_bounds: Rect,
    bytes: u64,
    last_used: u64,
}

pub struct ListRasterCache {
    entries: FxHashMap<u64, Entry>,
    /// While a camera gesture is in flight the host prefers reuse over
    /// refills: any existing raster serves at any scale ratio; the frame
    /// the host renders on settle refills every stale board at once.
    hold: bool,
    frame: u64,
}

impl ListRasterCache {
    pub fn new() -> Self {
        Self {
            entries: FxHashMap::default(),
            hold: false,
            frame: 0,
        }
    }

    pub fn set_hold(&mut self, held: bool) {
        self.hold = held;
    }

    /// The verdict for one hinted embed needing `needed_scale` device px
    /// per list unit. `max_dimension` is the device's texture limit.
    pub fn resolve(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        list: &DisplayList,
        needed_scale: f32,
        max_dimension: u32,
    ) -> RasterVerdict {
        let Some(bounds) = cacheable_bounds(list) else {
            return RasterVerdict::Inline;
        };
        if let Some(entry) = self.entries.get_mut(&list.id()) {
            // The hold serves at ANY ratio (gesture frames prefer reuse);
            // the settle frame falls through and refills, crisp.
            if self.hold || scale_serves(entry.content_scale, needed_scale) {
                entry.last_used = self.frame;
                return RasterVerdict::Quad(quad_source(entry));
            }
        }
        let Some(size) = raster_size(&bounds, needed_scale, max_dimension) else {
            return RasterVerdict::Inline; // too big for one texture: replay
        };
        let entry = create_entry(device, format, size, needed_scale, bounds, self.frame);
        let fill = fill_target(&entry, size);
        self.entries.insert(list.id(), entry);
        RasterVerdict::Fill(fill)
    }

    /// flutter's liveness sweep: an entry lives exactly as long as it is
    /// used every frame. A culled or re-recorded board's texture dies with
    /// the frame that stopped using it — no grace period, because nothing
    /// in a canvas app vanishes for a frame and returns, and a comeback
    /// costs one fill ≈ the inline paint it replaces.
    pub fn end_frame(&mut self) {
        let current = self.frame;
        self.frame += 1;
        self.entries.retain(|_, entry| entry.last_used >= current);
    }

    pub fn report(&self) -> PoolReport {
        PoolReport {
            count: self.entries.len() as u32,
            bytes: self.entries.values().map(|e| e.bytes).sum(),
        }
    }
}

fn scale_serves(entry_scale: f32, needed_scale: f32) -> bool {
    SCALE_SERVE_BAND.contains(&(needed_scale / entry_scale.max(1e-6)))
}

/// Admission that reads only the list: enough draws to beat a quad, no
/// backdrop reads (a backdrop samples what is BEHIND the list — flutter
/// excludes backdrop filters from its cache for the same reason), and
/// finite non-empty bounds.
fn cacheable_bounds(list: &DisplayList) -> Option<Rect> {
    if list.draw_count() < MIN_CACHED_DRAWS {
        return None;
    }
    if list.backdrop_group_count() > 0 {
        return None;
    }
    let bounds = list.bounds()?;
    (bounds.width > 0.0 && bounds.height > 0.0 && bounds.width.is_finite()).then_some(bounds)
}

fn raster_size(bounds: &Rect, scale: f32, max_dimension: u32) -> Option<[u32; 2]> {
    let w = (bounds.width * scale).ceil() as u32;
    let h = (bounds.height * scale).ceil() as u32;
    (w > 0 && h > 0 && w <= max_dimension && h <= max_dimension).then_some([w, h])
}

fn quad_source(entry: &Entry) -> QuadSource {
    QuadSource {
        view: entry.view.clone(),
        size: [entry.texture.width(), entry.texture.height()],
        content_scale: entry.content_scale,
        content_bounds: entry.content_bounds,
    }
}

fn fill_target(entry: &Entry, size: [u32; 2]) -> FillTarget {
    FillTarget {
        view: entry.view.clone(),
        texture: entry.texture.clone(),
        size,
        content_scale: entry.content_scale,
        content_bounds: entry.content_bounds,
    }
}

fn create_entry(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    size: [u32; 2],
    content_scale: f32,
    content_bounds: Rect,
    frame: u64,
) -> Entry {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("valo raster cache"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let bytes = size[0] as u64 * size[1] as u64 * 4;
    Entry {
        texture,
        view,
        content_scale,
        content_bounds,
        bytes,
        last_used: frame,
    }
}
