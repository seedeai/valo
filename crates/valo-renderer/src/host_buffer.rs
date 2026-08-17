use std::num::NonZeroU64;

/// Bytes of one per-draw uniform record: mat4 MVP + vec4 color + the generic
/// payload (see shaders/solid.wgsl's layout contract). 512 = 2× the dynamic
/// offset alignment — stride and record size stay equal, nothing is wasted.
pub(crate) const UNIFORM_SIZE: u64 = 512;
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
/// Each frame writes into CPU scratch; [`Self::flush`] copies touched blocks
/// with one `queue.write_buffer` each. wgpu stages those writes, so the ring
/// needs no fences. A 3-frame ring of persistent buffers means warm frames
/// create nothing — the cost that matters most on wasm.
///
/// Uniforms bind once per block via a dynamic offset. Vertex data (stencil
/// fans, stroke strips) lands in a second family of blocks. All uploads go
/// through the alloc/`flush` seam so a mapped staging backend can replace
/// this implementation without touching call sites.
pub struct HostBuffer {
    device: wgpu::Device,
    layout: wgpu::BindGroupLayout,
    frames: [FrameArena; FRAMES],
    frame: usize,
    stride: u64,
    uniform_block_size: u64,
    /// Total blocks ever created (stats: should go quiet after warm-up).
    pub(crate) blocks_created: u64,
}

#[derive(Default)]
struct FrameArena {
    uniforms: Vec<Block>,
    cursor: Cursor,
    vertices: Vec<Block>,
    vertex_cursor: Cursor,
}

#[derive(Default, Clone, Copy)]
struct Cursor {
    block: usize,
    offset: u64,
}

struct Block {
    buffer: wgpu::Buffer,
    /// Uniform blocks carry their bind group; vertex blocks don't need one.
    bind_group: Option<wgpu::BindGroup>,
    scratch: Vec<u8>,
    used: u64,
    /// Ring passes since this block last held data (see `begin_frame`).
    idle: u8,
}

/// Where one draw's uniforms live this frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DrawSlot {
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

