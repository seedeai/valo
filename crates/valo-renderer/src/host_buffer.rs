use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Bytes of one per-draw uniform record: mat4 MVP + vec4 color + the generic
/// payload (see shaders/solid.wgsl's layout contract). 512 = 2× the dynamic
/// offset alignment — stride and record size stay equal, nothing is wasted.
pub(crate) const UNIFORM_SIZE: u64 = 512;
/// Bytes of one blur kernel: Impeller's `KernelSamples` uniform block, one
/// vec4 per merged sample. It rides the same arena as the draw records, bound
/// at group 2 with a dynamic offset of its own.
pub(crate) const KERNEL_SIZE: u64 =
    (crate::planner::gaussian::MAX_KERNEL_SAMPLES * std::mem::size_of::<[f32; 4]>()) as u64;
/// Frames in flight the arena ring covers.
const FRAMES: usize = 3;
/// Ring passes a trailing block sits unused before draining (×3 frames each).
const IDLE_PASSES: u8 = 3;
/// Uniform slots per block (block size = slots × stride).
const SLOTS_PER_BLOCK: u64 = 1024;
/// Default vertex block size (fans allocate ranges; oversized meshes get a
/// dedicated block).
const VERTEX_BLOCK_SIZE: u64 = 256 * 1024;

/// `HostBuffer` bump-allocates per-draw uniforms and transient vertex data.
///
/// Each frame writes into CPU scratch; [`Self::flush`] moves the touched
/// blocks to the GPU the way the device's [`BlockWriter`] does it. A 3-frame
/// ring of persistent buffers means warm frames create nothing — the cost
/// that matters most on wasm.
///
/// Uniforms bind once per block via a dynamic offset: a draw's record at
/// group 0, a blur pass's kernel at group 2, both from the same blocks as
/// Impeller's `EmplaceUniform` takes both from one transient buffer. Vertex
/// data (stencil fans, stroke strips, glyph quads) lands in a second family
/// of blocks.
///
/// A frame is [`Self::begin_frame`], the allocations, [`Self::flush`], and,
/// once the frame is submitted, [`Self::after_submit`] with the receipt the
/// flush returned.
pub struct HostBuffer {
    factory: BlockFactory,
    frames: [FrameArena; FRAMES],
    frame: usize,
    stride: u64,
    /// The arena footprint of one kernel: [`KERNEL_SIZE`] rounded up to the
    /// stride, so every allocation keeps the offset alignment.
    kernel_stride: u64,
    uniform_block_size: u64,
    /// Blocks created this frame (stats: should go quiet after warm-up).
    blocks_created: u32,
}

/// `BlockFactory` is what every new block needs of the device.
struct BlockFactory {
    device: wgpu::Device,
    record_layout: wgpu::BindGroupLayout,
    kernel_layout: wgpu::BindGroupLayout,
    writer: BlockWriter,
}

/// `BlockWriter` is how a frame's blocks reach the GPU, picked once per
/// device. It owns its protocol: what a block is created with, when a block
/// may take a frame's data, how a flush writes it, and what has to happen
/// once the frame is submitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockWriter {
    /// Each block goes through one `queue.write_buffer`, which wgpu stages,
    /// so the ring needs no fences.
    Queue,
    /// The device's primary buffers are host-visible
    /// (`MAPPABLE_PRIMARY_BUFFERS`, unified memory): a block is mapped for
    /// writing and the scratch is copied in place, with no copy pass and
    /// nothing staged. Its map is asked for again once the frame that used
    /// it is submitted, and it comes back by the block's next turn in the
    /// ring; until it has, the block is still the GPU's and is passed over.
    Mapped,
}

#[derive(Default)]
struct FrameArena {
    uniforms: Vec<Block<UniformBindGroups>>,
    cursor: Cursor,
    vertices: Vec<Block<()>>,
    vertex_cursor: Cursor,
}

#[derive(Default, Clone, Copy)]
struct Cursor {
    block: usize,
    offset: u64,
}

