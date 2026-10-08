use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use valo_geometry::{FillRule, Matrix, Path, Rect};

use crate::{Bounds, Image, ImageFilter, Paint, Sampling};

/// `Backdrop` filters the sampled scene before a save layer's children paint.
///
/// Compose image filters to control the order of backdrop effects. The layer's
/// paint separately controls how the filtered backdrop and children composite.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Backdrop {
    /// `filter` transforms the sampled scene in local coordinates.
    pub filter: ImageFilter,
    /// `shared_key` lets tiles reuse the first matching filtered snapshot,
    /// which sees the scene as of the first tile. Use one key only for tiles
    /// over the same background with the same filter.
    pub shared_key: Option<u64>,
}

impl Backdrop {
    /// `new` filters the backdrop with `filter` before foreground content paints.
    pub fn new(filter: ImageFilter) -> Self {
        Self {
            filter,
            shared_key: None,
        }
    }

    /// `blur` creates an isotropic Gaussian backdrop blur in local units.
    pub fn blur(sigma: f32) -> Self {
        Self::new(ImageFilter::blur(sigma, sigma))
    }

    /// `shared` marks this backdrop as one tile of a keyed group.
    pub fn shared(mut self, key: u64) -> Self {
        self.shared_key = Some(key);
        self
    }
}

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
/// by [`crate::DisplayListBuilder`] at record time. A draw's `bounds` is where
/// it may show, list-root space, cropped by the clip: the rect replay culls it
/// by, [`Rect::EVERYTHING`] for a draw that fills a list with no clip.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Op {
    Save,
    /// Open an offscreen layer scope, closed by the matching `Restore`. The
    /// recorder works out every fact about the layer once and records it
    /// here, the ones its children decide backpatched when the scope closes —
    /// still record time; replay reads them, never works them out again.
    SaveLayer {
        paint: Paint,
        /// Set = this layer is a MASK: its composite converts
        /// the texture to COVERAGE (luminance or alpha) and multiplies the
        /// enclosing layer by it (DstIn over the whole enclosing extent, so
        /// content outside the mask's ink disappears).
        mask_composite: Option<MaskKind>,
        /// The layer's content: the union of its children's bounds,
        /// list-root space, cropped by the bounds hint and by the clips made
        /// inside the layer but not by the clips around it, which a filter
        /// on the layer reads past (Flutter's layer bounds). Never padded for
        /// the layer's own filter, nor flooded: the renderer sizes the
        /// layer's texture from it, the clip and the paint. Unbounded when a
        /// child fills a clip the layer does not crop (Flutter's
        /// `content_is_unbounded`).
        scope_bounds: Bounds,
        /// The clip where the layer opens, cropped by the bounds hint,
        /// list-root space: unbounded when there is neither, empty when the
        /// hint misses the clip. Bounds the layer's texture together with
        /// the target.
        clip_bounds: Bounds,
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
        /// The composite paints the layer's whole clip whatever its children
        /// draw: a destructive blend changes the parent where the layer is
        /// transparent, a filter that colours transparent pixels colours
        /// them everywhere, and a backdrop layer opens full of the filtered
        /// parent (Flutter's `content_is_unbounded`). The layer's texture
        /// then covers its clip.
        floods_clip: bool,
        /// An enclosing group's alpha can ride the composite instead of a
        /// texture of the group: the paint blends `SrcOver` with no colour
        /// or image filter to take the alpha in before it filters.
        takes_group_opacity: bool,
        /// The layer's own alpha can ride its children instead: they take a
        /// group alpha and are pairwise disjoint, and the composite is
        /// nothing but that alpha (`takes_group_opacity`), with no backdrop
        /// to seed and no bounds hint to crop by. Replay may then skip the
        /// texture and draw each child at its own slot (Impeller's opacity
        /// peephole: elision changes nothing about depth).
        can_elide: bool,
        /// Set = the layer opens with the filtered scene already painted
        /// beneath it. Filter parameters are in local coordinates. Children
        /// paint afterward, and the composite applies group alpha to the
        /// filtered backdrop and children as one image — Flutter's
        /// `saveLayer(bounds, paint, backdrop)`.
        backdrop: Option<Backdrop>,
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
        /// The path's ink in local coordinates, stroke included and before
        /// any effect: what a layer for the paint's effects has to hold.
        content_bounds: Rect,
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
        /// The run's ink in local coordinates, stroke included and before
        /// any effect: what a layer for the paint's effects has to hold.
        content_bounds: Rect,
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
///
/// Two lists are equal when they record the same commands. A list is equal to itself
/// without a comparison, and a nested list is compared the same way, so a scene that
/// embeds the pictures its last frame did, unchanged, compares in the time of its own
/// few commands: what a host asks before drawing a scene again.
#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct DisplayList {
    id: u64,
    pub(crate) ops: Vec<Op>,
    /// Union of all draw bounds, list-root space.
    pub(crate) bounds: Bounds,
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
    /// Whether a group alpha distributes over the draws: every one
    /// alpha-linear and none overlapping.
    pub(crate) supports_opacity: bool,
}

/// `GlyphPos` identifies and positions one glyph within a glyph run.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct GlyphPos {
    /// `id` is the glyph identifier in the run's font.
    pub id: u32,
    /// `x` is the glyph's local horizontal position in pixels.
    pub x: f32,
    /// `y` is the glyph's local baseline position in pixels.
    pub y: f32,
}

