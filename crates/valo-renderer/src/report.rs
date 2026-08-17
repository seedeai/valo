//! On-demand resource accounting — Skia's two-tier model
//! (`getResourceCacheUsage` totals cheap and always available; the per-pool
//! breakdown here is the `SkTraceMemoryDump` analog). Numbers are computed
//! from descriptors, not queried from the driver; `wgpu` gives the driver's
//! own view when the `counters` feature is on.

/// `PoolReport` summarizes one resource pool or cache.
#[derive(Clone, Copy, Debug, Default)]
pub struct PoolReport {
    /// `count` is the number of live entries.
    pub count: u32,
    /// `bytes` is their estimated memory usage.
    pub bytes: u64,
}

/// `AtlasReport` summarizes one glyph-atlas family.
#[derive(Clone, Copy, Debug, Default)]
pub struct AtlasReport {
    /// `pages` is the number of allocated atlas textures.
    pub pages: u32,
    /// `bytes` is their estimated GPU memory usage.
    pub bytes: u64,
    /// `entries` is the number of resident rasterized glyphs.
    ///
    /// Whitespace placeholders are excluded.
    pub entries: u32,
}

/// `WgpuCounters` reports wgpu's internal live-object accounting.
///
/// All values are zero unless the `counters` feature is enabled.
#[derive(Clone, Copy, Debug, Default)]
pub struct WgpuCounters {
    /// `enabled` indicates whether the `counters` feature is active.
    pub enabled: bool,
    /// `buffers` is the number of live wgpu buffers.
    pub buffers: i64,
    /// `textures` is the number of live wgpu textures.
    pub textures: i64,
    /// `bind_groups` is the number of live wgpu bind groups.
    pub bind_groups: i64,
    /// `buffer_memory` is wgpu's reported buffer memory in bytes.
    pub buffer_memory: i64,
    /// `texture_memory` is wgpu's reported texture memory in bytes.
    pub texture_memory: i64,
}

/// `MemoryReport` summarizes resources retained between frames.
///
/// Pool byte counts are estimates derived from resource descriptors. `wgpu`
/// contains separate internal counters when that feature is enabled.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryReport {
    /// `images` reports live uploaded images, including mip levels.
    pub images: PoolReport,
    /// `atlas` reports `[mask and SDF, color]` glyph-atlas families.
    pub atlas: [AtlasReport; 2],
    /// `targets` reports pooled layer, snapshot, filter, and scratch targets.
    pub targets: PoolReport,
    /// `host_buffer` reports transient upload-buffer blocks.
    pub host_buffer: PoolReport,
    /// `contours` reports cached flattened path contours.
    pub contours: PoolReport,
    /// `glyph_paths` reports cached vector glyph paths.
    pub glyph_paths: PoolReport,
    /// `ramps` reports cached gradient-ramp textures.
    pub ramps: PoolReport,
    /// `raster_cache` reports cached display-list textures.
    pub raster_cache: PoolReport,
    /// `wgpu` contains wgpu's internal object and memory counters.
    pub wgpu: WgpuCounters,
}

impl MemoryReport {
    /// `total_bytes` returns the sum of Valo's estimated retained memory.
    ///
    /// The separate wgpu counters are excluded.
    pub fn total_bytes(&self) -> u64 {
        self.images.bytes
            + self.atlas.iter().map(|a| a.bytes).sum::<u64>()
            + self.targets.bytes
            + self.raster_cache.bytes
            + self.host_buffer.bytes
            + self.contours.bytes
            + self.glyph_paths.bytes
            + self.ramps.bytes
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

#[cfg(test)]
mod tests {
    use super::{AtlasReport, MemoryReport, PoolReport, WgpuCounters};

    #[test]
    fn total_bytes_includes_every_valo_pool() {
        let report = MemoryReport {
            images: pool(1),
            atlas: [atlas(2), atlas(3)],
            targets: pool(4),
            host_buffer: pool(5),
            contours: pool(6),
            glyph_paths: pool(7),
            ramps: pool(8),
            raster_cache: pool(9),
            wgpu: WgpuCounters {
                buffer_memory: 1_000,
                texture_memory: 2_000,
                ..Default::default()
            },
        };

        assert_eq!(report.total_bytes(), 45);
    }

    fn pool(bytes: u64) -> PoolReport {
        PoolReport { count: 1, bytes }
    }

    fn atlas(bytes: u64) -> AtlasReport {
        AtlasReport {
            pages: 1,
            bytes,
            entries: 1,
        }
    }
}
