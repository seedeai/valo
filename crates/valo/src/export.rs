//! The export path: draw a display list into pixels of any size and ask the GPU for them back,
//! as STRAIGHT-alpha RGBA8 — what encoders and hosts expect. Internally valo is premultiplied
//! end to end (goldens stay premultiplied for byte-exactness); unpremultiplying is an
//! export-boundary concern only.
//!
//! Asking follows wgpu's `map_async` and Skia's `asyncRescaleAndReadPixels`: the request goes to
//! the GPU at once, and its answer is delivered when the device is polled — by the browser on the
//! web, by the host natively through [`Context::check_requests`], as Skia's hosts call
//! `checkAsyncWorkCompletion`.
//! Bounded exports use [`Context::poll_pixel_export`] instead: the context submits later
//! tiles when the host advances the export, without giving the request renderer ownership.

use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Poll};

use futures_channel::oneshot;
use valo_dl::{DisplayList, DisplayListBuilder};
use valo_geometry::Color;
use valo_renderer::{AlphaType, PixelBuffer, PixelFormat, PixelLayout};

use crate::surface::Offscreen;
use crate::Context;

/// `MAX_IN_FLIGHT_READBACKS` bounds staging buffers held by one `PixelExport`.
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
    ) -> PixelRequest {
        let side = self.device().limits().max_texture_dimension_2d;
        PixelRequest {
            copies: Some(self.copy_in_tiles(dl, size, clear, side)),
        }
    }

    /// `request_pixel_export` starts a pixel export with bounded GPU readbacks.
    ///
    /// Call [`Self::poll_pixel_export`] to read completed tiles and submit the next ones.
    /// At most two tile readbacks are in flight, and [`PixelExport::progress`] counts tiles
    /// copied into the final buffer. The export borrows nothing between polls, so this context
    /// can keep drawing frames. Invalid dimensions are refused before GPU work is submitted.
    ///
    /// ```no_run
    /// # use std::{sync::Arc, task::{Context as TaskContext, Poll, Waker}};
    /// # use valo::{Context, DisplayList, PixelBuffer, PixelExport, PixelReadError};
    /// # fn start(context: &mut Context, list: &Arc<DisplayList>)
    /// #     -> Result<PixelExport, PixelReadError> {
    /// let export = context.request_pixel_export(list, [4096, 4096], None)?;
    /// # Ok(export)
    /// # }
    /// // Call this turn again when the host's waker is notified.
    /// fn export_turn(
    ///     context: &mut Context,
    ///     export: &mut PixelExport,
    ///     waker: &Waker,
    /// ) -> Result<Poll<Result<PixelBuffer, PixelReadError>>, PixelReadError> {
    ///     #[cfg(not(target_arch = "wasm32"))]
    ///     context.check_requests()?; // Also pump this in the native event loop.
    ///     let result = context.poll_pixel_export(export, &mut TaskContext::from_waker(waker));
    ///     let (completed, total) = export.progress();
    ///     // Update the host's progress UI; stop polling when `result` is `Ready`.
    ///     Ok(result)
    /// }
    /// ```
    pub fn request_pixel_export(
        &mut self,
        dl: &Arc<DisplayList>,
        size: [u32; 2],
        clear: Option<Color>,
    ) -> Result<PixelExport, PixelReadError> {
        let side = self.device().limits().max_texture_dimension_2d;
        self.start_pixel_export(dl, size, clear, side)
    }

    /// `poll_pixel_export` advances an export and returns its assembled straight-alpha pixels.
    ///
    /// Use the context that started the export, keeping its rendering settings unchanged.
    /// Only this call borrows the context; no renderer is stored or shared by the export.
    /// Native hosts must also call [`Self::check_requests`] to deliver GPU completions; on the
    /// web the browser delivers them. On both platforms the host must poll again when woken:
    /// a GPU callback alone cannot submit later tiles. Passing a different context violates
    /// this contract and can cause GPU validation errors.
    ///
    /// # Panics
    ///
    /// Panics if the export has already returned `Ready`, including an error.
    pub fn poll_pixel_export(
        &mut self,
        export: &mut PixelExport,
        cx: &mut task::Context<'_>,
    ) -> Poll<Result<PixelBuffer, PixelReadError>> {
        assert!(
            !export.finished,
            "a finished PixelExport isn't polled again"
        );
        let Poll::Ready(result) = self.advance_pixel_export(export, cx) else {
            return Poll::Pending;
        };
        export.finished = true;
        export.tiles.clear();
        let mut rgba = std::mem::take(&mut export.rgba);
        Poll::Ready(result.map(|()| {
            unpremultiply(&mut rgba);
            PixelBuffer::new(export.layout, rgba).expect("the output fills its layout")
        }))
    }

    /// `start_pixel_export` submits the first bounded batch with a maximum tile edge of `side`.
    fn start_pixel_export(
        &mut self,
        dl: &Arc<DisplayList>,
        size: [u32; 2],
        clear: Option<Color>,
        side: u32,
    ) -> Result<PixelExport, PixelReadError> {
        let mut export = PixelExport::new(dl.clone(), size, clear, side)?;
        self.submit_export_tiles(&mut export);
        Ok(export)
    }

    /// `advance_pixel_export` collects completed tiles and refills the bounded readback queue.
    fn advance_pixel_export(
        &mut self,
        export: &mut PixelExport,
        cx: &mut task::Context<'_>,
    ) -> Poll<Result<(), PixelReadError>> {
        if let Err(error) = export.read_ready_tiles(cx) {
            return Poll::Ready(Err(error));
        }
        if export.completed == export.total {
            return Poll::Ready(Ok(()));
        }
        if self.submit_export_tiles(export) {
            // Newly submitted receivers need a poll to register the host's waker.
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }

    /// `submit_export_tiles` fills available readback slots without retaining the renderer.
    fn submit_export_tiles(&mut self, export: &mut PixelExport) -> bool {
        let before = export.next;
        while export.tiles.len() < MAX_IN_FLIGHT_READBACKS && export.next < export.total {
            let origin = [
                (export.next % export.columns as usize) as u32 * export.side,
                (export.next / export.columns as usize) as u32 * export.side,
            ];
            let size = [
                export.side.min(export.layout.size[0] - origin[0]),
                export.side.min(export.layout.size[1] - origin[1]),
            ];
            let moved = moved_by(&export.display_list, origin);
            export
                .tiles
                .push_back(self.copy_tile(&moved, origin, size, export.clear));
            export.next += 1;
        }
        export.next != before
    }

    /// `check_requests` delivers the requests the GPU has answered, without waiting. It fails
    /// only when the device is lost.
    pub fn check_requests(&self) -> Result<(), PixelReadError> {
        self.device()
            .poll(wgpu::PollType::Poll)
            .map(|_| ())
            .map_err(|e| PixelReadError::Failed(e.to_string()))
    }

    /// `copy_in_tiles` submits an ordinary request's tiles eagerly, as before.
    fn copy_in_tiles(
        &mut self,
        dl: &Arc<DisplayList>,
        size: [u32; 2],
        clear: Option<Color>,
        side: u32,
    ) -> Result<Copies, PixelReadError> {
        let layout = checked_layout(size)?;
        if size[0] <= side && size[1] <= side {
            let tiles = vec![self.copy_tile(dl, [0, 0], size, clear)];
            return Ok(Copies { layout, tiles });
        }
        let mut tiles = Vec::new();
        for y in (0..size[1]).step_by(side as usize) {
            for x in (0..size[0]).step_by(side as usize) {
                let tile = [side.min(size[0] - x), side.min(size[1] - y)];
                let moved = moved_by(dl, [x, y]);
                tiles.push(self.copy_tile(&moved, [x, y], tile, clear));
            }
        }
        Ok(Copies { layout, tiles })
    }

    /// `copy_tile` renders a tile and asks for its padded staging buffer in one submission.
    fn copy_tile(
        &mut self,
        dl: &DisplayList,
        origin: [u32; 2],
        size: [u32; 2],
        clear: Option<Color>,
    ) -> TileCopy {
        let offscreen = Offscreen::new(self.device(), size);
        self.render(dl, &offscreen.target(clear));
        let row_bytes = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("valo.export.readback"),
            size: u64::from(row_bytes) * u64::from(size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
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
        self.queue_handle()
            .submit(std::iter::once(encoder.finish()));
        TileCopy {
            origin,
            size,
            row_bytes,
            buffer,
            answered: Some(answered),
        }
    }
}

/// `PixelRequest` is pixels asked of the GPU by [`Context::request_pixels`], finishing once
/// [`Context::check_requests`] delivers them natively, or the browser does on the web.
pub struct PixelRequest {
    copies: Option<Result<Copies, PixelReadError>>,
}

impl Future for PixelRequest {
    type Output = Result<PixelBuffer, PixelReadError>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Some(Ok(copies)) = &mut this.copies {
            match copies.answers(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => {
                    this.copies = None;
                    return Poll::Ready(Err(error));
                }
                Poll::Ready(Ok(())) => {}
            }
        }
        let copies = this
            .copies
            .take()
            .expect("a finished PixelRequest isn't polled again");
        Poll::Ready(copies.and_then(Copies::join))
    }
}

