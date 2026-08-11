use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use valo_geometry::{FillRule, Matrix, Path, Rect};

use crate::{Image, Paint, Sampling};

/// How a clip combines with the scene: keep the inside, or keep the outside.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ClipOp {
    #[default]
    Intersect,
    Difference,
}

/// One recorded command. Draw/clip ops carry the **record-time oracle** inline:
/// device bounds (list-root space, already intersected with the clip stack) and
/// the depth slot the builder assigned — the renderer replays without counting
/// or re-deriving anything, and clips arrive KNOWING their expiry — the
/// renderer never patches anything at restore.
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
    /// Blur what's ALREADY on the target under `rect`, composite the blurred
    /// tile back, keep drawing on top — frosted glass.
    /// Tiles sharing `shared_key` blur ONCE over their union region; later
    /// tiles show the scene as of the first (Flutter's backdropKey trade).
    BackdropBlur {
        rect: Rect,
        /// σ in local units at record; replay scales it into device px.
        sigma: f32,
        shared_key: Option<u64>,
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
        bounds: Rect,
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

/// How a mask layer's texture becomes coverage at composite (SVG's
/// mask-type): luminance of the premultiplied pixels (the SVG default) or
/// their alpha alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum MaskKind {
    Luminance,
    Alpha,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// An immutable, retained recording. Cheap to clone (`Arc` it), free-threaded,
/// nestable. `id` is process-unique and stable for the value's lifetime —
/// identity-keyed caches (future raster cache, damage diffing) hang off it.
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
    /// Per shared backdrop key: union of the tiles' recorded bounds and the
    /// tile count — the first tile replayed blurs the whole union once.
    pub(crate) backdrop_groups: Vec<BackdropGroup>,
}

/// One glyph of a `GlyphRun`: id in the run's font, position in local px.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct GlyphPos {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

/// Record-time summary of one shared backdrop key (see `Op::BackdropBlur`).
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct BackdropGroup {
    pub key: u64,
    pub union_bounds: Rect,
    pub tiles: u32,
    /// `Some(σ)` while every tile of this key agrees — Impeller's
    /// `BackdropData::all_filters_equal` (its record pre-pass is
    /// `FirstPassDispatcher`; valo's is the recording itself). Mixed keys
    /// don't share: each tile blurs independently.
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
    ) -> Self {
        Self {
            id: next_id(),
            ops,
            bounds,
            draw_count,
            depth_slots,
            backdrop_groups,
        }
    }

    /// Process-unique identity (a deserialized list gets a fresh one: identity
    /// is about live values, not content).
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    pub fn bounds(&self) -> Option<Rect> {
        self.bounds
    }

    pub fn draw_count(&self) -> u32 {
        self.draw_count
    }

    pub fn depth_slots(&self) -> u32 {
        self.depth_slots
    }

    /// How many shared-backdrop groups this list records — a raster cache
    /// admission input: any backdrop read makes a list uncacheable
    /// standalone (it samples what is BEHIND it).
    pub fn backdrop_group_count(&self) -> usize {
        self.backdrop_groups.len()
    }

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
