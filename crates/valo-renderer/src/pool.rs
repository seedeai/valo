use std::collections::HashMap;

use crate::pipelines::{DEPTH_FORMAT, SAMPLE_COUNT};

/// `TargetPool` reuses offscreen textures across frames.
///
/// Layer, snapshot, filter, and raster-attachment targets are taken during
/// planning, stay alive through GPU submission, and return at [`Self::end_frame`].
/// Entries unused for several frames are dropped. Main-target MSAA color and
/// depth scratch is keyed by size and kept.
///
/// Views returned by `take_*` are cloned wgpu handles. Do not keep them past
/// [`Self::end_frame`]: the pool may reuse or drop the underlying textures.
pub struct TargetPool {
    device: wgpu::Device,
    frame: u64,
    /// Available pooled entries, exact-size matched.
    layers: Vec<Pooled<LayerTarget>>,
    snapshots: Vec<Pooled<Snapshot>>,
    filters: Vec<Pooled<FilterTarget>>,
    raster_attachments: Vec<Pooled<RasterAttachments>>,
    /// Taken this frame; reclaimed by `end_frame`.
    taken_layers: Vec<Pooled<LayerTarget>>,
    taken_snapshots: Vec<Pooled<Snapshot>>,
    taken_filters: Vec<Pooled<FilterTarget>>,
    taken_raster_attachments: Vec<Pooled<RasterAttachments>>,
    main_scratch: HashMap<(u32, u32, wgpu::TextureFormat, bool), MainScratch>,
}

/// `FILTER_SIZE_BUCKET` is the size quantum, in pixels, for pooled filter targets.
///
/// Blur-chain sizes snap up to this so consecutive frames and the horizontal
/// and vertical passes of one blur share textures.
pub const FILTER_SIZE_BUCKET: u32 = 32;

const EVICT_AFTER_FRAMES: u64 = 3;

struct Pooled<T> {
    size: [u32; 2],
    format: wgpu::TextureFormat,
    /// Attachments carry [`wgpu::TextureUsages::TRANSIENT`] — tile-only on
    /// hardware that supports it, so they cost no system memory.
    transient: bool,
    last_used: u64,
    value: T,
}

/// `LayerTarget` is one offscreen layer's attachments.
///
/// Content renders into `msaa` (4 samples) and resolves to `resolve`.
/// `resolve_texture` is also the copy source when a snapshot is taken inside
/// the layer.
#[derive(Clone)]
pub struct LayerTarget {
    pub msaa: wgpu::TextureView,
    pub resolve_texture: wgpu::Texture,
    pub resolve: wgpu::TextureView,
    pub depth: wgpu::TextureView,
}