/// `Copies` is an ordinary request's eagerly submitted tiles.
struct Copies {
    layout: PixelLayout,
    tiles: Vec<TileCopy>,
}

impl Copies {
    /// `answers` is ready once every tile's buffer is mapped, or one failed.
    fn answers(&mut self, cx: &mut task::Context<'_>) -> Poll<Result<(), PixelReadError>> {
        for tile in &mut self.tiles {
            let Some(answered) = &mut tile.answered else {
                continue;
            };
            let mapped = match Pin::new(answered).poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(mapped)) => mapped,
                Poll::Ready(Err(_)) => Err(wgpu::BufferAsyncError),
            };
            tile.answered = None;
            if let Err(error) = mapped {
                return Poll::Ready(Err(PixelReadError::Failed(error.to_string())));
            }
        }
        Poll::Ready(Ok(()))
    }

    /// `join` assembles mapped tiles into a single straight-alpha pixel buffer.
    fn join(self) -> Result<PixelBuffer, PixelReadError> {
        let mut rgba = vec![
            0;
            self.layout
                .byte_len()
                .map_err(|_| PixelReadError::TooLarge)?
        ];
        for tile in &self.tiles {
            let data = tile
                .buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|e| PixelReadError::Failed(e.to_string()))?;
            place_tile(&mut rgba, self.layout.size[0], tile, &data);
        }
        unpremultiply(&mut rgba);
        Ok(PixelBuffer::new(self.layout, rgba).expect("the tiles fill the layout"))
    }
}

