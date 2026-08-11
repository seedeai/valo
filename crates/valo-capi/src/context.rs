//! The rendering context: GPU bring-up, image upload, and the two render
//! routes — a presentable window surface (from a raw `CAMetalLayer*`) and
//! headless render-to-pixels (exports, golden tests).

use valo::{Color, ImageDesc};

use crate::{borrow, borrow_mut, dispose_handle, into_handle, ValoColor, ValoDisplayList};

pub struct ValoContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    context: valo::Context,
    surface: Option<valo::Surface>,
}

pub struct ValoImage {
    pub(crate) image: valo::Image,
}

/// Bring up the GPU (instance → adapter → device) and a valo context, with
/// no window attached — pair with [`valo_context_attach_metal_layer`] to
/// present, or render headless. Null when no adapter exists.
#[no_mangle]
pub extern "C" fn valo_context_new() -> *mut ValoContext {
    let Some((instance, adapter, device, queue)) = request_gpu() else {
        return std::ptr::null_mut();
    };
    let context = valo::Context::new(device.clone(), queue);
    into_handle(ValoContext {
        instance,
        adapter,
        device,
        context,
        surface: None,
    })
}

/// # Safety
/// `context` must be a live [`valo_context_new`] handle (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_context_dispose(context: *mut ValoContext) {
    unsafe { dispose_handle(context) }
}

/// Attach a presentable surface over a raw `CAMetalLayer*` (macOS/iOS).
/// Returns false when surface creation fails; a previous surface is
/// replaced.
///
/// # Safety
/// `context` must be a live handle; `metal_layer` must be a valid
/// `CAMetalLayer*` that outlives the surface.
#[no_mangle]
pub unsafe extern "C" fn valo_context_attach_metal_layer(
    context: *mut ValoContext,
    metal_layer: *mut std::ffi::c_void,
    width: u32,
    height: u32,
) -> bool {
    let Some(ctx) = (unsafe { borrow_mut(context) }) else {
        return false;
    };
    if metal_layer.is_null() {
        return false;
    }
    let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(metal_layer);
    let surface = unsafe {
        valo::Surface::new_unsafe(
            &ctx.instance,
            &ctx.adapter,
            &ctx.device,
            target,
            [width, height],
        )
    };
    match surface {
        Ok(surface) => {
            ctx.surface = Some(surface);
            true
        }
        Err(_) => false,
    }
}

/// Resize the attached surface (no-op without one).
///
/// # Safety
/// `context` must be a live handle (or null, a no-op).
#[no_mangle]
pub unsafe extern "C" fn valo_context_resize(context: *mut ValoContext, width: u32, height: u32) {
    if let Some(ctx) = unsafe { borrow_mut(context) } {
        if let Some(surface) = &mut ctx.surface {
            surface.resize([width, height]);
        }
    }
}

/// The Metal device the context renders with (macOS) — hand it to a
/// `CAMetalLayer` so externally-owned swapchain textures live on the same
/// GPU device. Borrowed: valid while the context lives, not retained.
///
/// # Safety
/// `context` must be a live handle (or null → null).
#[cfg(target_os = "macos")]
#[no_mangle]
pub unsafe extern "C" fn valo_context_metal_device(
    context: *mut ValoContext,
) -> *mut std::ffi::c_void {
    let Some(ctx) = (unsafe { borrow_mut(context) }) else {
        return std::ptr::null_mut();
    };
    valo::metal_device_of(&ctx.device).map_or(std::ptr::null_mut(), |device| device.as_ptr())
}

/// Render one frame into a caller-owned `MTLTexture*` (macOS) — the
/// external-swapchain route: the embedder drives the drawable cycle, valo
/// only draws. `format`: 0 bgra8unorm · 1 rgba8unorm, matching the
/// texture. The texture must allow copies (set the layer's
/// `framebufferOnly` to false) — dst-reading blends snapshot the target.
/// Returns after SUBMISSION: presenting a drawable right after is safe
/// (the display waits for the drawable's GPU writes on its own), but call
/// [`valo_context_wait_for_gpu`] before reading the texture from the CPU.
///
/// # Safety
/// `context` and `list` must be live handles; `texture` must be a valid
/// `MTLTexture*` of exactly `width` × `height` in `format`, created on
/// [`valo_context_metal_device`]'s device.
#[cfg(target_os = "macos")]
#[no_mangle]
pub unsafe extern "C" fn valo_context_render_to_metal_texture(
    context: *mut ValoContext,
    list: *const ValoDisplayList,
    clear: ValoColor,
    texture: *mut std::ffi::c_void,
    width: u32,
    height: u32,
    format: i32,
) -> bool {
    let (Some(ctx), Some(list)) = (unsafe { borrow_mut(context) }, unsafe { borrow(list) }) else {
        return false;
    };
    let Some(texture) = std::ptr::NonNull::new(texture) else {
        return false;
    };
    if width == 0 || height == 0 {
        return false;
    }
    let format = metal_texture_format(format);
    let external =
        unsafe { valo::ExternalMetalTexture::wrap(&ctx.device, texture, [width, height], format) };
    ctx.context
        .render(&list.list, &external.target(Some(clear.into())));
    reclaim(ctx);
    true
}

/// One non-blocking poll per frame: wgpu frees dead resources only when a
/// poll observes GPU completion — a submit-only loop frees NOTHING
/// (measured: ~16 KB leaked per frame). Frame-rate loops reclaim fully
/// this way; unthrottled loops (benchmarks) outrun completion signaling
/// and must call [`valo_context_wait_for_gpu`] periodically instead.
fn reclaim(ctx: &mut ValoContext) {
    let _ = ctx.device.poll(wgpu::PollType::Poll);
}