/// `Block` is one buffer of the arena and its CPU scratch; `views` is what
/// it is bound through: a uniform block's bind groups, one per record
/// shape, or nothing for a vertex block.
struct Block<Views> {
    buffer: wgpu::Buffer,
    views: Views,
    scratch: Vec<u8>,
    used: u64,
    /// Ring passes since this block last held data (see `begin_frame`).
    idle: u8,
    /// Whether the buffer is mapped for writing now; set by the map's
    /// callback, which runs during whichever `device.poll` delivers it.
    writable: Arc<AtomicBool>,
}

/// `UniformBindGroups` are one uniform block's two views: a draw record at
/// group 0 and a blur kernel at group 2, each bound at a dynamic offset.
struct UniformBindGroups {
    record: wgpu::BindGroup,
    kernel: wgpu::BindGroup,
}

/// `UniformSlot` is where one allocation of the uniform blocks lives this
/// frame: a draw's record or a blur pass's kernel.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UniformSlot {
    pub block: usize,
    pub offset: u32,
}

/// A transient vertex range (offsets in bytes into the block's buffer).
#[derive(Clone, Copy, Debug)]
pub(crate) struct VertexSlot {
    pub block: usize,
    pub offset: u64,
    pub bytes: u64,
}

/// `Flushed` is a flushed frame's receipt: what it uploaded, and the blocks
/// whose maps [`HostBuffer::after_submit`] asks for again once the frame
/// is submitted.
#[must_use = "a flushed frame's blocks are remapped by `after_submit`"]
pub struct Flushed {
    /// `uniform_bytes` is the number of uniform bytes uploaded.
    pub uniform_bytes: u64,
    /// `vertex_bytes` is the number of vertex bytes uploaded.
    pub vertex_bytes: u64,
    /// `blocks_created` is the number of blocks this frame created.
    pub blocks_created: u32,
    remaps: Vec<Remap>,
}

/// `Remap` is one block written through its map, whose map is to be asked
/// for again.
struct Remap {
    buffer: wgpu::Buffer,
    writable: Arc<AtomicBool>,
}

impl HostBuffer {
    /// `new` creates an empty host buffer for `device`.
    ///
    /// Uniform stride is at least the per-draw record size and at least the
    /// device's `min_uniform_buffer_offset_alignment`.
    pub fn new(device: &wgpu::Device) -> Self {
        let record_layout = dynamic_uniform_layout(
            device,
            "valo.host_buffer",
            wgpu::ShaderStages::VERTEX_FRAGMENT,
            UNIFORM_SIZE,
        );
        let kernel_layout = dynamic_uniform_layout(
            device,
            "valo.host_buffer.kernel",
            wgpu::ShaderStages::FRAGMENT,
            KERNEL_SIZE,
        );
        let stride = (device.limits().min_uniform_buffer_offset_alignment as u64).max(UNIFORM_SIZE);
        Self {
            factory: BlockFactory {
                device: device.clone(),
                record_layout,
                kernel_layout,
                writer: BlockWriter::for_device(device),
            },
            frames: Default::default(),
            frame: 0,
            stride,
            kernel_stride: KERNEL_SIZE.next_multiple_of(stride),
            uniform_block_size: stride * SLOTS_PER_BLOCK,
            blocks_created: 0,
        }
    }

