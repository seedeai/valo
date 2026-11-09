//! The export path: draw a display list into pixels of any size and ask the GPU for them back,
//! as STRAIGHT-alpha RGBA8 — what encoders and hosts expect. Internally valo is premultiplied
//! end to end (goldens stay premultiplied for byte-exactness); unpremultiplying is an
//! export-boundary concern only.
//!
//! Asking follows wgpu's `map_async` and Skia's `asyncRescaleAndReadPixels`: the request goes to
//! the GPU at once, and its answer is delivered when the device is polled — by the browser on the
//! web, by the host natively through [`Context::check_requests`], as Skia's hosts call
//! `checkAsyncWorkCompletion`.

use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{self, Poll};

use futures_channel::oneshot;
use valo_dl::{DisplayList, DisplayListBuilder};
use valo_geometry::Color;
use valo_renderer::RendererCore;
use valo_renderer::{AlphaType, PixelBuffer, PixelFormat, PixelLayout};

use crate::surface::Offscreen;
use crate::Context;

/// `MAX_IN_FLIGHT_READBACKS` bounds staging buffers held by one pixel request.
const MAX_IN_FLIGHT_READBACKS: usize = 2;

/// `PixelReadError` is why a picture's pixels could not be read back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PixelReadError {
    /// `Empty` is a size with no pixels.
    Empty,
    /// `TooLarge` is a size whose pixels do not fit in one buffer.
    TooLarge,
    /// `Failed` is the GPU's copy failing, with the device's reason.
    Failed(String),
}

impl fmt::Display for PixelReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PixelReadError::Empty => write!(f, "the picture has no pixels"),
            PixelReadError::TooLarge => write!(f, "the picture's pixels don't fit in one buffer"),
            PixelReadError::Failed(why) => write!(f, "reading the picture back failed: {why}"),
        }
    }
}

impl std::error::Error for PixelReadError {}

/// `ignore_progress` is the callback for requests whose callers do not track progress.
fn ignore_progress(_: usize, _: usize) {}

impl Context {
    /// `request_pixels` draws `dl` into `size` pixels over `clear` and asks the GPU for them, as
    /// packed RGBA8 with straight alpha.
    ///
    /// The answer comes back through [`Context::check_requests`]: natively the host calls it
    /// while requests are out; on the web the browser answers on its own. The drawing, its copy
    /// and the request go to the GPU now, in tiles when `size` passes the device's largest
    /// texture, meeting without a seam. The request borrows nothing, so the host can go on
    /// drawing frames while it waits.
    pub fn request_pixels(
        &mut self,
        dl: &Arc<DisplayList>,
        size: [u32; 2],
        clear: Option<Color>,
    ) -> PixelRequest<fn(usize, usize)> {
        let side = self.device().limits().max_texture_dimension_2d;
        self.request_pixels_with_side(dl, size, clear, side, ignore_progress)
    }

    /// `request_pixels_with_progress` reports each GPU tile once its readback is mapped.
    ///
    /// A request still resolves to one assembled `PixelBuffer`. At most two tile readbacks are
    /// in flight at once, and `progress` receives `(completed_tiles, total_tiles)`.
    pub fn request_pixels_with_progress<F: FnMut(usize, usize) + 'static>(
        &mut self,
        dl: &Arc<DisplayList>,
        size: [u32; 2],
        clear: Option<Color>,
        progress: F,
    ) -> PixelRequest<F> {
        let side = self.device().limits().max_texture_dimension_2d;
        self.request_pixels_with_side(dl, size, clear, side, progress)
    }

    /// `request_pixels_with_side` uses `side` as the maximum tile edge.
    fn request_pixels_with_side<F: FnMut(usize, usize) + 'static>(
        &mut self,
        dl: &Arc<DisplayList>,
        size: [u32; 2],
        clear: Option<Color>,
        side: u32,
        progress: F,
    ) -> PixelRequest<F> {
        let copies = Copies::new(
            dl.clone(),
            size,
            clear,
            side,
            self.renderer.clone(),
            self.device().clone(),
            self.queue.clone(),
            progress,
        );
        PixelRequest {
            copies: Some(copies),
        }
    }

    /// `check_requests` delivers the requests the GPU has answered, without waiting. It fails
    /// only when the device is lost.
    pub fn check_requests(&self) -> Result<(), PixelReadError> {
        self.device()
            .poll(wgpu::PollType::Poll)
            .map(|_| ())
            .map_err(|e| PixelReadError::Failed(e.to_string()))
    }
}

/// `PixelRequest` is pixels asked of the GPU by [`Context::request_pixels`], finishing once
/// [`Context::check_requests`] delivers them natively, or the browser does on the web.
pub struct PixelRequest<F = fn(usize, usize)> {
    copies: Option<Result<Copies<F>, PixelReadError>>,
}