/// Block until every submitted frame has finished on the GPU — needed
/// only before CPU reads of a rendered texture (tests, exports); frame
/// loops must NOT call this (it serializes the pipeline).
///
/// # Safety
/// `context` must be a live handle (or null, a no-op).
#[cfg(target_os = "macos")]
#[no_mangle]
pub unsafe extern "C" fn valo_context_wait_for_gpu(context: *mut ValoContext) {
    if let Some(ctx) = unsafe { borrow_mut(context) } {
        let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
    }
}

/// Wrap a caller-owned `MTLTexture*` as a drawable image, zero-copy
/// (macOS) — external renderers (a 3D pass, a video frame) draw straight
/// into valo frames without a readback. The texture must be created with
/// shader-read usage and stay alive while the image is drawn.
///
/// # Safety
/// `context` must be a live handle; `texture` must be a valid
/// `MTLTexture*` of exactly `width` × `height` in `format` (0 bgra8unorm
/// · 1 rgba8unorm), created on [`valo_context_metal_device`]'s device.
#[cfg(target_os = "macos")]
#[no_mangle]
pub unsafe extern "C" fn valo_context_import_metal_texture(
    context: *mut ValoContext,
    texture: *mut std::ffi::c_void,
    width: u32,
    height: u32,
    format: i32,
) -> *mut ValoImage {
    let Some(ctx) = (unsafe { borrow_mut(context) }) else {
        return std::ptr::null_mut();
    };
    let Some(texture) = std::ptr::NonNull::new(texture) else {
        return std::ptr::null_mut();
    };
    if width == 0 || height == 0 {
        return std::ptr::null_mut();
    }
    let wrapped = unsafe {
        valo::wrap_metal_texture(
            &ctx.device,
            texture,
            [width, height],
            metal_texture_format(format),
            wgpu::TextureUsages::TEXTURE_BINDING,
        )
    };
    let image = ctx.context.import_image(wrapped, [width, height]);
    into_handle(ValoImage { image })
}

#[cfg(target_os = "macos")]
fn metal_texture_format(format: i32) -> wgpu::TextureFormat {
    match format {
        1 => wgpu::TextureFormat::Rgba8Unorm,
        _ => wgpu::TextureFormat::Bgra8Unorm,
    }
}

/// Register the font collection glyph runs rasterize through — without it
/// paragraphs lay out but draw nothing. Each collection change is a new
/// snapshot, so call again after adding faces; rendering uses the
/// collection registered at render time.
///
/// Render one frame onto the attached surface and present it. Returns
/// false without a surface or when the swapchain skipped the frame
/// (occluded window) — both are recoverable, try next frame.
///
/// # Safety
/// `context` and `list` must be live handles (or null → false).
#[no_mangle]
pub unsafe extern "C" fn valo_context_render(
    context: *mut ValoContext,
    list: *const ValoDisplayList,
    clear: ValoColor,
) -> bool {
    let (Some(ctx), Some(list)) = (unsafe { borrow_mut(context) }, unsafe { borrow(list) }) else {
        return false;
    };
    let Some(surface) = &mut ctx.surface else {
        return false;
    };
    let Some(frame) = surface.acquire() else {
        return false;
    };
    ctx.context
        .render(&list.list, &frame.target(Some(clear.into())));
    frame.present();
    reclaim(ctx);
    true
}

/// Render headless into caller-allocated straight-alpha RGBA8 pixels
/// (`width * height * 4` bytes). The export/golden route.
///
/// # Safety
/// `context` and `list` must be live handles; `out_pixels` must point to
/// at least `width * height * 4` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn valo_context_render_to_pixels(
    context: *mut ValoContext,
    list: *const ValoDisplayList,
    clear: ValoColor,
    width: u32,
    height: u32,
    out_pixels: *mut u8,
) -> bool {
    let (Some(ctx), Some(list)) = (unsafe { borrow_mut(context) }, unsafe { borrow(list) }) else {
        return false;
    };
    if out_pixels.is_null() || width == 0 || height == 0 {
        return false;
    }
    let pixels = ctx
        .context
        .render_to_rgba(&list.list, [width, height], Some(Color::from(clear)));
    unsafe { std::ptr::copy_nonoverlapping(pixels.as_ptr(), out_pixels, pixels.len()) };
    true
}

/// Upload straight-alpha RGBA8 pixels as a drawable image (mipmapped).
///
/// # Safety
/// `context` must be a live handle; `pixels` must point to
/// `width * height * 4` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn valo_context_create_image(
    context: *mut ValoContext,
    width: u32,
    height: u32,
    pixels: *const u8,
) -> *mut ValoImage {
    let Some(ctx) = (unsafe { borrow_mut(context) }) else {
        return std::ptr::null_mut();
    };
    if pixels.is_null() || width == 0 || height == 0 {
        return std::ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(pixels, (width * height * 4) as usize) };
    let image = ctx.context.upload_image(
        ImageDesc {
            size: [width, height],
            premultiplied: false,
            mips: true,
        },
        bytes,
    );
    into_handle(ValoImage { image })
}

/// # Safety
/// `image` must be a live [`valo_context_create_image`] handle (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_image_dispose(image: *mut ValoImage) {
    unsafe { dispose_handle(image) }
}

/// instance → adapter → device/queue, blocking (bring-up happens once).
fn request_gpu() -> Option<(wgpu::Instance, wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    pollster::block_on(async {
        let instance = wgpu::Instance::default();
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
                label: Some("valo.capi"),
                required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
                ..Default::default()
            })
            .await
            .ok()?;
        Some((instance, adapter, device, queue))
    })
}