    /// `bind_group_layout` returns the group-0 layout for per-draw uniforms.
    ///
    /// Binding 0 is a dynamic-offset uniform buffer. Pipeline layouts are
    /// built from this layout.
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.factory.record_layout
    }

    /// `kernel_bind_group_layout` returns the group-2 layout for a blur
    /// pass's kernel.
    ///
    /// Binding 0 is a dynamic-offset uniform buffer [`KERNEL_SIZE`] long,
    /// read from the same blocks as the draw records.
    pub fn kernel_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.factory.kernel_layout
    }

    /// `begin_frame` rotates to the next arena and resets its cursors.
    ///
    /// Blocks are retained so warm frames never create buffers. Trailing
    /// unused blocks from a spike drain after a few idle ring passes so a
    /// recurring large mesh does not recreate them every frame.
    pub fn begin_frame(&mut self) {
        if self.factory.writer == BlockWriter::Mapped {
            // Delivers the map callbacks of frames the GPU has finished with.
            let _ = self.factory.device.poll(wgpu::PollType::Poll);
        }
        self.blocks_created = 0;
        self.frame = (self.frame + 1) % FRAMES;
        let arena = &mut self.frames[self.frame];
        arena.cursor = Cursor::default();
        arena.vertex_cursor = Cursor::default();
        recycle(&mut arena.uniforms);
        recycle(&mut arena.vertices);
    }

    /// Bump-allocate one uniform slot and copy `bytes` into scratch.
    pub(crate) fn alloc_uniform(&mut self, bytes: &[u8]) -> UniformSlot {
        debug_assert!(bytes.len() as u64 <= self.stride);
        self.alloc_uniform_footprint(bytes, self.stride)
    }

    /// `alloc_kernel` bump-allocates one blur kernel, [`KERNEL_SIZE`] bytes,
    /// from the same blocks as the draw records; the slot binds at group 2.
    pub(crate) fn alloc_kernel(&mut self, bytes: &[u8]) -> UniformSlot {
        debug_assert!(bytes.len() as u64 == KERNEL_SIZE);
        self.alloc_uniform_footprint(bytes, self.kernel_stride)
    }

    /// `alloc_uniform_footprint` takes `footprint` bytes of the uniform arena,
    /// a multiple of the stride, and copies `bytes` to its start.
    fn alloc_uniform_footprint(&mut self, bytes: &[u8], footprint: u64) -> UniformSlot {
        let arena = &mut self.frames[self.frame];
        let writer = self.factory.writer;
        advance_cursor(&mut arena.cursor, &arena.uniforms, footprint, writer);
        if arena.cursor.block >= arena.uniforms.len() {
            let block = self.factory.uniform_block(self.uniform_block_size);
            arena.uniforms.push(block);
            self.blocks_created += 1;
        }
        let cursor = arena.cursor;
        write_scratch(&mut arena.uniforms[cursor.block], cursor.offset, bytes);
        arena.cursor.offset += footprint;
        UniformSlot {
            block: cursor.block,
            offset: cursor.offset as u32,
        }
    }

    /// Bump-allocate a transient vertex range and copy `bytes` into scratch.
    pub(crate) fn alloc_vertices(&mut self, bytes: &[u8]) -> VertexSlot {
        let len = bytes.len() as u64;
        let arena = &mut self.frames[self.frame];
        let writer = self.factory.writer;
        advance_cursor(&mut arena.vertex_cursor, &arena.vertices, len, writer);
        if arena.vertex_cursor.block >= arena.vertices.len() {
            let block = self.factory.vertex_block(VERTEX_BLOCK_SIZE.max(len));
            arena.vertices.push(block);
            self.blocks_created += 1;
        }
        let cursor = arena.vertex_cursor;
        write_scratch(&mut arena.vertices[cursor.block], cursor.offset, bytes);
        arena.vertex_cursor.offset += len.next_multiple_of(4);
        VertexSlot {
            block: cursor.block,
            offset: cursor.offset,
            bytes: len,
        }
    }

    /// `flush` uploads this frame's used scratch to the GPU and returns the
    /// frame's receipt, which [`Self::after_submit`] takes once the frame is
    /// submitted.
    pub fn flush(&mut self, queue: &wgpu::Queue) -> Flushed {
        let arena = &mut self.frames[self.frame];
        let mut flushed = Flushed {
            uniform_bytes: arena.uniforms.iter().map(|b| b.used).sum(),
            vertex_bytes: arena.vertices.iter().map(|b| b.used).sum(),
            blocks_created: self.blocks_created,
            remaps: Vec::new(),
        };
        let writer = self.factory.writer;
        for block in arena.uniforms.iter_mut().filter(|block| block.used > 0) {
            writer.write(block, queue, &mut flushed.remaps);
        }
        for block in arena.vertices.iter_mut().filter(|block| block.used > 0) {
            writer.write(block, queue, &mut flushed.remaps);
        }
        flushed
    }

    /// `after_submit` asks again for the maps of the blocks the submitted
    /// frame wrote through them, so they are writable by their next turn in
    /// the ring. Nothing for a device that writes through the queue.
    pub fn after_submit(&mut self, flushed: Flushed) {
        for remap in flushed.remaps {
            let writable = remap.writable;
            remap
                .buffer
                .slice(..)
                .map_async(wgpu::MapMode::Write, move |result| {
                    if result.is_ok() {
                        writable.store(true, Ordering::Release);
                    }
                });
        }
    }

    /// Retained blocks across the whole ring: what a spike frame pins.
    pub(crate) fn report(&self) -> crate::PoolReport {
        let mut count = 0u32;
        let mut bytes = 0u64;
        for arena in &self.frames {
            let scratch = arena.uniforms.iter().map(|b| b.scratch.len());
            for len in scratch.chain(arena.vertices.iter().map(|b| b.scratch.len())) {
                count += 1;
                bytes += len as u64;
            }
        }
        crate::PoolReport { count, bytes }
    }

    pub(crate) fn bind_group(&self, block: usize) -> &wgpu::BindGroup {
        &self.frames[self.frame].uniforms[block].views.record
    }

    /// `kernel_bind_group` is the group-2 bind of `block`, for a kernel slot.
    pub(crate) fn kernel_bind_group(&self, block: usize) -> &wgpu::BindGroup {
        &self.frames[self.frame].uniforms[block].views.kernel
    }

    pub(crate) fn vertex_buffer(&self, block: usize) -> &wgpu::Buffer {
        &self.frames[self.frame].vertices[block].buffer
    }
}

