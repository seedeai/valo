//! The shared CoreVideo backing must outlive every GPU command that samples it.
#![cfg(any(target_os = "macos", target_os = "ios"))]
use image::ImageEncoder;
use pollster::block_on;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use valo_codec::{DecodeOptions, ImageLoader};
use valo_codec_apple::AppleDecoder;

fn red_png() -> Arc<[u8]> {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&[255, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    bytes.into()
}

/// A compute pass that samples the image once; recording it is what retains the texture.
fn record_sample(device: &wgpu::Device, image: &valo::Image) -> wgpu::CommandBuffer {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(
            "@group(0) @binding(0) var image: texture_2d<f32>;
             @group(0) @binding(1) var<storage, read_write> result: array<vec4<f32>>;
             @compute @workgroup_size(1) fn main() { result[0] = textureLoad(image, vec2<i32>(0, 0), 0); }"
                .into(),
        ),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(image.view()),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.finish()
}

#[test]
fn native_backing_is_held_by_recorded_commands_until_the_gpu_retires_them() {
    let (device, queue) = valo_harness::headless_device().unwrap();
    let images = valo::ImageContext::new(device.clone(), queue.clone());
    let loader = ImageLoader::new(images, vec![Box::new(AppleDecoder::default())]);
    let decoded = block_on(loader.decode(red_png(), DecodeOptions::default())).unwrap();
    assert_eq!(decoded.texture().format(), wgpu::TextureFormat::Bgra8Unorm);

    // Import the same Metal texture a second time with an instrumented release callback that
    // owns the original image, so the callback's timing tells us when wgpu let go.
    let raw = {
        let hal = unsafe { decoded.texture().as_hal::<wgpu::hal::api::Metal>() }.unwrap();
        NonNull::from(hal.raw_handle()).cast::<std::ffi::c_void>()
    };
    let released = Arc::new(AtomicBool::new(false));
    let release_flag = released.clone();
    let texture = unsafe {
        valo::import_metal_texture(
            &device,
            raw,
            Box::new(move || {
                drop(decoded);
                release_flag.store(true, Ordering::Release);
            }),
        )
    }
    .unwrap();
    let imported = valo::Image::from_texture(texture, [1, 1], 1);
    {
        let hal = unsafe { imported.texture().as_hal::<wgpu::hal::api::Metal>() }.unwrap();
        assert_eq!(
            NonNull::from(hal.raw_handle()).cast::<std::ffi::c_void>(),
            raw,
            "import retains the original texture rather than copying it"
        );
    }
    drop(loader);

    let commands = record_sample(&device, &imported);
    drop(imported);
    device.poll(wgpu::PollType::Poll).unwrap();
    assert!(
        !released.load(Ordering::Acquire),
        "recorded commands must keep the native backing alive"
    );
    queue.submit([commands]);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    assert!(
        released.load(Ordering::Acquire),
        "the backing is released once the GPU has finished with it"
    );
}
