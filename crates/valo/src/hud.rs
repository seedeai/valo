//! A built-in perf overlay: three monospace lines on frosted glass, drawn
//! by the engine itself — any host can drop it on top of a frame to watch
//! timings, workload, and memory live (the debug HUD every renderer grows;
//! this one ships in the box so hosts don't reinvent it).

use valo_dl::{DisplayListBuilder, Paint};
use valo_geometry::{Color, Rect};
use valo_renderer::{MemoryReport, RenderStats};
use valo_text::{FontCollection, ParagraphBuilder, TextStyle};

use crate::text::DrawParagraphExt;

/// `Hud` records renderer statistics as an in-frame overlay.
///
/// Use a registered monospace font so its columns remain aligned.
pub struct Hud {
    family: String,
    size: f32,
}

impl Hud {
    /// `new` creates a HUD using a registered monospace font family.
    pub fn new(family: impl Into<String>) -> Self {
        Self {
            family: family.into(),
            size: 15.0,
        }
    }

    /// `with_size` sets the text size in pixels.
    ///
    /// The default is 15 pixels.
    pub fn with_size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// `draw` records the overlay at the top of a frame.
    ///
    /// `note` adds host state to the first line, and `memory` adds an optional
    /// resource line. The returned height can position other UI below the HUD.
    pub fn draw(
        &self,
        b: &mut DisplayListBuilder,
        fonts: &mut FontCollection,
        stats: &RenderStats,
        memory: Option<&MemoryReport>,
        note: &str,
        width: f32,
    ) -> f32 {
        let mut text = format!("{}\n{}", frame_line(stats, note), work_line(stats));
        if let Some(memory) = memory {
            text.push('\n');
            text.push_str(&memory_line(memory));
        }
        let mut p = ParagraphBuilder::new(fonts);
        p.add_text(
            &text,
            &TextStyle::new(&self.family, self.size, Color::rgb(0.92, 0.94, 1.0)),
        );
        let mut p = p.build();
        p.layout(f32::INFINITY);

        let pad = self.size * 0.7;
        let height = p.height() + pad * 2.0;
        let strip = Rect::new(0.0, 0.0, width, height);
        b.backdrop_blur(strip, 6.0);
        b.draw_rect(
            strip,
            &Paint::from_color(Color::rgba(0.05, 0.05, 0.08, 0.4)),
        );
        b.draw_paragraph(&p, (pad, pad));
        height
    }
}

fn frame_line(s: &RenderStats, note: &str) -> String {
    format!(
        "{note:10} cpu {:>6.2} ms (plan {:>5.2} + encode {:>5.2})   gpu {:>6.2} ms",
        s.cpu_ms, s.plan_ms, s.encode_ms, s.gpu_ms,
    )
}

fn work_line(s: &RenderStats) -> String {
    format!(
        "draws {:>6} (culled {:>6})   calls {:>6}   passes {:>3}   filters {:>3}   snapshots {:>2}   cache {:>3}q/{}f   text {}/{}/{}   rasters {:>4}   held {:>4}   gc {}",
        s.draws,
        s.culled,
        s.draw_calls,
        s.render_passes,
        s.filter_passes,
        s.snapshots,
        s.raster_quads,
        s.raster_fills,
        s.text_tiers[0],
        s.text_tiers[1],
        s.text_tiers[2],
        s.glyph_rasters,
        s.held_rasters,
        s.atlas_gcs,
    )
}

fn memory_line(m: &MemoryReport) -> String {
    format!(
        "mem {:>6.1} MB   atlas {}p/{}e mask, {}p color   targets {:>3}   rasters {:>3} ({:>5.1} MB)   host {:>2} blocks   contours {:>4}   glyph paths {:>4}",
        m.total_bytes() as f64 / 1e6,
        m.atlas[0].pages,
        m.atlas[0].entries,
        m.atlas[1].pages,
        m.targets.count,
        m.raster_cache.count,
        m.raster_cache.bytes as f64 / 1e6,
        m.host_buffer.count,
        m.contours.count,
        m.glyph_paths.count,
    )
}
