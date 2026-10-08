use crate::frame::ContextAttachments;
use crate::pipelines::{DEPTH_FORMAT, SAMPLE_COUNT};

/// `TargetPool` reuses offscreen textures across frames.
///
/// Layer, copy, filter, and attachment textures are taken during planning,
/// stay alive through GPU submission, and return at [`Self::end_frame`], so
/// every take this frame is a texture no other take shares. Entries unused for
/// several frames are dropped. No multisample attachment carries a picture
/// from one frame to the next: a target that keeps its pixels has them drawn
/// back each frame instead.
///
/// Views returned by `take_*` are cloned wgpu handles. Do not keep them past
/// [`Self::end_frame`]: the pool may reuse or drop the underlying textures.
pub struct TargetPool {
    device: wgpu::Device,
    frame: u64,
    layers: Shelf<LayerTarget>,
    copy_textures: Shelf<CopyTexture>,
    filters: Shelf<FilterTarget>,
    attachments: Shelf<Attachments>,
}

const EVICT_AFTER_FRAMES: u64 = 3;

/// `PoolKey` is what a pooled texture is matched by: exactly its size and
/// its format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PoolKey {
    size: [u32; 2],
    format: wgpu::TextureFormat,
}

/// `Shelf` is one kind of pooled texture: the entries free to take, and the
/// ones taken this frame, which [`Shelf::end_frame`] puts back.
struct Shelf<T> {
    free: Vec<Pooled<T>>,
    taken: Vec<Pooled<T>>,
}

struct Pooled<T> {
    key: PoolKey,
    last_used: u64,
    value: T,
}

impl<T: Clone> Shelf<T> {
    fn new() -> Self {
        Self {
            free: Vec::new(),
            taken: Vec::new(),
        }
    }

    /// `take` is a free entry matching `key`, or a new one `create` makes,
    /// held until the frame ends.
    fn take(&mut self, key: PoolKey, frame: u64, create: impl FnOnce() -> T) -> T {
        let mut entry = match self.free.iter().position(|entry| entry.key == key) {
            Some(index) => self.free.swap_remove(index),
            None => Pooled {
                key,
                last_used: frame,
                value: create(),
            },
        };
        entry.last_used = frame;
        let value = entry.value.clone();
        self.taken.push(entry);
        value
    }

    /// `end_frame` puts this frame's takes back and drops the entries unused
    /// since `cutoff`.
    fn end_frame(&mut self, cutoff: u64) {
        self.free.append(&mut self.taken);
        self.free.retain(|entry| entry.last_used >= cutoff);
    }

    /// `keys` are every entry's key, free or taken.
    fn keys(&self) -> impl Iterator<Item = &PoolKey> {
        self.free.iter().chain(&self.taken).map(|entry| &entry.key)
    }
}

/// `LayerTarget` is one offscreen layer's attachments.
///
/// Content renders into `msaa` (4 samples, tile-only) and resolves to
/// `resolve`. `resolve_texture` is also the copy source when a destination
/// read or a backdrop inside the layer copies what it holds.
#[derive(Clone)]
pub struct LayerTarget {
    pub msaa: wgpu::TextureView,
    pub resolve_texture: wgpu::Texture,
    pub resolve: wgpu::TextureView,
    pub depth: wgpu::TextureView,
}

/// `CopyTexture` is a texture the size of a target that the target is
/// copied into when it is split: drawn back by its next pass, and read by a
/// destination read or a backdrop.
///
/// `view` is sampleable; `texture` is the copy destination.
#[derive(Clone)]
pub struct CopyTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

/// `Attachments` are the tile-only MSAA color and depth attachments of a
/// target whose resolve texture lives elsewhere: the main target's or a
/// raster-cache fill's.
#[derive(Clone)]
pub struct Attachments {
    pub msaa: wgpu::TextureView,
    pub depth: wgpu::TextureView,
}

impl Attachments {
    /// `resolving_into` is these attachments with their colour resolving
    /// into `resolve`.
    pub(crate) fn resolving_into(self, resolve: wgpu::TextureView) -> ContextAttachments {
        ContextAttachments {
            msaa: self.msaa,
            depth: self.depth,
            resolve,
        }
    }
}

/// `FilterTarget` is a single-sample color target for a gaussian filter pass.
///
/// It has no depth buffer. After the pass it is sampled by the next pass or
/// the composite.
#[derive(Clone)]
pub struct FilterTarget {
    pub view: wgpu::TextureView,
}

