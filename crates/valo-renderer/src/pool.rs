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
    /// Whether multisample scratch is marked transient
    /// ([`transient_scratch_helps`]).
    transient_scratch: bool,
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
/// Content renders into its multisample scratch, `msaa` and `depth` (4
/// samples, discarded after every pass), and resolves to `resolve`. `resolve_texture` is also the copy source when a destination
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

/// `Attachments` are multisample scratch: the 4-sample colour and depth a
/// target renders into, discarded after every pass. Taken alone, they serve
/// a target whose resolve texture lives elsewhere: the main target or a
/// raster-cache fill.
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
            transient_scratch: transient_scratch_helps(&device.adapter_info()),
            frame: 0,
            layers: Shelf::new(),
            copy_textures: Shelf::new(),
            filters: Shelf::new(),
            attachments: Shelf::new(),
        }
    }

    /// `take_layer` returns a pooled offscreen layer of `size` and `format`:
    /// multisample scratch and a sampleable resolve.
    ///
    /// The returned views must not be used after [`Self::end_frame`].
    pub fn take_layer(&mut self, size: [u32; 2], format: wgpu::TextureFormat) -> LayerTarget {
        let key = PoolKey { size, format };
        let (device, transient) = (&self.device, self.transient_scratch);
        self.layers.take(key, self.frame, || {
            new_layer(device, size, format, transient)
        })
    }

    /// `take_attachments` returns pooled multisample scratch of `size` and
    /// `format`, for a target whose resolve texture lives elsewhere: the main
    /// target or a raster-cache fill. It belongs to the caller alone until
    /// [`Self::end_frame`], so two targets open at once never share samples.
    /// Exact-size match.
    pub fn take_attachments(&mut self, size: [u32; 2], format: wgpu::TextureFormat) -> Attachments {
        let key = PoolKey { size, format };
        let (device, transient) = (&self.device, self.transient_scratch);
        self.attachments.take(key, self.frame, || {
            new_scratch(device, size, format, transient)
        })
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
            view: attachment_texture(device, size, format, 1, AttachmentRole::Sampled)
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

/// `transient_scratch_helps` decides whether multisample scratch is marked
/// `TRANSIENT_ATTACHMENT`, which lets it live only in tile memory (Apple's
/// memoryless storage, Vulkan's lazily allocated memory). The flag needs
/// `StoreOp::Discard`, which every pass uses.
fn transient_scratch_helps(adapter: &wgpu::AdapterInfo) -> bool {
    match adapter.transient_saves_memory {
        // A native adapter says whether the flag saves memory.
        Some(saves_memory) => saves_memory,
        // A browser can't say, and its WebGPU rejects any texture carrying a
        // usage it predates: the flag goes only where the browser knows it.
        // A WebGPU outside a browser, such as a WaOS program's, can't say
        // either, and has no browser to ask.
        None => {
            adapter.backend == wgpu::Backend::BrowserWebGpu && browser_knows_transient_attachments()
        }
    }
}

/// `browser_knows_transient_attachments` reports whether the browser's
/// WebGPU defines `GPUTextureUsage.TRANSIENT_ATTACHMENT`: how a page detects
/// a usage added to WebGPU after it shipped. Only the browser target has
/// JavaScript to ask; on any other, wasm32 under WASI included, it's false.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn browser_knows_transient_attachments() -> bool {
    let global = js_sys::global();
    js_sys::Reflect::get(&global, &"GPUTextureUsage".into())
        .ok()
        .filter(|usage| usage.is_object())
        .and_then(|usage| js_sys::Reflect::has(&usage, &"TRANSIENT_ATTACHMENT".into()).ok())
        .unwrap_or(false)
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn browser_knows_transient_attachments() -> bool {
    false
}

/// `AttachmentRole` is what a pooled render-attachment texture is for, which
/// decides its usages.
#[derive(Clone, Copy)]
enum AttachmentRole {
    /// Multisample colour or depth that every pass discards, marked
    /// transient where that helps.
    Scratch { transient: bool },
    /// A layer's resolve or a filter target: composited and copied from.
    Sampled,
}

impl AttachmentRole {
    /// `usage` is what a texture in this role is created with.
    fn usage(self) -> wgpu::TextureUsages {
        let role = match self {
            AttachmentRole::Scratch { transient: true } => {
                wgpu::TextureUsages::TRANSIENT_ATTACHMENT
            }
            AttachmentRole::Scratch { transient: false } => wgpu::TextureUsages::empty(),
            AttachmentRole::Sampled => {
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC
            }
        };
        wgpu::TextureUsages::RENDER_ATTACHMENT | role
    }
}

/// `new_layer` creates a layer: its multisample scratch and its sampleable
/// resolve.
fn new_layer(
    device: &wgpu::Device,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    transient_scratch: bool,
) -> LayerTarget {
    let scratch = new_scratch(device, size, format, transient_scratch);
    let resolve = attachment_texture(device, size, format, 1, AttachmentRole::Sampled);
    LayerTarget {
        msaa: scratch.msaa,
        resolve: resolve.create_view(&Default::default()),
        resolve_texture: resolve,
        depth: scratch.depth,
    }
}

/// `new_scratch` creates multisample scratch: a 4-sample colour and depth
/// pair that every pass discards.
fn new_scratch(
    device: &wgpu::Device,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    transient: bool,
) -> Attachments {
    let role = AttachmentRole::Scratch { transient };
    let msaa = attachment_texture(device, size, format, SAMPLE_COUNT, role);
    let depth = attachment_texture(device, size, DEPTH_FORMAT, SAMPLE_COUNT, role);
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

/// `attachment_texture` creates a render-attachment texture for `role`.
fn attachment_texture(
    device: &wgpu::Device,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    samples: u32,
    role: AttachmentRole,
) -> wgpu::Texture {
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
        usage: role.usage(),
        view_formats: &[],
    })
}

impl TargetPool {
    /// Pooled + taken targets and the main target's attachments. Bytes are
    /// descriptor estimates: MSAA attachments cost samples × bpp.
    pub(crate) fn report(&self) -> crate::PoolReport {
        // Copy textures, filter targets and a layer's resolve.
        const FLAT_BPP: u64 = 4;
        // Multisample scratch takes no memory where it's transient (tile
        // memory only), and 4 + 4 bytes per sample elsewhere.
        let scratch_bpp = if self.transient_scratch {
            0
        } else {
            u64::from(SAMPLE_COUNT) * (4 + 4)
        };
        let layer_bpp = FLAT_BPP + scratch_bpp;
        let mut count = 0u32;
        let mut bytes = 0u64;
        let mut add = |key: &PoolKey, bpp: u64| {
            count += 1;
            bytes += key.size[0] as u64 * key.size[1] as u64 * bpp;
        };
        self.layers.keys().for_each(|key| add(key, layer_bpp));
        self.copy_textures.keys().for_each(|key| add(key, FLAT_BPP));
        self.filters.keys().for_each(|key| add(key, FLAT_BPP));
        self.attachments
            .keys()
            .for_each(|key| add(key, scratch_bpp));
        crate::PoolReport { count, bytes }
    }
}