/// `PixelExport` is a bounded pixel readback advanced by its rendering context.
///
/// It holds the display list, at most two staging buffers, and one full-size output buffer,
/// but no renderer. [`Context::poll_pixel_export`] borrows the context only while submitting
/// or collecting work, leaving it available for ordinary drawing between polls. This type
/// is not a `Future`: even on the web, later tiles need the host to drive the context.
/// Dropping the export discards its output and submits no further tiles; already submitted
/// GPU work finishes normally. Progress covers readback only, not encoding or saving.
pub struct PixelExport {
    layout: PixelLayout,
    display_list: Arc<DisplayList>,
    clear: Option<Color>,
    side: u32,
    columns: u32,
    total: usize,
    next: usize,
    completed: usize,
    tiles: VecDeque<TileCopy>,
    rgba: Vec<u8>,
    finished: bool,
}

impl PixelExport {
    /// `progress` returns `(completed_tiles, total_tiles)` copied into the output buffer.
    ///
    /// Counts advance only in [`Context::poll_pixel_export`], after the mapped rows are copied
    /// and their staging buffer is released. `(total, total)` does not include file encoding.
    pub fn progress(&self) -> (usize, usize) {
        (self.completed, self.total)
    }

    /// `new` validates the output and allocates its only assembled pixel buffer.
    fn new(
        display_list: Arc<DisplayList>,
        size: [u32; 2],
        clear: Option<Color>,
        side: u32,
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
        Ok(Self {
            layout,
            display_list,
            clear,
            side,
            columns,
            total,
            next: 0,
            completed: 0,
            tiles: VecDeque::new(),
            rgba,
            finished: false,
        })
    }