impl BlockWriter {
    fn for_device(device: &wgpu::Device) -> Self {
        if device
            .features()
            .contains(wgpu::Features::MAPPABLE_PRIMARY_BUFFERS)
        {
            BlockWriter::Mapped
        } else {
            BlockWriter::Queue
        }
    }

    /// `usage` is what a block of `base` usage is created with.
    fn usage(self, base: wgpu::BufferUsages) -> wgpu::BufferUsages {
        match self {
            BlockWriter::Queue => base | wgpu::BufferUsages::COPY_DST,
            BlockWriter::Mapped => base | wgpu::BufferUsages::MAP_WRITE,
        }
    }

    /// `can_write` reports whether `block` may take this frame's data now.
    fn can_write<Views>(self, block: &Block<Views>) -> bool {
        match self {
            BlockWriter::Queue => true,
            BlockWriter::Mapped => block.writable.load(Ordering::Acquire),
        }
    }

    /// `write` moves `block`'s used scratch into its buffer; a block written
    /// through its map goes back to the GPU and joins `remaps`.
    fn write<Views>(self, block: &mut Block<Views>, queue: &wgpu::Queue, remaps: &mut Vec<Remap>) {
        let used = &block.scratch[..block.used as usize];
        match self {
            BlockWriter::Queue => queue.write_buffer(&block.buffer, 0, used),
            BlockWriter::Mapped => {
                // Only a writable block was allocated from (see `advance_cursor`).
                let mut view = block
                    .buffer
                    .slice(..block.used)
                    .get_mapped_range_mut()
                    .expect("a block allocated from is mapped for writing");
                view.copy_from_slice(used);
                drop(view);
                block.buffer.unmap();
                block.writable.store(false, Ordering::Release);
                remaps.push(Remap {
                    buffer: block.buffer.clone(),
                    writable: Arc::clone(&block.writable),
                });
            }
        }
    }
}

