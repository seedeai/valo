use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use valo_geometry::{FillRule, Matrix, Path, Rect};

use crate::{Image, Paint, Sampling};

/// `ClipOp` controls how a clip shape changes the current clip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ClipOp {
    /// `Intersect` retains pixels inside the clip shape.
    #[default]
    Intersect,
    /// `Difference` retains pixels outside the clip shape.
    Difference,
}

/// `Op` is one recorded display-list command.
///
/// Draw and clip operations include the bounds and ordering metadata resolved
/// by [`crate::DisplayListBuilder`] at record time.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Op {
    Save,
    /// Open an offscreen layer scope, closed by the matching `Restore`. All
    /// oracle fields are backpatched when the scope closes — still record
    /// time; replay reads them, never counts.
    SaveLayer {
        paint: Paint,
        /// Set = this layer is a MASK: its composite converts
        /// the texture to COVERAGE (luminance or alpha) and multiplies the
        /// enclosing layer by it (DstIn over the whole enclosing extent, so
        /// content outside the mask's ink disappears).
        mask_composite: Option<MaskKind>,
        /// Children's union bounds ∩ clip ∩ hint, list-root space — the
        /// layer texture's size and the composite quad's rect.
        scope_bounds: Rect,
        /// Slot count when the scope opened. Children continue the SAME
        /// depth line as the parent (Impeller's global numbering,
        /// `Canvas::current_depth_`); the layer's own pass rebases by
        /// subtracting it. Impeller records one span (`total_content_depth`)
        /// and counts during replay; valo's replay never counts, so it
        /// records both ends of the span instead.
        base_slot: u32,
        /// The composite draw's slot — next on the same line, after the
        /// children's span (so the span is composite_slot - base_slot - 1).
        composite_slot: u32,
        /// Alpha-linear + pairwise-disjoint children and a plain-alpha
        /// composite: replay may skip the texture entirely and let the
        /// alpha ride each child at its own slot (Impeller's opacity
        /// peephole: elision changes nothing about depth).
        can_elide: bool,
        /// Set = the layer OPENS pre-filled with a blur of everything
        /// already painted beneath it (σ in local units; replay scales it
        /// into device px). Children paint over that glass, and the
        /// composite applies group alpha to blur + children as one image —
        /// Flutter's `saveLayer(bounds, paint, backdrop)`. A backdrop layer
        /// never elides: the seed needs a texture.
        backdrop_sigma: Option<f32>,
        /// Tiles sharing one key reuse the FIRST tile's blur (and see the
        /// scene as of that tile). Meaningful only with `backdrop_sigma`.
        backdrop_key: Option<u64>,
    },
    Restore,
    /// Appends to the current transform (canvas semantics: applies to
    /// subsequently drawn geometry first).
    Transform(Matrix),
    DrawRect {
        rect: Rect,
        paint: Paint,
        bounds: Rect,
        slot: u32,
    },
    DrawPath {
        path: Arc<Path>,
        fill_rule: FillRule,
        paint: Paint,
        bounds: Rect,
        slot: u32,
    },
    /// A mask-blurred solid (r)rect in CLOSED FORM — one draw, no filter
    /// passes; why a box shadow costs one quad (Impeller's
    /// SolidRRectBlurContents). Recorded when a solid paint has `mask_blur`;
    /// `radii` are per corner, clockwise from top-left ([0.0; 4] = sharp).
    RRectBlur {
        rect: Rect,
        radii: [f32; 4],
        paint: Paint,
        bounds: Rect,
        slot: u32,
    },
    /// Depth-buffer clip (Impeller's "new clips"): the renderer
    /// stencils the shape, then writes a depth CEILING at `expiry_slot` —
    /// Intersect ceilings the exterior, Difference the interior. Draws below
    /// the ceiling fail the depth test there; draws after the scope's restore
    /// sit above it. Expiry is auto — restore renders nothing.
    ClipPath {
        path: Arc<Path>,
        fill_rule: FillRule,
        op: ClipOp,
        /// The slot of the restore that ends this clip's scope (backpatched
        /// by the builder when the scope closes — still record-time).
        expiry_slot: u32,
    },
    /// Textured quad: `src` (texture px) → `dst` (local space). Sampling
    /// picks filter/tiling; paint contributes tint (color as multiplier),
    /// alpha, and blend.
    DrawImage {
        image: Image,
        src: Rect,
        dst: Rect,
        sampling: Sampling,
        paint: Paint,
        bounds: Rect,
        slot: u32,
    },
    /// Positioned glyphs from a laid-out paragraph — the TextFrame analog
    /// (font id + glyph ids + positions, so this crate never depends on
    /// the text stack). One op per placed run; `y` sits on the
    /// baseline; the renderer picks bitmap/SDF/path per transform.
    GlyphRun {
        /// The font INSTANCE, carried by value to raster (Skia: text
        /// blobs hold `sk_sp<SkTypeface>` — nothing is registered
        /// renderer-side). Serialization keeps only the raster identity.
        #[cfg_attr(feature = "serde", serde(serialize_with = "serialize_font_uid"))]
        font: std::sync::Arc<valo_text::Font>,
        size: f32,
        /// Blend/alpha/mask-blur apply like any draw; `paint.color` tints
        /// mask glyphs (color glyphs keep their palette, alpha only).
        paint: Paint,
        glyphs: Arc<Vec<GlyphPos>>,
        bounds: Rect,
        slot: u32,
    },
    /// Embed another list by reference — the retained-layer composition op.
    DrawDisplayList {
        list: Arc<DisplayList>,
        bounds: Rect,
        /// Child slots are child-relative; replay offsets them by this.
        base_slot: u32,
        /// The embedder judges this subtree stable and heavy enough to
        /// raster-cache (policy is the caller's; admission stays in the
        /// renderer).
        cache: bool,
    },
}

