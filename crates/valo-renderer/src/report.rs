//! On-demand resource accounting — Skia's two-tier model
//! (`getResourceCacheUsage` totals cheap and always available; the per-pool
//! breakdown here is the `SkTraceMemoryDump` analog). Numbers are computed
//! from descriptors, not queried from the driver; `wgpu` gives the driver's
//! own view when the `counters` feature is on.

/// One cache/pool: live entries + estimated GPU/heap bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct PoolReport {
    pub count: u32,
    pub bytes: u64,
}

/// One glyph-atlas family (mask/SDF share R8 pages; emoji use RGBA pages).
#[derive(Clone, Copy, Debug, Default)]
pub struct AtlasReport {
    pub pages: u32,
    pub bytes: u64,
    /// Resident rasterized glyphs (whitespace placeholders excluded).
    pub entries: u32,
}

/// wgpu's internal live-object counters — the driver-side ground truth the
/// leak test cross-checks valo's own accounting against. All zeros unless
/// built with the `counters` feature (wgpu compiles them out otherwise).
#[derive(Clone, Copy, Debug, Default)]
pub struct WgpuCounters {
    pub enabled: bool,
    pub buffers: i64,
    pub textures: i64,
    pub bind_groups: i64,
    pub buffer_memory: i64,
    pub texture_memory: i64,
}

/// Everything valo holds between frames, one line per subsystem.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryReport {
    /// Live uploaded images (deduped; bytes include the mip chain).
    pub images: PoolReport,
    /// Glyph atlas families: [mask/SDF, color].
    pub atlas: [AtlasReport; 2],
    /// Pooled layer/snapshot/filter targets + main scratch.
    pub targets: PoolReport,
    /// Transient-arena blocks (×3 frame ring), scratch bytes.
    pub host_buffer: PoolReport,
    /// Flattened-contour cache entries + point bytes.
    pub contours: PoolReport,
    /// Outline-tier glyph path cache.
    pub glyph_paths: PoolReport,
    /// Baked >8-stop gradient ramp textures (Impeller's texture path).
    pub ramps: PoolReport,
    /// Persistent raster-cache textures for hinted embedded lists — the
    /// frame-boundary cache.
    pub raster_cache: PoolReport,
    pub wgpu: WgpuCounters,
}

impl MemoryReport {
    /// Sum of valo's own estimates (excludes the wgpu counters).
    pub fn total_bytes(&self) -> u64 {
        self.images.bytes
            + self.atlas.iter().map(|a| a.bytes).sum::<u64>()
            + self.targets.bytes
            + self.raster_cache.bytes
            + self.host_buffer.bytes
            + self.contours.bytes
            + self.glyph_paths.bytes
    }
}

pub(crate) fn wgpu_counters(device: &wgpu::Device) -> WgpuCounters {
    let hal = device.get_internal_counters().hal;
    WgpuCounters {
        enabled: cfg!(feature = "counters"),
        buffers: hal.buffers.read() as i64,
        textures: hal.textures.read() as i64,
        bind_groups: hal.bind_groups.read() as i64,
        buffer_memory: hal.buffer_memory.read() as i64,
        texture_memory: hal.texture_memory.read() as i64,
    }
}