impl TargetPool {
    /// `new` creates an empty pool for `device`.
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            device: device.clone(),
            frame: 0,
            layers: Shelf::new(),
            copy_textures: Shelf::new(),
            filters: Shelf::new(),
            attachments: Shelf::new(),
        }
    }

    /// `take_layer` returns a pooled offscreen layer of `size` and `format`,
    /// its multisample attachments tile-only: every pass discards them.
    ///
    /// The returned views must not be used after [`Self::end_frame`].
    pub fn take_layer(&mut self, size: [u32; 2], format: wgpu::TextureFormat) -> LayerTarget {
        let key = PoolKey { size, format };
        let device = &self.device;
        self.layers
            .take(key, self.frame, || new_layer(device, size, format))
    }

    /// `take_attachments` returns pooled tile-only MSAA color and depth of
    /// `size` and `format`, for a target whose resolve texture lives
    /// elsewhere: the main target's and a raster-cache fill's. Every pass
    /// discards them. They belong to the caller alone until
    /// [`Self::end_frame`], so two targets open at once never share samples.
    /// Exact-size match.
    pub fn take_attachments(&mut self, size: [u32; 2], format: wgpu::TextureFormat) -> Attachments {
        let key = PoolKey { size, format };
        let device = &self.device;
        self.attachments
            .take(key, self.frame, || new_attachments(device, size, format))
    }

    /// `take_copy_texture` returns a pooled copy texture of `size` and
    /// `format`.
    ///
    /// The returned views must not be used after [`Self::end_frame`].
    pub fn take_copy_texture(
        &mut self,
        size: [u32; 2],
        format: wgpu::TextureFormat,
    ) -> CopyTexture {
        let key = PoolKey { size, format };
        let device = &self.device;
        self.copy_textures
            .take(key, self.frame, || new_copy_texture(device, size, format))
    }

    /// `take_filter` returns a pooled single-sample filter target of `size` and `format`.
    ///
    /// Exact-size match: a filter stage's texture is exactly what it holds.
    /// The returned view must not be used after [`Self::end_frame`].
    pub fn take_filter(&mut self, size: [u32; 2], format: wgpu::TextureFormat) -> FilterTarget {
        let key = PoolKey { size, format };
        let device = &self.device;
        self.filters.take(key, self.frame, || FilterTarget {
            view: attachment_texture(device, size, format, 1, true, false)
                .create_view(&Default::default()),
        })
    }

    /// `end_frame` returns this frame's takes to the pool and drops idle entries.
    pub fn end_frame(&mut self) {
        self.frame += 1;
        let cutoff = self.frame.saturating_sub(EVICT_AFTER_FRAMES);
        self.layers.end_frame(cutoff);
        self.copy_textures.end_frame(cutoff);
        self.filters.end_frame(cutoff);
        self.attachments.end_frame(cutoff);
    }
}

/// `new_layer` creates a layer's tile-only 4-sample colour and depth and its
/// sampleable resolve.
fn new_layer(device: &wgpu::Device, size: [u32; 2], format: wgpu::TextureFormat) -> LayerTarget {
    let msaa = attachment_texture(device, size, format, SAMPLE_COUNT, false, true);
    let resolve = attachment_texture(device, size, format, 1, true, false);
    let depth = attachment_texture(device, size, DEPTH_FORMAT, SAMPLE_COUNT, false, true);
    LayerTarget {
        msaa: msaa.create_view(&Default::default()),
        resolve: resolve.create_view(&Default::default()),
        resolve_texture: resolve,
        depth: depth.create_view(&Default::default()),
    }
}

/// `new_attachments` creates a tile-only 4-sample color and depth pair.
fn new_attachments(
    device: &wgpu::Device,
    size: [u32; 2],
    format: wgpu::TextureFormat,
) -> Attachments {
    let msaa = attachment_texture(device, size, format, SAMPLE_COUNT, false, true);
    let depth = attachment_texture(device, size, DEPTH_FORMAT, SAMPLE_COUNT, false, true);
    Attachments {
        msaa: msaa.create_view(&Default::default()),
        depth: depth.create_view(&Default::default()),
    }
}

/// `new_copy_texture` creates a sampleable texture regions are copied into.
fn new_copy_texture(
    device: &wgpu::Device,
    size: [u32; 2],
    format: wgpu::TextureFormat,
) -> CopyTexture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("valo.copy"),
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
    CopyTexture {
        view: texture.create_view(&Default::default()),
        texture,
    }
}

/// A render-attachment texture; `sampleable + copyable` adds the usages a
/// layer's resolve target needs (composited from, copied from).
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
        // StoreOp::Discard, which every pass uses.
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
    /// Pooled + taken targets and the main target's attachments. Bytes are
    /// descriptor estimates: MSAA attachments cost samples × bpp.
    pub(crate) fn report(&self) -> crate::PoolReport {
        // A layer's 4-sample colour and depth are tile-only, so only its
        // 1-sample resolve counts.
        const LAYER_BPP: u64 = 4;
        const FLAT_BPP: u64 = 4; // copy textures + filter targets
                                 // Attachments carry `TRANSIENT_ATTACHMENT`: tile-only on hardware
                                 // that supports it, they exist as objects but occupy no memory.
        const ATTACHMENTS_BPP: u64 = 0;
        let mut count = 0u32;
        let mut bytes = 0u64;
        let mut add = |key: &PoolKey, bpp: u64| {
            count += 1;
            bytes += key.size[0] as u64 * key.size[1] as u64 * bpp;
        };
        self.layers.keys().for_each(|key| add(key, LAYER_BPP));
        self.copy_textures.keys().for_each(|key| add(key, FLAT_BPP));
        self.filters.keys().for_each(|key| add(key, FLAT_BPP));
        self.attachments
            .keys()
            .for_each(|key| add(key, ATTACHMENTS_BPP));
        crate::PoolReport { count, bytes }
    }
}
