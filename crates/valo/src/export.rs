//! The export path: render to an offscreen target and read back
//! STRAIGHT-alpha RGBA8 — what PNG encoders and hosts expect. Internally
//! valo is premultiplied end to end (goldens stay premultiplied for
//! byte-exactness); unpremultiplying is an export-boundary concern only.

// The render-and-read-back path is native-only (web exports go through the
// canvas), so its imports live under the same cfg.
#[cfg(not(target_arch = "wasm32"))]
use valo_dl::DisplayList;
#[cfg(not(target_arch = "wasm32"))]
use valo_geometry::Color;

#[cfg(not(target_arch = "wasm32"))]
use crate::surface::Offscreen;
#[cfg(not(target_arch = "wasm32"))]
use crate::Context;

#[cfg(not(target_arch = "wasm32"))]
impl Context {
    /// `render_to_rgba` renders a display list into straight-alpha RGBA8 pixels.
    ///
    /// The returned rows are tightly packed. This native-only operation blocks
    /// until GPU readback completes.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_to_rgba(
        &mut self,
        dl: &DisplayList,
        size: [u32; 2],
        clear: Option<Color>,
    ) -> Vec<u8> {
        let offscreen = Offscreen::new(self.device(), size);
        self.render(dl, &offscreen.target(clear));
        let mut pixels = read_back(
            self.device(),
            &self.queue_handle(),
            offscreen.texture(),
            size,
        );
        unpremultiply(&mut pixels);
        pixels
    }
}

/// `unpremultiply` converts premultiplied RGBA8 pixels to straight alpha in place.
///
/// Fully transparent pixels are set to transparent black because their color
/// channels are undefined. `pixels` must contain complete four-byte pixels;
/// any trailing bytes are left unchanged.
pub fn unpremultiply(pixels: &mut [u8]) {
    for px in pixels.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
            continue;
        }
        for c in &mut px[..3] {
            *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
        }
    }
}

/// Copy the texture into a mappable buffer (rows padded to the 256-byte
/// copy alignment), block until mapped, strip the padding.
#[cfg(not(target_arch = "wasm32"))]
fn read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    size: [u32; 2],
) -> Vec<u8> {
    let [w, h] = size;
    let bytes_per_row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("valo.export.readback"),
        size: bytes_per_row as u64 * h as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| result.expect("map export"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll export");
    let data = slice.get_mapped_range().expect("export staging maps");
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let start = (row * bytes_per_row) as usize;
        out.extend_from_slice(&data[start..start + (w * 4) as usize]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::unpremultiply;

    #[test]
    fn unpremultiply_inverts_premultiplication() {
        // (128, 0, 64, 128) premul → straight: 64·255/128 = 127.5 → 128.
        let mut px = vec![128, 0, 64, 128, 10, 20, 30, 0];
        unpremultiply(&mut px);
        assert_eq!(&px[..4], &[255, 0, 128, 128]);
        assert_eq!(&px[4..], &[0, 0, 0, 0], "transparent pixels zero out");
    }
}