impl<F: FnMut(usize, usize)> Future for PixelRequest<F> {
    type Output = Result<PixelBuffer, PixelReadError>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Some(Ok(copies)) = &mut this.copies {
            match copies.read_ready_tiles(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => {
                    this.copies = None;
                    return Poll::Ready(Err(error));
                }
                Poll::Ready(Ok(false)) => return Poll::Pending,
                Poll::Ready(Ok(true)) => {}
            }
        }
        let copies = this
            .copies
            .take()
            .expect("a finished PixelRequest isn't polled again");
        Poll::Ready(copies.map(Copies::finish))
    }
}

/// `Copies` schedules bounded tile readbacks and assembles their pixels into one output buffer.
struct Copies<F> {
    layout: PixelLayout,
    renderer: Arc<Mutex<RendererCore>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    display_list: Arc<DisplayList>,
    clear: Option<Color>,
    side: u32,
    columns: u32,
    total: usize,
    next: usize,
    completed: usize,
    tiles: VecDeque<TileCopy>,
    rgba: Vec<u8>,
    progress: Box<F>,
}

impl<F: FnMut(usize, usize)> Copies<F> {
    /// `new` allocates the final buffer and starts a bounded number of tile reads.
    #[allow(clippy::too_many_arguments)]
    fn new(
        display_list: Arc<DisplayList>,
        size: [u32; 2],
        clear: Option<Color>,
        side: u32,
        renderer: Arc<Mutex<RendererCore>>,
        device: wgpu::Device,
        queue: wgpu::Queue,
        progress: F,
    ) -> Result<Self, PixelReadError> {
        let layout = checked_layout(size)?;
        if side == 0 {
            return Err(PixelReadError::Empty);
        }
        let columns = size[0].div_ceil(side);
        let rows = size[1].div_ceil(side);
        let total = usize::try_from(u64::from(columns) * u64::from(rows))
            .map_err(|_| PixelReadError::TooLarge)?;
        let rgba = vec![0; layout.byte_len().map_err(|_| PixelReadError::TooLarge)?];
        let mut copies = Self {
            layout,
            renderer,
            device,
            queue,
            display_list,
            clear,
            side,
            columns,
            total,
            next: 0,
            completed: 0,
            tiles: VecDeque::new(),
            rgba,
            progress: Box::new(progress),
        };
        copies.schedule();
        Ok(copies)
    }

    /// `read_ready_tiles` copies mapped readbacks, reports them, and keeps the queue bounded.
    fn read_ready_tiles(
        &mut self,
        cx: &mut task::Context<'_>,
    ) -> Poll<Result<bool, PixelReadError>> {
        let mut index = 0;
        while index < self.tiles.len() {
            let mapped = match self.tiles[index].answered.as_mut() {
                Some(answered) => Pin::new(answered).poll(cx),
                None => {
                    index += 1;
                    continue;
                }
            };
            let mapped = match mapped {
                Poll::Pending => {
                    index += 1;
                    continue;
                }
                Poll::Ready(Ok(mapped)) => mapped,
                Poll::Ready(Err(_)) => {
                    return Poll::Ready(Err(PixelReadError::Failed(
                        "GPU readback channel closed".to_owned(),
                    )));
                }
            };
            if let Err(error) = mapped {
                return Poll::Ready(Err(PixelReadError::Failed(error.to_string())));
            }
            let mut tile = self.tiles.remove(index).expect("the mapped tile exists");
            tile.answered = None;
            let data = match tile.buffer.slice(..).get_mapped_range() {
                Ok(data) => data,
                Err(error) => {
                    return Poll::Ready(Err(PixelReadError::Failed(error.to_string())));
                }
            };
            place_tile(&mut self.rgba, self.layout.size[0], &tile, &data);
            drop(data);
            tile.buffer.unmap();
            self.completed += 1;
            (self.progress)(self.completed, self.total);
        }
        self.schedule();
        Poll::Ready(Ok(self.completed == self.total))
    }

    /// `schedule` submits tiles until the readback queue reaches its fixed capacity.
    fn schedule(&mut self) {
        while self.tiles.len() < MAX_IN_FLIGHT_READBACKS && self.next < self.total {
            let x = (self.next as u32 % self.columns) * self.side;
            let y = (self.next as u32 / self.columns) * self.side;
            let size = [
                self.side.min(self.layout.size[0] - x),
                self.side.min(self.layout.size[1] - y),
            ];
            let moved = moved_by(&self.display_list, [x, y]);
            let tile = copy_tile(
                &mut self.renderer.lock().expect("renderer lock poisoned"),
                &self.device,
                &self.queue,
                &moved,
                [x, y],
                size,
                self.clear,
            );
            self.tiles.push_back(tile);
            self.next += 1;
        }
    }