impl BlockFactory {
    /// `uniform_block` is a new uniform block of `size` bytes with its two
    /// bind groups.
    fn uniform_block(&self, size: u64) -> Block<UniformBindGroups> {
        let buffer = self.buffer(
            "valo.host_buffer.uniforms",
            wgpu::BufferUsages::UNIFORM,
            size,
        );
        let bind_groups = UniformBindGroups {
            record: uniform_view(&self.device, &self.record_layout, &buffer, UNIFORM_SIZE),
            kernel: uniform_view(&self.device, &self.kernel_layout, &buffer, KERNEL_SIZE),
        };
        self.block(buffer, bind_groups, size)
    }

    /// `vertex_block` is a new vertex block of `size` bytes.
    fn vertex_block(&self, size: u64) -> Block<()> {
        let buffer = self.buffer(
            "valo.host_buffer.vertices",
            wgpu::BufferUsages::VERTEX,
            size,
        );
        self.block(buffer, (), size)
    }

    fn buffer(&self, label: &str, usage: wgpu::BufferUsages, size: u64) -> wgpu::Buffer {
        let mapped = self.writer == BlockWriter::Mapped;
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: self.writer.usage(usage),
            mapped_at_creation: mapped,
        })
    }

    fn block<Views>(&self, buffer: wgpu::Buffer, views: Views, size: u64) -> Block<Views> {
        Block {
            buffer,
            views,
            scratch: vec![0; size as usize],
            used: 0,
            idle: 0,
            writable: Arc::new(AtomicBool::new(self.writer == BlockWriter::Mapped)),
        }
    }
}

/// Walk to the first RETAINED block with room for `needed` bytes (blocks
/// keep whatever size they were created with — judging fit by anything
/// else can strand the cursor on a too-small block and overrun it) that the
/// writer can write now. Lands past the end when nothing fits; the caller
/// pushes a right-sized block.
fn advance_cursor<Views>(
    cursor: &mut Cursor,
    blocks: &[Block<Views>],
    needed: u64,
    writer: BlockWriter,
) {
    while let Some(block) = blocks.get(cursor.block) {
        if writer.can_write(block) && cursor.offset + needed <= block.scratch.len() as u64 {
            return;
        }
        cursor.block += 1;
        cursor.offset = 0;
    }
}

/// `dynamic_uniform_layout` is a one-binding layout: a uniform buffer bound
/// `size` bytes at a time, at a dynamic offset.
fn dynamic_uniform_layout(
    device: &wgpu::Device,
    label: &str,
    visibility: wgpu::ShaderStages,
    size: u64,
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: NonZeroU64::new(size),
            },
            count: None,
        }],
    })
}

/// `uniform_view` binds `size` bytes of `buffer`, placed by a dynamic offset.
fn uniform_view(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    size: u64,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("valo.host_buffer.uniforms"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: NonZeroU64::new(size),
            }),
        }],
    })
}

/// `recycle` starts a ring pass over `blocks`: each block's use is
/// reset, idle passes counted, and trailing blocks idle for long enough
/// dropped.
fn recycle<Views>(blocks: &mut Vec<Block<Views>>) {
    for b in blocks.iter_mut() {
        b.idle = if b.used > 0 {
            0
        } else {
            b.idle.saturating_add(1)
        };
        b.used = 0;
    }
    while blocks.len() > 1 && blocks.last().is_some_and(|b| b.idle >= IDLE_PASSES) {
        blocks.pop();
    }
}

