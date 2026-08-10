//! Do TRANSIENT textures actually go memoryless? Allocates ten 4-sample
//! 2048² attachments (≈64 MB each if backed) and parks so `footprint`
//! can read the ledger. Argument "backed" drops the TRANSIENT flag.
fn main() {
    let (device, _queue) = valo_harness::headless_device().expect("a GPU adapter");
    let transient = std::env::args().nth(1).as_deref() != Some("backed");
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
    if transient {
        usage |= wgpu::TextureUsages::TRANSIENT;
    }
    let textures: Vec<wgpu::Texture> = (0..10)
        .map(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("probe"),
                size: wgpu::Extent3d {
                    width: 2048,
                    height: 2048,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 4,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8Unorm,
                usage,
                view_formats: &[],
            })
        })
        .collect();
    println!(
        "allocated {} textures, transient={transient}",
        textures.len()
    );
    std::thread::sleep(std::time::Duration::from_secs(8));
}