    /// `finish` turns the assembled premultiplied bytes into straight-alpha pixels.
    fn finish(mut self) -> PixelBuffer {
        unpremultiply(&mut self.rgba);
        PixelBuffer::new(self.layout, self.rgba).expect("the output fills its layout")
    }
}

/// `copy_tile` renders one display-list tile and requests its staging buffer readback.
fn copy_tile(
    renderer: &mut RendererCore,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    dl: &DisplayList,
    origin: [u32; 2],
    size: [u32; 2],
    clear: Option<Color>,
) -> TileCopy {
    let offscreen = Offscreen::new(device, size);
    renderer.render(dl, &offscreen.target(clear));
    let row_bytes = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("valo.export.readback"),
        size: u64::from(row_bytes) * u64::from(size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        offscreen.texture().as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
    let (answer, answered) = oneshot::channel();
    encoder.map_buffer_on_submit(&buffer, wgpu::MapMode::Read, .., move |mapped| {
        let _ = answer.send(mapped);
    });
    queue.submit(std::iter::once(encoder.finish()));
    TileCopy {
        origin,
        size,
        row_bytes,
        buffer,
        answered: Some(answered),
    }
}

/// `TileCopy` is one tile's pixels in a buffer, its rows padded to `row_bytes`, with the answer
/// to its mapping until it comes.
struct TileCopy {
    origin: [u32; 2],
    size: [u32; 2],
    row_bytes: u32,
    buffer: wgpu::Buffer,
    answered: Option<oneshot::Receiver<Result<(), wgpu::BufferAsyncError>>>,
}

/// `checked_layout` is the layout of `size`'s packed pixels, refusing a size with none or with
/// more than one buffer holds.
fn checked_layout(size: [u32; 2]) -> Result<PixelLayout, PixelReadError> {
    if size.contains(&0) {
        return Err(PixelReadError::Empty);
    }
    let layout = PixelLayout::packed(size, PixelFormat::Rgba8, AlphaType::Straight);
    layout.byte_len().map_err(|_| PixelReadError::TooLarge)?;
    Ok(layout)
}

/// `moved_by` is `dl` drawn `origin` pixels up and to the left: the part of it a tile at
/// `origin` shows, moved by whole pixels so neighbouring tiles agree.
fn moved_by(dl: &Arc<DisplayList>, origin: [u32; 2]) -> DisplayList {
    let mut b = DisplayListBuilder::new();
    b.translate(-(origin[0] as f32), -(origin[1] as f32));
    b.draw_display_list(dl);
    b.build()
}

/// `place_tile` copies a tile's padded rows into the whole picture's packed rows.
fn place_tile(rgba: &mut [u8], width: u32, tile: &TileCopy, data: &[u8]) {
    let row = tile.size[0] as usize * 4;
    for y in 0..tile.size[1] as usize {
        let from = y * tile.row_bytes as usize;
        let to = ((tile.origin[1] as usize + y) * width as usize + tile.origin[0] as usize) * 4;
        rgba[to..to + row].copy_from_slice(&data[from..from + row]);
    }
}

/// `unpremultiply` converts premultiplied RGBA8 pixels to straight alpha in place.
///
/// Fully transparent pixels are set to transparent black because their color
/// channels are undefined. `pixels` must contain complete four-byte pixels;
/// any trailing bytes are left unchanged.
pub fn unpremultiply(pixels: &mut [u8]) {
    for px in pixels.as_chunks_mut::<4>().0 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Paint;
    use std::time::{Duration, Instant};
    use valo_dl::Shader;
    use valo_geometry::{Point, Rect};

    fn context(test: &str) -> Option<Context> {
        let Some((device, queue)) = valo_harness::headless_device() else {
            eprintln!("SKIP {test}: no GPU adapter");
            return None;
        };
        Some(Context::new(device, queue))
    }

    /// `answer` waits for the device, then takes the request's answer.
    fn answer<F: FnMut(usize, usize) + 'static>(
        context: &Context,
        request: PixelRequest<F>,
    ) -> Result<PixelBuffer, PixelReadError> {
        let mut request = Box::pin(request);
        let mut waiting = task::Context::from_waker(task::Waker::noop());
        let started = Instant::now();
        loop {
            context.check_requests().unwrap();
            if let Poll::Ready(answered) = request.as_mut().poll(&mut waiting) {
                return answered;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "never answered");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// `scene` is a gradient under a translucent circle and a turned square, so a seam or a
    /// misplaced tile shows in its pixels.
    fn scene() -> Arc<DisplayList> {
        let mut b = DisplayListBuilder::new();
        b.draw_rect(
            Rect::new(0.0, 0.0, 300.0, 200.0),
            &Paint::from_shader(Shader::linear(
                Point::new(0.0, 0.0),
                Point::new(300.0, 200.0),
                Color::rgb(0.9, 0.2, 0.1),
                Color::rgb(0.1, 0.3, 0.9),
            )),
        );
        b.draw_circle(
            Point::new(130.0, 90.0),
            70.0,
            &Paint::from_color(Color::rgba(1.0, 1.0, 0.0, 0.5)),
        );
        b.translate(220.0, 140.0);
        b.rotate(0.6);
        b.draw_rect(
            Rect::new(-40.0, -40.0, 80.0, 80.0),
            &Paint::from_color(Color::rgb(0.0, 0.6, 0.3)),
        );
        Arc::new(b.build())
    }

    #[test]
    fn tiles_join_into_the_picture_drawn_whole() {
        let Some(mut context) = context("tiles_join_into_the_picture_drawn_whole") else {
            return;
        };
        let (dl, size) = (scene(), [300, 200]);
        let mut drawn = |side| {
            let request = context.request_pixels_with_side(
                &dl,
                size,
                Some(Color::WHITE),
                side,
                Box::new(|_, _| {}),
            );
            answer(&context, request).unwrap()
        };
        let (whole, tiled) = (drawn(1024), drawn(64));
        assert_eq!(tiled.layout(), whole.layout());
        assert!(
            tiled.bytes() == whole.bytes(),
            "tiles of 64 px show the picture drawn whole"
        );
    }

    #[test]
    fn a_request_reports_each_mapped_tile_until_its_single_buffer_is_ready() {
        fn assert_send<T: Send>() {}
        assert_send::<Context>();
        assert_send::<PixelRequest>();
        use std::cell::RefCell;
        use std::rc::Rc;
        let Some(mut context) =
            context("a_request_reports_each_mapped_tile_until_its_single_buffer_is_ready")
        else {
            return;
        };
        let progress = Rc::new(RefCell::new(Vec::new()));
        let observed = progress.clone();
        let request = context.request_pixels_with_side(
            &scene(),
            [300, 200],
            Some(Color::WHITE),
            64,
            Box::new(move |completed, total| {
                observed.borrow_mut().push((completed, total));
            }),
        );
        let pixels = answer(&context, request).unwrap();
        assert_eq!(pixels.layout().size, [300, 200]);
        assert_eq!(
            *progress.borrow(),
            (1..=20)
                .map(|completed| (completed, 20))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn pixels_come_back_with_straight_alpha() {
        let Some(mut context) = context("pixels_come_back_with_straight_alpha") else {
            return;
        };
        let mut b = DisplayListBuilder::new();
        b.draw_rect(
            Rect::new(0.0, 0.0, 4.0, 4.0),
            &Paint::from_color(Color::rgba(1.0, 0.0, 0.0, 0.5)),
        );
        let request = context.request_pixels(&Arc::new(b.build()), [4, 4], None);
        let pixels = answer(&context, request).unwrap();
        assert_eq!(
            pixels.layout(),
            PixelLayout::packed([4, 4], PixelFormat::Rgba8, AlphaType::Straight)
        );
        assert_eq!(&pixels.bytes()[..4], &[255, 0, 0, 128]);
    }

    #[test]
    fn a_request_is_answered_through_check_requests() {
        let Some(mut context) = context("a_request_is_answered_through_check_requests") else {
            return;
        };
        let mut request = context.request_pixels(&scene(), [300, 200], None);
        let mut waiting = task::Context::from_waker(task::Waker::noop());
        assert!(
            Pin::new(&mut request).poll(&mut waiting).is_pending(),
            "nothing answers before the device is checked"
        );
        let started = Instant::now();
        let answered = loop {
            context.check_requests().unwrap();
            if let Poll::Ready(answered) = Pin::new(&mut request).poll(&mut waiting) {
                break answered;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "never answered");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(answered.unwrap().layout().size, [300, 200]);
    }

    #[test]
    fn an_empty_picture_is_refused() {
        let Some(mut context) = context("an_empty_picture_is_refused") else {
            return;
        };
        let request = context.request_pixels(&scene(), [0, 10], None);
        assert_eq!(
            pollster::block_on(request).err(),
            Some(PixelReadError::Empty)
        );
    }

    #[test]
    fn unpremultiply_inverts_premultiplication() {
        // (128, 0, 64, 128) premul → straight: 64·255/128 = 127.5 → 128.
        let mut px = vec![128, 0, 64, 128, 10, 20, 30, 0];
        unpremultiply(&mut px);
        assert_eq!(&px[..4], &[255, 0, 128, 128]);
        assert_eq!(&px[4..], &[0, 0, 0, 0], "transparent pixels zero out");
    }

    #[test]
    fn a_size_with_no_pixels_is_refused() {
        assert_eq!(checked_layout([0, 10]), Err(PixelReadError::Empty));
        assert_eq!(
            checked_layout([u32::MAX, u32::MAX]),
            Err(PixelReadError::TooLarge)
        );
    }
}