fn write_scratch<Views>(block: &mut Block<Views>, offset: u64, bytes: &[u8]) {
    block.scratch[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
    block.used = block.used.max(offset + bytes.len() as u64);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device with host-visible primary buffers, where the adapter offers them.
    fn mappable_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        if !adapter
            .features()
            .contains(wgpu::Features::MAPPABLE_PRIMARY_BUFFERS)
        {
            return None;
        }
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::MAPPABLE_PRIMARY_BUFFERS,
            ..Default::default()
        }))
        .ok()
    }

    #[test]
    fn a_mapped_block_is_written_in_place_and_writable_again_by_its_next_turn() {
        let Some((device, queue)) = mappable_device() else {
            eprintln!("SKIP a_mapped_block_is_written_in_place: no mappable adapter");
            return;
        };
        let mut host = HostBuffer::new(&device);
        assert_eq!(host.factory.writer, BlockWriter::Mapped);
        host.begin_frame();
        let slot = host.alloc_uniform(&[7u8; 64]);
        let flushed = host.flush(&queue);
        assert_eq!(
            (flushed.uniform_bytes, flushed.vertex_bytes),
            (64, 0),
            "the bytes used, not the stride"
        );
        let block = &host.frames[host.frame].uniforms[slot.block];
        assert!(
            !block.writable.load(Ordering::Acquire),
            "unmapped for the GPU"
        );
        host.after_submit(flushed);
        // Nothing was submitted, so the map completes at the next poll: the block is
        // writable again when its turn in the ring comes back.
        for _ in 0..FRAMES {
            host.begin_frame();
        }
        let block = &host.frames[host.frame].uniforms[slot.block];
        assert!(block.writable.load(Ordering::Acquire));
        let again = host.alloc_uniform(&[8u8; 64]);
        assert_eq!(again.block, slot.block, "the block is written again");
        assert_eq!(host.blocks_created, 0, "and no block is created for it");
    }

    fn headless() -> Option<wgpu::Device> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .ok()?;
        let (device, _queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        Some(device)
    }

    /// A blur kernel comes out of the same block as the draw records around
    /// it, at an aligned offset, so a blur pass creates no buffer of its own.
    #[test]
    fn kernels_share_the_uniform_blocks() {
        let Some(device) = headless() else {
            eprintln!("SKIP kernels_share_the_uniform_blocks: no GPU adapter");
            return;
        };
        let mut host = HostBuffer::new(&device);
        host.begin_frame();
        let before = host.alloc_uniform(&[1u8; UNIFORM_SIZE as usize]);
        let kernel = host.alloc_kernel(&[2u8; KERNEL_SIZE as usize]);
        let after = host.alloc_uniform(&[3u8; UNIFORM_SIZE as usize]);
        assert_eq!((before.block, kernel.block, after.block), (0, 0, 0));
        assert_eq!(u64::from(kernel.offset) % host.stride, 0);
        assert!(u64::from(after.offset) >= u64::from(kernel.offset) + KERNEL_SIZE);
        assert_eq!(host.blocks_created, 1);
    }

    /// The B2 repro: a big retained block followed by a small one, revisited
    /// next ring pass with allocations that fit the BIG block's remaining
    /// space. The old fit check judged by the incoming allocation's would-be
    /// block size, evicted to the small block, and overran its scratch.
    #[test]
    fn retained_mixed_size_blocks_never_overrun() {
        let Some(device) = headless() else {
            eprintln!("SKIP retained_mixed_size_blocks_never_overrun: no GPU adapter");
            return;
        };
        let mut host = HostBuffer::new(&device);
        host.begin_frame();
        // Ring slot N: one oversized dedicated block, then a default block.
        host.alloc_vertices(&vec![1u8; 1024 * 1024]);
        host.alloc_vertices(&[2u8; 64]);
        // Come back around to the same ring slot (blocks are retained).
        for _ in 0..FRAMES {
            host.begin_frame();
        }
        let a = host.alloc_vertices(&vec![3u8; 200 * 1024]);
        let b = host.alloc_vertices(&vec![4u8; 800 * 1024]); // used to panic
        assert_eq!(a.block, 0);
        assert_eq!(b.block, 0, "800 KB still fits the 1 MB block");

        // And a genuine overflow walks PAST the small block into a fresh
        // right-sized one instead of overrunning it.
        let c = host.alloc_vertices(&vec![5u8; 900 * 1024]);
        assert_eq!(c.bytes, 900 * 1024);
        assert!(c.block >= 2, "small retained block is skipped, not overrun");
    }
}