    /// `read_ready_tiles` copies mapped readbacks and releases their staging buffers.
    fn read_ready_tiles(&mut self, cx: &mut task::Context<'_>) -> Result<(), PixelReadError> {
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
                    return Err(PixelReadError::Failed(
                        "GPU readback channel closed".to_owned(),
                    ));
                }
            };
            if let Err(error) = mapped {
                return Err(PixelReadError::Failed(error.to_string()));
            }
            let mut tile = self.tiles.remove(index).expect("the mapped tile exists");
            tile.answered = None;
            let data = match tile.buffer.slice(..).get_mapped_range() {
                Ok(data) => data,
                Err(error) => {
                    return Err(PixelReadError::Failed(error.to_string()));
                }
            };
            place_tile(&mut self.rgba, self.layout.size[0], &tile, &data);
            drop(data);
            tile.buffer.unmap();
            self.completed += 1;
        }
        Ok(())
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
    fn answer(context: &Context, request: PixelRequest) -> Result<PixelBuffer, PixelReadError> {
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

    /// `answer_export` drives bounded readbacks and checks progress at each host poll.
    fn answer_export(
        context: &mut Context,
        export: &mut PixelExport,
        mut observe: impl FnMut(usize, usize),
    ) -> Result<PixelBuffer, PixelReadError> {
        let mut waiting = task::Context::from_waker(task::Waker::noop());
        let started = Instant::now();
        loop {
            context.check_requests().unwrap();
            let previous = export.completed;
            let result = context.poll_pixel_export(export, &mut waiting);
            assert!(export.tiles.len() <= MAX_IN_FLIGHT_READBACKS);
            assert!(export.completed >= previous);
            assert!(export.completed - previous <= MAX_IN_FLIGHT_READBACKS);
            let (completed, total) = export.progress();
            assert!(completed <= total);
            observe(completed, total);
            if let Poll::Ready(result) = result {
                assert!(export.tiles.is_empty());
                assert!(export.rgba.is_empty());
                return result;
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
            let request = PixelRequest {
                copies: Some(context.copy_in_tiles(&dl, size, Some(Color::WHITE), side)),
            };
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
    fn bounded_tiles_match_an_eager_request_in_one_output_buffer() {
        let Some(mut context) =
            context("bounded_tiles_match_an_eager_request_in_one_output_buffer")
        else {
            return;
        };
        let dl = scene();
        for (size, clear) in [([300, 200], Some(Color::WHITE)), ([71, 59], None)] {
            let request = context.request_pixels(&dl, size, clear);
            let whole = answer(&context, request).unwrap();
            for side in [64, 1024] {
                let mut export = context.start_pixel_export(&dl, size, clear, side).unwrap();
                let total = export.total;
                assert_eq!(export.progress(), (0, total));
                assert_eq!(export.tiles.len(), MAX_IN_FLIGHT_READBACKS.min(total));
                let allocation = export.rgba.as_ptr();
                let tiled = answer_export(&mut context, &mut export, |_, _| {}).unwrap();
                assert_eq!(
                    tiled.bytes().as_ptr(),
                    allocation,
                    "output is moved, not copied"
                );
                assert_eq!(tiled.layout(), whole.layout());
                assert_eq!(tiled.bytes(), whole.bytes());
                assert_eq!(export.progress(), (total, total));
            }
        }
    }

    #[test]
    fn progress_advances_only_when_the_host_collects_completed_tiles() {
        let Some(mut context) =
            context("progress_advances_only_when_the_host_collects_completed_tiles")
        else {
            return;
        };
        let mut export = context
            .start_pixel_export(&scene(), [300, 200], None, 64)
            .unwrap();
        context
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        assert_eq!(export.progress(), (0, 20));
        assert_eq!(export.next, MAX_IN_FLIGHT_READBACKS);
        let mut progress = Vec::new();
        answer_export(&mut context, &mut export, |completed, total| {
            progress.push((completed, total));
        })
        .unwrap();
        assert_eq!(progress.last(), Some(&(20, 20)));
        assert!(progress
            .iter()
            .any(|&(completed, _)| completed > 0 && completed < 20));
    }

    #[test]
    fn a_context_can_draw_and_finish_an_ordinary_request_between_export_polls() {
        let Some(mut context) =
            context("a_context_can_draw_and_finish_an_ordinary_request_between_export_polls")
        else {
            return;
        };
        let dl = scene();
        let mut export = context
            .start_pixel_export(&dl, [300, 200], None, 64)
            .unwrap();
        let offscreen = Offscreen::new(context.device(), [300, 200]);
        context.render(&dl, &offscreen.target(None));
        let request = context.request_pixels(&dl, [300, 200], None);
        let ordinary = answer(&context, request).unwrap();
        assert_eq!(export.next, MAX_IN_FLIGHT_READBACKS);
        let bounded = answer_export(&mut context, &mut export, |_, _| {}).unwrap();
        assert_eq!(bounded.bytes(), ordinary.bytes());
    }

    #[test]
    fn dropping_an_export_does_not_retain_or_block_the_context() {
        let Some(mut context) = context("dropping_an_export_does_not_retain_or_block_the_context")
        else {
            return;
        };
        let dl = scene();
        let export = context
            .start_pixel_export(&dl, [300, 200], None, 64)
            .unwrap();
        assert_eq!(export.next, MAX_IN_FLIGHT_READBACKS);
        drop(export);
        let request = context.request_pixels(&dl, [300, 200], None);
        assert!(answer(&context, request).is_ok());
    }

    #[test]
    fn a_failed_export_releases_its_buffers_without_reporting_completion() {
        let Some(mut context) =
            context("a_failed_export_releases_its_buffers_without_reporting_completion")
        else {
            return;
        };
        let mut export = context
            .start_pixel_export(&scene(), [300, 200], None, 64)
            .unwrap();
        let (sender, receiver) = oneshot::channel();
        export.tiles[0].answered = Some(receiver);
        drop(sender);
        let mut waiting = task::Context::from_waker(task::Waker::noop());
        let result = context.poll_pixel_export(&mut export, &mut waiting);
        assert!(matches!(
            result,
            Poll::Ready(Err(PixelReadError::Failed(_)))
        ));
        assert!(export.finished);
        assert!(export.tiles.is_empty());
        assert!(export.rgba.is_empty());
        assert_eq!(export.progress(), (0, 20));
    }

    #[test]
    fn refilling_the_queue_wakes_the_host_to_register_new_readbacks() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct WakeCount(AtomicUsize);

        impl task::Wake for WakeCount {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let Some(mut context) =
            context("refilling_the_queue_wakes_the_host_to_register_new_readbacks")
        else {
            return;
        };
        let mut export = context
            .start_pixel_export(&scene(), [300, 200], None, 64)
            .unwrap();
        let count = Arc::new(WakeCount(AtomicUsize::new(0)));
        let waker = task::Waker::from(count.clone());
        let mut waiting = task::Context::from_waker(&waker);
        context
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        assert!(context
            .poll_pixel_export(&mut export, &mut waiting)
            .is_pending());
        assert_eq!(export.progress(), (2, 20));
        assert_eq!(export.next, 4);
        assert!(count.0.load(Ordering::Relaxed) > 0);
        answer_export(&mut context, &mut export, |_, _| {}).unwrap();
        let mut export = context
            .request_pixel_export(&scene(), [4, 4], None)
            .unwrap();
        assert!(context
            .poll_pixel_export(&mut export, &mut waiting)
            .is_pending());
        let before_completion = count.0.load(Ordering::Relaxed);
        context
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        assert!(count.0.load(Ordering::Relaxed) > before_completion);
        answer_export(&mut context, &mut export, |_, _| {}).unwrap();
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn native_contexts_and_both_request_types_remain_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Context>();
        assert_send::<PixelRequest>();
        assert_send::<PixelExport>();
    }

    #[test]
    fn an_ordinary_request_still_outlives_its_context() {
        let Some(mut context) = context("an_ordinary_request_still_outlives_its_context") else {
            return;
        };
        let device = context.device().clone();
        let request = context.request_pixels(&scene(), [300, 200], None);
        drop(context);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        assert!(pollster::block_on(request).is_ok());
    }

    #[test]
    fn invalid_export_dimensions_are_refused_before_submitting_tiles() {
        let Some(mut context) =
            context("invalid_export_dimensions_are_refused_before_submitting_tiles")
        else {
            return;
        };
        assert_eq!(
            context.request_pixel_export(&scene(), [0, 10], None).err(),
            Some(PixelReadError::Empty)
        );
        assert_eq!(
            context
                .request_pixel_export(&scene(), [u32::MAX, u32::MAX], None)
                .err(),
            Some(PixelReadError::TooLarge)
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
