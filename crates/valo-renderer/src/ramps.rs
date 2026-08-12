//! Baked 1D gradient ramps — Impeller's texture-gradient path
//! (`geometry/gradient.cc` `CreateGradientBuffer` + `gradient_generator.cc`
//! `CreateGradientTexture`): stop lists past the uniform budget become an
//! RGBA8 N×1 straight-color texture sampled linearly, with
//! `N = min(round(1 / min_adjacent_stop_delta) + 1, 1024)` so tight stop
//! pairs stay resolvable without absurd textures. Content-keyed; entries
//! idle out after a few frames like pooled targets.

use std::hash::{Hash, Hasher};

use rustc_hash::FxHashMap;
use valo_dl::GradientStop;

use crate::report::PoolReport;

/// Frames an entry may sit unused before eviction (pool policy).
const IDLE_FRAMES: u64 = 3;
/// Impeller's cap ("avoid absurdly large textures from stops that are
/// very close together").
const MAX_TEXELS: u32 = 1024;

pub(crate) struct RampCache {
    entries: FxHashMap<u64, Entry>,
    frame: u64,
}

struct Entry {
    view: wgpu::TextureView,
    texels: u32,
    last_used: u64,
}

impl RampCache {
    pub fn new() -> Self {
        Self {
            entries: FxHashMap::default(),
            frame: 0,
        }
    }

    /// The ramp texture for `stops`, baking on first sight.
    pub fn ensure(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        stops: &[GradientStop],
    ) -> (wgpu::TextureView, u32) {
        let key = stop_key(stops);
        let frame = self.frame;
        let entry = self.entries.entry(key).or_insert_with(|| {
            let (bytes, texels) = bake(stops);
            let view = upload(device, queue, &bytes, texels);
            Entry {
                view,
                texels,
                last_used: frame,
            }
        });
        entry.last_used = frame;
        (entry.view.clone(), entry.texels)
    }

    pub fn end_frame(&mut self) {
        self.frame += 1;
        let horizon = self.frame.saturating_sub(IDLE_FRAMES);
        self.entries.retain(|_, e| e.last_used >= horizon);
    }

    pub fn report(&self) -> PoolReport {
        PoolReport {
            count: self.entries.len() as u32,
            bytes: self.entries.values().map(|e| e.texels as u64 * 4).sum(),
        }
    }
}

/// Impeller's texel-count derivation: enough resolution that the smallest
/// stop gap still spans a texel, capped.
fn texel_count(stops: &[GradientStop]) -> u32 {
    let mut minimum_delta = 1.0f32;
    for pair in stops.windows(2) {
        let delta = pair[1].offset - pair[0].offset;
        if delta < 1e-4 {
            continue; // hard stops don't drive resolution
        }
        minimum_delta = minimum_delta.min(delta);
    }
    ((1.0 / minimum_delta).round() as u32 + 1).clamp(2, MAX_TEXELS)
}

/// Piecewise-linear evaluation of the stop ramp into straight RGBA8. The
/// fragment premultiplies after sampling, matching Impeller and Skia's default
/// gradient interpolation.
fn bake(stops: &[GradientStop]) -> (Vec<u8>, u32) {
    let texels = texel_count(stops);
    let mut bytes = Vec::with_capacity(texels as usize * 4);
    for i in 0..texels {
        let t = i as f32 / (texels - 1) as f32;
        let color = sample(stops, t).components();
        for channel in color {
            bytes.push((channel * 255.0 + 0.5) as u8);
        }
    }
    (bytes, texels)
}

fn sample(stops: &[GradientStop], t: f32) -> valo_geometry::Color {
    let first = stops.first().expect("recorder rejects empty stops");
    if t <= first.offset {
        return first.color;
    }
    for pair in stops.windows(2) {
        if t <= pair[1].offset {
            let span = (pair[1].offset - pair[0].offset).max(1e-6);
            let k = (t - pair[0].offset) / span;
            return lerp(pair[0].color, pair[1].color, k);
        }
    }
    stops.last().expect("non-empty").color
}

fn lerp(a: valo_geometry::Color, b: valo_geometry::Color, k: f32) -> valo_geometry::Color {
    valo_geometry::Color {
        r: a.r + (b.r - a.r) * k,
        g: a.g + (b.g - a.g) * k,
        b: a.b + (b.b - a.b) * k,
        a: a.a + (b.a - a.a) * k,
    }
}

fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bytes: &[u8],
    texels: u32,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("valo gradient ramp"),
        size: wgpu::Extent3d {
            width: texels,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(texels * 4),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: texels,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

fn stop_key(stops: &[GradientStop]) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    for stop in stops {
        stop.offset.to_bits().hash(&mut hasher);
        stop.color.r.to_bits().hash(&mut hasher);
        stop.color.g.to_bits().hash(&mut hasher);
        stop.color.b.to_bits().hash(&mut hasher);
        stop.color.a.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use valo_dl::GradientStop;
    use valo_geometry::Color;

    use super::sample;

    #[test]
    fn translucent_stops_interpolate_before_premultiplication() {
        let stops = [
            GradientStop {
                offset: 0.0,
                color: Color::rgba(1.0, 0.0, 0.0, 1.0),
            },
            GradientStop {
                offset: 1.0,
                color: Color::rgba(0.0, 0.0, 1.0, 0.5),
            },
        ];

        assert_eq!(sample(&stops, 0.5), Color::rgba(0.5, 0.0, 0.5, 0.75));
    }
}