/// `Snapshot` is a copy of a destination region for advanced blends.
///
/// `view` is sampleable; `texture` is the copy destination.
#[derive(Clone)]
pub struct Snapshot {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

/// `RasterAttachments` are the transient MSAA color and depth attachments
/// for a raster-cache fill.
///
/// The resolve target is the cache's own persistent texture, so it is not
/// pooled here. Both attachments are tile-only on hardware that supports it.
#[derive(Clone)]
pub struct RasterAttachments {
    pub msaa: wgpu::TextureView,
    pub depth: wgpu::TextureView,
}

/// `FilterTarget` is a single-sample color target for a gaussian filter pass.
///
/// It has no depth buffer. After the pass it is sampled by the next pass or
/// the composite.
#[derive(Clone)]
pub struct FilterTarget {
    pub view: wgpu::TextureView,
}

/// `MainScratch` is the MSAA color and depth scratch for the frame's main target.
///
/// The caller owns the resolve texture; this pool only provides the 4-sample
/// attachments around it.
#[derive(Clone)]
pub struct MainScratch {
    pub msaa: wgpu::TextureView,
    pub depth: wgpu::TextureView,
}

impl TargetPool {
    /// `new` creates an empty pool for `device`.
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            device: device.clone(),
            frame: 0,
            layers: Vec::new(),
            snapshots: Vec::new(),
            filters: Vec::new(),
            raster_attachments: Vec::new(),
            taken_layers: Vec::new(),
            taken_snapshots: Vec::new(),
            taken_filters: Vec::new(),
            taken_raster_attachments: Vec::new(),
            main_scratch: HashMap::new(),
        }
    }

    /// `take_layer` returns a pooled offscreen layer of `size` and `format`.
    ///
    /// `transient` is true when every segment of the target discards at pass
    /// end. The returned views must not be used after [`Self::end_frame`].
    pub fn take_layer(
        &mut self,
        size: [u32; 2],
        format: wgpu::TextureFormat,
        transient: bool,
    ) -> LayerTarget {
        let entry = take_matching(&mut self.layers, size, format, transient)
            .unwrap_or_else(|| self.create_layer(size, format, transient));
        let value = entry.value.clone();
        self.taken_layers.push(refreshed(entry, self.frame));
        value
    }

    /// `take_raster_attachments` returns pooled MSAA color and depth for a
    /// raster-cache fill of `size` and `format`.
    ///
    /// The resolve target is the cache's own persistent texture, so only the
    /// transient attachments pool here. Exact-size match. The returned views
    /// must not be used after [`Self::end_frame`].
    pub fn take_raster_attachments(
        &mut self,
        size: [u32; 2],
        format: wgpu::TextureFormat,
    ) -> RasterAttachments {
        let entry = take_matching(&mut self.raster_attachments, size, format, true)
            .unwrap_or_else(|| self.create_raster_attachments(size, format));
        let value = entry.value.clone();
        self.taken_raster_attachments
            .push(refreshed(entry, self.frame));
        value
    }

    /// `take_snapshot` returns a pooled destination copy of `size` and `format`.
    ///
    /// The returned views must not be used after [`Self::end_frame`].
    pub fn take_snapshot(&mut self, size: [u32; 2], format: wgpu::TextureFormat) -> Snapshot {
        let entry = take_matching(&mut self.snapshots, size, format, false)
            .unwrap_or_else(|| self.create_snapshot(size, format));
        let value = entry.value.clone();
        self.taken_snapshots.push(refreshed(entry, self.frame));
        value
    }

    /// `take_filter` returns a pooled single-sample filter target of `size` and `format`.
    ///
    /// `size` should already be snapped to `FILTER_SIZE_BUCKET`. The returned
    /// view must not be used after [`Self::end_frame`].
    pub fn take_filter(&mut self, size: [u32; 2], format: wgpu::TextureFormat) -> FilterTarget {
        let entry = take_matching(&mut self.filters, size, format, false)
            .unwrap_or_else(|| self.create_filter(size, format));
        let value = entry.value.clone();
        self.taken_filters.push(refreshed(entry, self.frame));
        value
    }

    /// `main_scratch` returns MSAA color and depth for the frame's main target.
    ///
    /// `transient` is true when every segment discards at pass end (a
    /// single-segment frame). The swap to a persistent pair on the first
    /// resume also comes through here. Scratch is keyed by size and kept.
    pub fn main_scratch(
        &mut self,
        size: [u32; 2],
        format: wgpu::TextureFormat,
        transient: bool,
    ) -> MainScratch {
        if self.main_scratch.len() > 8 {
            self.main_scratch.clear(); // a handful of live sizes; reset is fine
        }
        let device = self.device.clone();
        self.main_scratch
            .entry((size[0], size[1], format, transient))
            .or_insert_with(|| MainScratch {
                msaa: attachment_texture(&device, size, format, SAMPLE_COUNT, false, transient)
                    .create_view(&Default::default()),
                depth: attachment_texture(
                    &device,
                    size,
                    DEPTH_FORMAT,
                    SAMPLE_COUNT,
                    false,
                    transient,
                )
                .create_view(&Default::default()),
            })
            .clone()
    }

    /// `end_frame` returns this frame's takes to the pool and drops idle entries.
    pub fn end_frame(&mut self) {
        self.frame += 1;
        let cutoff = self.frame.saturating_sub(EVICT_AFTER_FRAMES);
        self.layers.append(&mut self.taken_layers);
        self.snapshots.append(&mut self.taken_snapshots);
        self.filters.append(&mut self.taken_filters);
        self.raster_attachments
            .append(&mut self.taken_raster_attachments);
        self.layers.retain(|e| e.last_used >= cutoff);
        self.snapshots.retain(|e| e.last_used >= cutoff);
        self.filters.retain(|e| e.last_used >= cutoff);
        self.raster_attachments.retain(|e| e.last_used >= cutoff);
    }

    fn create_layer(
        &self,
        size: [u32; 2],
        format: wgpu::TextureFormat,
        transient: bool,
    ) -> Pooled<LayerTarget> {
        let msaa = attachment_texture(&self.device, size, format, SAMPLE_COUNT, false, transient);
        let resolve = attachment_texture(&self.device, size, format, 1, true, false);
        let depth = attachment_texture(
            &self.device,
            size,
            DEPTH_FORMAT,
            SAMPLE_COUNT,
            false,
            transient,
        );
        Pooled {
            size,
            format,
            transient,
            last_used: self.frame,
            value: LayerTarget {
                msaa: msaa.create_view(&Default::default()),
                resolve: resolve.create_view(&Default::default()),
                resolve_texture: resolve,
                depth: depth.create_view(&Default::default()),
            },
        }
    }

    fn create_raster_attachments(
        &self,
        size: [u32; 2],
        format: wgpu::TextureFormat,
    ) -> Pooled<RasterAttachments> {
        let msaa = attachment_texture(&self.device, size, format, SAMPLE_COUNT, false, true);
        let depth = attachment_texture(&self.device, size, DEPTH_FORMAT, SAMPLE_COUNT, false, true);
        Pooled {
            size,
            format,
            transient: true,
            last_used: self.frame,
            value: RasterAttachments {
                msaa: msaa.create_view(&Default::default()),
                depth: depth.create_view(&Default::default()),
            },
        }
    }

    fn create_filter(&self, size: [u32; 2], format: wgpu::TextureFormat) -> Pooled<FilterTarget> {
        let texture = attachment_texture(&self.device, size, format, 1, true, false);
        Pooled {
            size,
            format,
            transient: false,
            last_used: self.frame,
            value: FilterTarget {
                view: texture.create_view(&Default::default()),
            },
        }
    }

    fn create_snapshot(&self, size: [u32; 2], format: wgpu::TextureFormat) -> Pooled<Snapshot> {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("valo.snapshot"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        Pooled {
            size,
            format,
            transient: false,
            last_used: self.frame,
            value: Snapshot {
                view: texture.create_view(&Default::default()),
                texture,
            },
        }
    }
}