impl HostBuffer {
    /// `new` creates an empty host buffer for `device`.
    ///
    /// Uniform stride is at least the per-draw record size and at least the
    /// device's `min_uniform_buffer_offset_alignment`.
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("valo.host_buffer"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(UNIFORM_SIZE),
                },
                count: None,
            }],
        });
        let stride = (device.limits().min_uniform_buffer_offset_alignment as u64).max(UNIFORM_SIZE);
        Self {
            device: device.clone(),
            layout,
            frames: Default::default(),
            frame: 0,
            stride,
            uniform_block_size: stride * SLOTS_PER_BLOCK,
            blocks_created: 0,
        }
    }

    /// `bind_group_layout` returns the group-0 layout for per-draw uniforms.
    ///
    /// Binding 0 is a dynamic-offset uniform buffer. Pipeline layouts are
    /// built from this layout.
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// `begin_frame` rotates to the next arena and resets its cursors.
    ///
    /// Blocks are retained so warm frames never create buffers. Trailing
    /// unused blocks from a spike drain after a few idle ring passes so a
    /// recurring large mesh does not recreate them every frame.
    pub fn begin_frame(&mut self) {
        self.frame = (self.frame + 1) % FRAMES;
        let arena = &mut self.frames[self.frame];
        arena.cursor = Cursor::default();
        arena.vertex_cursor = Cursor::default();
        for blocks in [&mut arena.uniforms, &mut arena.vertices] {
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
    }

    /// Bump-allocate one uniform slot and copy `bytes` into scratch.
    pub(crate) fn alloc_uniform(&mut self, bytes: &[u8]) -> DrawSlot {
        debug_assert!(bytes.len() as u64 <= self.stride);
        let stride = self.stride;
        let block_size = self.uniform_block_size;
        let (device, layout) = (self.device.clone(), self.layout.clone());
        let arena = &mut self.frames[self.frame];

        advance_cursor(&mut arena.cursor, &arena.uniforms, stride);
        if arena.cursor.block >= arena.uniforms.len() {
            arena
                .uniforms
                .push(new_uniform_block(&device, &layout, block_size));
            self.blocks_created += 1;
        }
        let cursor = arena.cursor;
        write_scratch(&mut arena.uniforms[cursor.block], cursor.offset, bytes);
        arena.cursor.offset += stride;
        DrawSlot {
            block: cursor.block,
            offset: cursor.offset as u32,
        }
    }

    /// Bump-allocate a transient vertex range and copy `bytes` into scratch.
    pub(crate) fn alloc_vertices(&mut self, bytes: &[u8]) -> VertexSlot {
        let len = bytes.len() as u64;
        let block_size = VERTEX_BLOCK_SIZE.max(len);
        let device = self.device.clone();
        let arena = &mut self.frames[self.frame];

        advance_cursor(&mut arena.vertex_cursor, &arena.vertices, len);
        if arena.vertex_cursor.block >= arena.vertices.len() {
            arena.vertices.push(new_vertex_block(&device, block_size));
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

    /// `flush` uploads this frame's used scratch to the GPU.
    ///
    /// One `write_buffer` runs per touched block. Returns
    /// `(uniform_bytes, vertex_bytes)` written, for frame statistics.
    pub fn flush(&mut self, queue: &wgpu::Queue) -> (u64, u64) {
        let arena = &self.frames[self.frame];
        let mut written = (0u64, 0u64);
        for b in &arena.uniforms {
            written.0 += b.used;
        }
        for b in &arena.vertices {
            written.1 += b.used;
        }
        for b in arena.uniforms.iter().chain(arena.vertices.iter()) {
            if b.used > 0 {
                queue.write_buffer(&b.buffer, 0, &b.scratch[..b.used as usize]);
            }
        }
        written
    }

    /// Retained blocks across the whole ring: what a spike frame pins.
    pub(crate) fn report(&self) -> crate::PoolReport {
        let mut count = 0u32;
        let mut bytes = 0u64;
        for arena in &self.frames {
            for b in arena.uniforms.iter().chain(arena.vertices.iter()) {
                count += 1;
                bytes += b.scratch.len() as u64;
            }
        }
        crate::PoolReport { count, bytes }
    }

    pub(crate) fn bind_group(&self, block: usize) -> &wgpu::BindGroup {
        self.frames[self.frame].uniforms[block]
            .bind_group
            .as_ref()
            .expect("uniform blocks always carry a bind group")
    }

    pub(crate) fn vertex_buffer(&self, block: usize) -> &wgpu::Buffer {
        &self.frames[self.frame].vertices[block].buffer
    }
}

/// Walk to the first RETAINED block with room for `needed` bytes (blocks
/// keep whatever size they were created with — judging fit by anything
/// else can strand the cursor on a too-small block and overrun it). Lands
/// past the end when nothing fits; the caller pushes a right-sized block.
fn advance_cursor(cursor: &mut Cursor, blocks: &[Block], needed: u64) {
    while let Some(block) = blocks.get(cursor.block) {
        if cursor.offset + needed <= block.scratch.len() as u64 {
            return;
        }
        cursor.block += 1;
        cursor.offset = 0;
    }
}

fn new_uniform_block(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, size: u64) -> Block {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("valo.host_buffer.uniforms"),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("valo.host_buffer.uniforms"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buffer,
                offset: 0,
                size: NonZeroU64::new(UNIFORM_SIZE),
            }),
        }],
    });
    Block {
        buffer,
        bind_group: Some(bind_group),
        scratch: vec![0; size as usize],
        used: 0,
        idle: 0,
    }
}

fn new_vertex_block(device: &wgpu::Device, size: u64) -> Block {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("valo.host_buffer.vertices"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    Block {
        buffer,
        bind_group: None,
        scratch: vec![0; size as usize],
        used: 0,
        idle: 0,
    }
}

fn write_scratch(block: &mut Block, offset: u64, bytes: &[u8]) {
    block.scratch[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
    block.used = block.used.max(offset + bytes.len() as u64);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headless() -> Option<wgpu::Device> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .ok()?;
        let (device, _queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        Some(device)
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