/// `MaskKind` controls how a mask layer converts pixels into coverage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum MaskKind {
    /// `Luminance` derives coverage from premultiplied pixel luminance.
    Luminance,
    /// `Alpha` uses only the pixel alpha channel as coverage.
    Alpha,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// `DisplayList` is an immutable recording of drawing commands.
///
/// Display lists are GPU-free, thread-safe, and nestable. Wrap a list in
/// [`Arc`] to share or replay it without copying its commands.
#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct DisplayList {
    id: u64,
    pub(crate) ops: Vec<Op>,
    /// Union of all draw bounds, list-root space. `None` = draws nothing.
    pub(crate) bounds: Option<Rect>,
    /// Draw commands in this list, nested lists included.
    pub(crate) draw_count: u32,
    /// Depth slots consumed when replayed (draws + clip-scope restores),
    /// nested lists included — the renderer derives its z quantum from this.
    pub(crate) depth_slots: u32,
    /// Per shared backdrop key: the union of the recorded regions of the
    /// backdrop layers carrying it — the first one replayed blurs the whole
    /// union once, and the rest reuse that blur.
    pub(crate) backdrop_groups: Vec<BackdropGroup>,
    /// Backdrop reads when replayed, shared or not, nested lists included.
    /// A rasterized copy of such a list would freeze what it read.
    pub(crate) backdrop_reads: u32,
}

/// `GlyphPos` identifies and positions one glyph within a glyph run.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct GlyphPos {
    /// `id` is the glyph identifier in the run's font.
    pub id: u32,
    /// `x` is the glyph's local horizontal position in pixels.
    pub x: f32,
    /// `y` is the glyph's local baseline position in pixels.
    pub y: f32,
}

/// `BackdropGroup` summarizes regions sharing one backdrop-blur key.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct BackdropGroup {
    /// `key` identifies the shared backdrop group.
    pub key: u64,
    /// `union_bounds` encloses every region in the group.
    pub union_bounds: Rect,
    /// `sigma` is the shared blur radius when every region agrees.
    ///
    /// It is `None` when regions with this key use different radii and cannot
    /// share one blur result.
    pub sigma: Option<f32>,
}

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

impl DisplayList {
    pub(crate) fn new(
        ops: Vec<Op>,
        bounds: Option<Rect>,
        draw_count: u32,
        depth_slots: u32,
        backdrop_groups: Vec<BackdropGroup>,
        backdrop_reads: u32,
    ) -> Self {
        Self {
            id: next_id(),
            ops,
            bounds,
            draw_count,
            depth_slots,
            backdrop_groups,
            backdrop_reads,
        }
    }

    /// `id` returns the process-unique identity of this live display list.
    ///
    /// Deserialization creates a fresh identity; equal content does not imply
    /// equal identity.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// `ops` returns the recorded commands in replay order.
    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    /// `bounds` returns the union of visible draw bounds in list coordinates.
    ///
    /// It returns `None` when the list draws nothing.
    pub fn bounds(&self) -> Option<Rect> {
        self.bounds
    }

    /// `draw_count` returns the number of draws, including nested lists.
    pub fn draw_count(&self) -> u32 {
        self.draw_count
    }

    /// `depth_slots` returns the ordering slots required to replay this list.
    pub fn depth_slots(&self) -> u32 {
        self.depth_slots
    }

    /// `backdrop_reads` counts backdrop reads when replayed, shared or not,
    /// nested lists included.
    pub fn backdrop_reads(&self) -> u32 {
        self.backdrop_reads
    }

    /// `backdrop_group` returns the group recorded for `key`, if present.
    pub fn backdrop_group(&self, key: u64) -> Option<&BackdropGroup> {
        self.backdrop_groups.iter().find(|g| g.key == key)
    }
}

#[cfg(feature = "serde")]
fn serialize_font_uid<S: serde::Serializer>(
    font: &std::sync::Arc<valo_text::Font>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(font.uid().0)
}