fn take_matching<T>(
    pool: &mut Vec<Pooled<T>>,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    transient: bool,
) -> Option<Pooled<T>> {
    let idx = pool
        .iter()
        .position(|e| e.size == size && e.format == format && e.transient == transient)?;
    Some(pool.swap_remove(idx))
}

fn refreshed<T>(mut entry: Pooled<T>, frame: u64) -> Pooled<T> {
    entry.last_used = frame;
    entry
}

/// A render-attachment texture; `sampleable + copyable` adds the usages a
/// layer's resolve target needs (composited from, snapshotted from).
fn attachment_texture(
    device: &wgpu::Device,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    samples: u32,
    sampleable: bool,
    transient: bool,
) -> wgpu::Texture {
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
    if sampleable {
        usage |= wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC;
    }
    if transient {
        // Tile-only where hardware supports it (Apple: MTLStorageMode
        // Memoryless — zero bytes of system memory); the web backend
        // strips the bit, other backends treat it as a hint. Requires
        // StoreOp::Discard, which single-segment targets already use.
        debug_assert!(!sampleable, "transient attachments cannot be sampled");
        usage |= wgpu::TextureUsages::TRANSIENT_ATTACHMENT;
    }
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("valo.pooled"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

impl TargetPool {
    /// Pooled + taken targets and the persistent main scratch. Bytes are
    /// descriptor estimates: MSAA attachments cost samples × bpp.
    pub(crate) fn report(&self) -> crate::PoolReport {
        // 4-sample color + depth (16 + 16) plus a 1-sample resolve;
        // transient attachments are tile-only, so only the resolve counts.
        const LAYER_BPP: u64 = 36;
        const LAYER_TRANSIENT_BPP: u64 = 4;
        const SCRATCH_BPP: u64 = 32; // caller owns the resolve
        const FLAT_BPP: u64 = 4; // snapshots + filter targets
        let mut count = 0u32;
        let mut bytes = 0u64;
        let mut add = |size: [u32; 2], bpp: u64| {
            count += 1;
            bytes += size[0] as u64 * size[1] as u64 * bpp;
        };
        for t in self.layers.iter().chain(&self.taken_layers) {
            add(
                t.size,
                if t.transient {
                    LAYER_TRANSIENT_BPP
                } else {
                    LAYER_BPP
                },
            );
        }
        for t in self.snapshots.iter().chain(&self.taken_snapshots) {
            add(t.size, FLAT_BPP);
        }
        for t in self.filters.iter().chain(&self.taken_filters) {
            add(t.size, FLAT_BPP);
        }
        for &(w, h, _, transient) in self.main_scratch.keys() {
            // Transient pairs exist as objects but occupy no memory.
            add([w, h], if transient { 0 } else { SCRATCH_BPP });
        }
        crate::PoolReport { count, bytes }
    }
}
