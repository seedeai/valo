//! The export path: draw a display list into pixels of any size and ask the GPU for them back,
//! as STRAIGHT-alpha RGBA8 — what encoders and hosts expect. Internally valo is premultiplied
//! end to end (goldens stay premultiplied for byte-exactness); unpremultiplying is an
//! export-boundary concern only.
//!
//! Asking follows wgpu's `map_async` and Skia's `asyncRescaleAndReadPixels`: the request goes to
//! the GPU at once, and its answer is delivered when the device is polled — by the browser on the
//! web, by the host natively through [`Context::check_requests`], as Skia's hosts call
//! `checkAsyncWorkCompletion`.

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

    /// `check_requests` delivers the requests the GPU has answered, without waiting. It fails
    /// only when the device is lost.
    pub fn check_requests(&self) -> Result<(), PixelReadError> {
        self.device()
            .poll(wgpu::PollType::Poll)
            .map(|_| ())
            .map_err(|e| PixelReadError::Failed(e.to_string()))
    }

    /// `copy_in_tiles` draws `dl` in tiles at most `side` pixels a side, each copied into a
    /// buffer the CPU will map.
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

    /// `copy_tile` draws `dl` into a `size` texture, copies it into a buffer and asks for the
    /// buffer mapped, in one submission; the tile sits at `origin` in the whole picture.
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

/// `Copies` is a picture drawn and copied tile by tile, its buffers asked for.
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

    /// `join` copies the mapped tiles into one picture with straight alpha.
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
        context
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        pollster::block_on(request)
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
            let copies = context.copy_in_tiles(&dl, size, Some(Color::WHITE), side);
            answer(
                &context,
                PixelRequest {
                    copies: Some(copies),
                },
            )
            .unwrap()
        };
        let (whole, tiled) = (drawn(1024), drawn(64));
        assert_eq!(tiled.layout(), whole.layout());
        assert!(
            tiled.bytes() == whole.bytes(),
            "tiles of 64 px show the picture drawn whole"
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