/// `BackdropGroup` summarizes regions sharing one backdrop-filter key.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct BackdropGroup {
    /// `key` identifies the shared backdrop group.
    pub key: u64,
    /// `union_bounds` encloses every region in the group.
    pub union_bounds: Rect,
    /// `filter` is shared when every region agrees; `None` disables reuse.
    pub filter: Option<ImageFilter>,
}

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

impl PartialEq for DisplayList {
    fn eq(&self, other: &DisplayList) -> bool {
        self.id == other.id || self.ops == other.ops
    }
}

/// `Recording` is what a builder resolved about its ops while recording
/// them, handed to the list it builds; see [`DisplayList`]'s fields.
pub(crate) struct Recording {
    pub bounds: Bounds,
    pub depth_slots: u32,
    pub supports_opacity: bool,
}

/// `Tally` is what a list's finished ops add up to, counted in one pass
/// when the list is built: its draws, its backdrop reads and its shared
/// backdrop groups (Impeller collects backdrop groups in a pass of its own,
/// `FirstPassDispatcher::saveLayer`).
#[derive(Default)]
struct Tally {
    draw_count: u32,
    backdrop_reads: u32,
    backdrop_groups: Vec<BackdropGroup>,
}

impl Tally {
    fn of(ops: &[Op]) -> Self {
        let mut tally = Self::default();
        for op in ops {
            tally.add(op);
        }
        tally
    }

    fn add(&mut self, op: &Op) {
        match op {
            Op::SaveLayer {
                clip_bounds,
                backdrop,
                ..
            } => {
                self.draw_count += 1; // the composite
                if let Some(backdrop) = backdrop {
                    self.backdrop_reads += 1;
                    self.join_backdrop_group(backdrop, clip_bounds);
                }
            }
            Op::DrawDisplayList { list, .. } => {
                self.draw_count += list.draw_count;
                self.backdrop_reads += list.backdrop_reads;
            }
            Op::DrawRect { .. }
            | Op::DrawPath { .. }
            | Op::RRectBlur { .. }
            | Op::DrawImage { .. }
            | Op::GlyphRun { .. } => self.draw_count += 1,
            Op::Save | Op::Restore | Op::Transform(_) | Op::ClipPath { .. } => {}
        }
    }

    /// `join_backdrop_group` adds a keyed backdrop layer, whose clip is
    /// `region`, to its group. An unclipped layer's region is every rect: a
    /// group's union is a rect the planner cuts to its target.
    fn join_backdrop_group(&mut self, backdrop: &Backdrop, region: &Bounds) {
        let Some(key) = backdrop.shared_key else {
            return;
        };
        let region = match region {
            // Clipped away: the layer shows nowhere, so nothing reads its glass.
            Bounds::Empty => return,
            Bounds::Bounded(rect) => *rect,
            Bounds::Unbounded => Rect::EVERYTHING,
        };
        match self.backdrop_groups.iter_mut().find(|g| g.key == key) {
            Some(group) => {
                group.union_bounds = group.union_bounds.union(&region);
                if group.filter.as_ref() != Some(&backdrop.filter) {
                    group.filter = None; // Different filters cannot share one snapshot.
                }
            }
            None => self.backdrop_groups.push(BackdropGroup {
                key,
                union_bounds: region,
                filter: Some(backdrop.filter.clone()),
            }),
        }
    }
}

impl DisplayList {
    pub(crate) fn new(ops: Vec<Op>, recording: Recording) -> Self {
        let Recording {
            bounds,
            depth_slots,
            supports_opacity,
        } = recording;
        let Tally {
            draw_count,
            backdrop_reads,
            backdrop_groups,
        } = Tally::of(&ops);
        Self {
            id: next_id(),
            ops,
            bounds,
            draw_count,
            depth_slots,
            backdrop_groups,
            backdrop_reads,
            supports_opacity,
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

    /// `bounds` returns where the list's draws may show, in list
    /// coordinates: nowhere, inside a rectangle, or, when a draw fills a clip
    /// the list does not make, anywhere.
    pub fn bounds(&self) -> Bounds {
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

    /// `supports_opacity` says whether a group alpha distributes over this
    /// list's draws: every one alpha-linear and none overlapping. A layer
    /// that embeds the list asks this instead of forfeiting elision
    /// (Flutter's `can_apply_group_opacity`).
    pub fn supports_opacity(&self) -> bool {
        self.supports_opacity
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

#[cfg(test)]
mod equality_tests {
    use std::sync::Arc;

    use super::*;
    use crate::{DisplayListBuilder, Paint};

    fn rects(count: u32) -> DisplayList {
        let mut b = DisplayListBuilder::new();
        let paint = Paint::default();
        for i in 0..count {
            b.draw_rect(Rect::new(0.0, i as f32 * 10.0, 5.0, 5.0), &paint);
        }
        b.build()
    }

    #[test]
    fn lists_with_the_same_commands_are_equal_and_others_not() {
        assert_eq!(rects(3), rects(3));
        assert_ne!(rects(3), rects(4));
    }

    #[test]
    fn a_scene_over_the_same_retained_picture_is_equal_by_identity() {
        let picture = Arc::new(rects(3));
        let scene = |picture: &Arc<DisplayList>| {
            let mut b = DisplayListBuilder::new();
            b.draw_display_list(picture);
            b.build()
        };
        assert_eq!(scene(&picture), scene(&picture));
        assert_ne!(scene(&picture), scene(&Arc::new(rects(4))));
    }
}
