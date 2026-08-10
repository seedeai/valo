//! Dev harness: everything tests and examples need to run valo **without a browser**
//! (and without a window): headless device bring-up, texture readback, and the
//! golden-image runner. Never linked by shipping crates.

pub mod interactive;
pub mod scenes;

use std::path::Path;
use std::sync::Arc;

/// The fonts every bench/example scene shares (Fira Sans from assets/).
/// Fira Sans for content, JetBrains Mono (OFL; see
/// assets/fonts/OFL.txt) for HUDs.
pub fn example_fonts() -> Arc<valo::FontCollection> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut fonts = valo::FontCollection::new();
    fonts
        .register(
            "Fira Sans",
            std::fs::read(format!("{dir}/fira_sans.ttf")).expect("fira_sans.ttf"),
        )
        .expect("register Fira Sans");
    fonts
        .register(
            "JetBrains Mono",
            std::fs::read(format!("{dir}/jetbrains_mono.ttf")).expect("jetbrains_mono.ttf"),
        )
        .expect("register JetBrains Mono");
    Arc::new(fonts)
}

/// Bring up a headless native device. `None` when the machine has no adapter
/// (CI shells) — golden tests skip gracefully instead of failing.
pub fn headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .ok()?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("valo.harness"),
                // GPU timing when the adapter offers it (stats.gpu_ms).
                required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
                ..Default::default()
            })
            .await
            .ok()?;
        Some((device, queue))
    })
}

/// Read an RGBA8 texture back to tightly-packed bytes (row padding stripped).
pub fn read_texture_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    size: [u32; 2],
) -> Vec<u8> {
    let [w, h] = size;
    let bytes_per_row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("valo.harness.readback"),
        size: bytes_per_row as u64 * h as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
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
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range();
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let start = (row * bytes_per_row) as usize;
        out.extend_from_slice(&data[start..start + (w * 4) as usize]);
    }
    out
}

/// Compare `actual` against the checked-in golden PNG. `VALO_BLESS=1` (re)writes
/// the golden instead. On mismatch, writes `<name>.actual.png` beside the golden
/// and panics with a pixel report.
pub fn assert_golden(dir: &Path, name: &str, size: [u32; 2], actual: &[u8]) {
    /// Per-channel tolerance: absorbs backend rounding, catches real changes.
    const TOLERANCE: u8 = 3;
    let golden_path = dir.join(format!("{name}.png"));

    if std::env::var_os("VALO_BLESS").is_some() {
        std::fs::create_dir_all(dir).unwrap();
        write_png(&golden_path, size, actual);
        eprintln!("blessed {}", golden_path.display());
        return;
    }

    let golden = read_png(&golden_path).unwrap_or_else(|| {
        panic!(
            "missing golden {} — run with VALO_BLESS=1 to create it",
            golden_path.display()
        )
    });
    assert_eq!(golden.0, size, "golden {name} size changed");

    let mut bad = 0usize;
    let mut max_diff = 0u8;
    for (a, g) in actual.iter().zip(golden.1.iter()) {
        let d = a.abs_diff(*g);
        max_diff = max_diff.max(d);
        if d > TOLERANCE {
            bad += 1;
        }
    }
    if bad > 0 {
        let actual_path = dir.join(format!("{name}.actual.png"));
        write_png(&actual_path, size, actual);
        panic!(
            "golden {name}: {bad} channel values differ by more than {TOLERANCE} (max diff {max_diff}); \
             actual written to {} — VALO_BLESS=1 to accept",
            actual_path.display()
        );
    }
}

pub fn write_png(path: &Path, size: [u32; 2], rgba: &[u8]) {
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), size[0], size[1]);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

fn read_png(path: &Path) -> Option<([u32; 2], Vec<u8>)> {
    let file = std::fs::File::open(path).ok()?;
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    Some(([info.width, info.height], buf))
}

/// One-call example runner: headless device → build the scene (the closure
/// gets the `Context` so it can upload images) → render → PNG under
/// `target/examples/<name>.png` + a stats line. Keeps every per-feature
/// example down to its scene.
pub fn run_example(
    name: &str,
    size: [u32; 2],
    clear: valo::Color,
    build: impl FnOnce(&mut valo::Context) -> valo::DisplayList,
) {
    let Some((device, queue)) = headless_device() else {
        eprintln!("{name}: no GPU adapter — skipping");
        return;
    };
    let mut ctx = valo::Context::new(device.clone(), queue.clone());
    let offscreen = valo::Offscreen::new(&device, size);
    let dl = build(&mut ctx);
    let stats = ctx.render(&dl, &offscreen.target(Some(clear)));
    println!(
        "{name}: ops {} · draws {} · clips {} · culled {} · layers {}+{}e · snapshots {} · backdrops {}+{}s · filters {} · reordered {} · text {}/{}/{} · cpu {:.2}ms · gpu {:.2}ms",
        stats.ops,
        stats.draws,
        stats.clips,
        stats.culled,
        stats.layers_rendered,
        stats.layers_elided,
        stats.snapshots,
        stats.backdrops,
        stats.shared_backdrops,
        stats.filter_passes,
        stats.opaque_reordered,
        stats.text_tiers[0],
        stats.text_tiers[1],
        stats.text_tiers[2],
        stats.cpu_ms,
        stats.gpu_ms
    );
    let rgba = read_texture_rgba(&device, &queue, offscreen.texture(), size);
    let dir = Path::new("target/examples");
    std::fs::create_dir_all(dir).expect("create target/examples");
    let path = dir.join(format!("{name}.png"));
    write_png(&path, size, &rgba);
    println!("wrote {}", path.display());
}
