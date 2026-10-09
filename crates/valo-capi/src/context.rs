//! The rendering context: GPU bring-up, image upload, and the two render
//! routes — a presentable window surface (from a raw `CAMetalLayer*`) and
//! headless render-to-pixels (exports, golden tests).

use valo::{Color, ImageDesc};

use crate::{
    borrow, borrow_mut, dispose_handle, into_handle, ValoAdapterOptions, ValoColor, ValoDisplayList,
};

/// `ValoContext` is the GPU renderer handle for C embedders.
///
/// Create it with [`valo_context_new`] or [`valo_context_new_with_options`]
/// (null when no adapter gives a device) and release it with
/// [`valo_context_dispose`]. It owns the wgpu device and an optional
/// presentable surface. Handles are not thread-safe. On macOS and iOS, pair
/// it with `valo_context_attach_metal_layer` to present; anywhere, render
/// headless with [`valo_context_render_to_pixels`].
pub struct ValoContext {
    /// `instance` and `adapter` create the surface a window attaches, which
    /// only a Metal layer does so far.
    #[cfg_attr(
        not(any(target_os = "macos", target_os = "ios")),
        expect(dead_code, reason = "only a Metal layer attaches a surface")
    )]
    instance: wgpu::Instance,
    #[cfg_attr(
        not(any(target_os = "macos", target_os = "ios")),
        expect(dead_code, reason = "only a Metal layer attaches a surface")
    )]
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    context: valo::Context,
    surface: Option<valo::Surface>,
}

/// `ValoImage` is a drawable GPU image handle.
///
/// Create it with [`valo_context_create_image`] (uploads RGBA8 pixels) or,
/// on macOS, by wrapping a caller-owned Metal texture. Dispose with
/// [`valo_image_dispose`]. Recorded display lists retain the image independently
/// of this C handle.
pub struct ValoImage {
    pub(crate) image: valo::Image,
}

/// `valo_context_new` brings up the GPU and a valo context with no window attached.
///
/// Bring-up is blocking (instance → adapter → device) and happens once. It
/// is [`valo_context_new_with_options`] with any adapter admitted
/// (compatibility), high performance first and no forced fallback. On macOS
/// and iOS, pair with `valo_context_attach_metal_layer` to present;
/// anywhere, render headless. Returns null when no adapter gives a device.
#[no_mangle]
pub extern "C" fn valo_context_new() -> *mut ValoContext {
    let options = ValoAdapterOptions {
        power_preference: 2,
        force_fallback_adapter: false,
        feature_level: 0,
    };
    context_from(&options)
}

/// `valo_context_new_with_options` brings up a context on the adapter
/// `options` choose: `valo::request_device`.
///
/// Every adapter the options' feature level admits is tried, ranked by their
/// power preference with software adapters last, until one gives a device.
/// Blocking, like [`valo_context_new`]. Returns null on a null `options` or
/// when no adapter gives a device.
///
/// # Safety
/// `options` must be null or point to a valid [`ValoAdapterOptions`].
#[no_mangle]
pub unsafe extern "C" fn valo_context_new_with_options(
    options: *const ValoAdapterOptions,
) -> *mut ValoContext {
    match unsafe { options.as_ref() } {
        Some(options) => context_from(options),
        None => std::ptr::null_mut(),
    }
}

/// `context_from` opens a device as `options` say and wraps a context
/// around it; null when no adapter gives one.
fn context_from(options: &ValoAdapterOptions) -> *mut ValoContext {
    let instance = wgpu::Instance::default();
    let wgpu_options = options.wgpu_options();
    let request = valo::request_device(&instance, &wgpu_options, options.feature_level());
    let Ok((adapter, device, queue)) = pollster::block_on(request) else {
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

/// `valo_context_dispose` releases a context handle. Null is a no-op.
///
/// # Safety
/// `context` must be a live [`valo_context_new`] handle (or null).
#[no_mangle]
pub unsafe extern "C" fn valo_context_dispose(context: *mut ValoContext) {
    unsafe { dispose_handle(context) }
}

/// `valo_context_attach_metal_layer` attaches a presentable surface over a raw
/// `CAMetalLayer*` (macOS/iOS).
///
/// Returns false when surface creation fails; a previous surface is replaced.
///
/// # Safety
/// `context` must be a live handle; `metal_layer` must be a valid
/// `CAMetalLayer*` that outlives the surface.
#[cfg(any(target_os = "macos", target_os = "ios"))]
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

/// `valo_context_resize` resizes the attached surface (no-op without one).
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

/// `valo_context_metal_device` returns the Metal device the context renders with (macOS).
///
/// Hand it to a `CAMetalLayer` so externally-owned swapchain textures live
/// on the same GPU device. Borrowed: valid while the context lives, not
/// retained. Null context returns null.
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

/// `valo_context_render_to_metal_texture` draws one frame into a caller-owned
/// `MTLTexture*` (macOS).
///
/// This is the external-swapchain route: the embedder drives the drawable
/// cycle, valo only draws. `format`: 0 bgra8unorm · 1 rgba8unorm, matching
/// the texture. The texture must allow copies (set the layer's
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

/// `valo_context_wait_for_gpu` blocks until every submitted frame has finished on the GPU.
///
/// Needed only before CPU reads of a rendered texture (tests, exports);
/// frame loops must NOT call this (it serializes the pipeline). Null is a
/// no-op.
///
/// # Safety
/// `context` must be a live handle (or null, a no-op).
#[no_mangle]
pub unsafe extern "C" fn valo_context_wait_for_gpu(context: *mut ValoContext) {
    if let Some(ctx) = unsafe { borrow_mut(context) } {
        let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
    }
}

/// `valo_context_import_metal_texture` wraps a caller-owned `MTLTexture*` as a
/// drawable image, zero-copy (macOS).
///
/// External renderers (a 3D pass, a video frame) draw straight into valo
/// frames without a readback. The texture must be created with shader-read
/// usage and stay alive while the image is drawn. `format`: 0 bgra8unorm ·
/// 1 rgba8unorm. Returns null on a null handle, null texture, or zero size.
///
/// # Safety
/// `context` must be a live handle; `texture` must be a valid
/// `MTLTexture*` of exactly `width` × `height` in `format`, created on
/// [`valo_context_metal_device`]'s device.
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

/// `valo_context_render` draws one frame onto the attached surface and presents it.
///
/// Returns false without a surface or when the swapchain skipped the frame
/// (occluded window) — both are recoverable, try next frame. Null handles
/// return false.
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
    let Ok(frame) = surface.acquire() else {
        return false;
    };
    ctx.context
        .render(&list.list, &frame.target(Some(clear.into())));
    ctx.context.present(frame);
    reclaim(ctx);
    true
}

/// `valo_context_render_to_pixels` renders headless into caller-allocated
/// straight-alpha RGBA8 pixels (`width * height * 4` bytes).
///
/// This is the export and golden-test route. Returns false on a null handle,
/// null buffer, or zero size.
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

/// `valo_context_create_image` uploads straight-alpha RGBA8 pixels as a drawable
/// image (mipmapped).
///
/// The pixel buffer is copied; it only has to outlive this call. Returns
/// null on a null handle, null pixels, or zero size.
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

/// `valo_image_dispose` releases an image handle. Null is a no-op.
///
/// # Safety
/// `image` must be a live image handle (from [`valo_context_create_image`]
/// or a Metal import) or null.
#[no_mangle]
pub unsafe extern "C" fn valo_image_dispose(image: *mut ValoImage) {
    unsafe { dispose_handle(image) }
}
