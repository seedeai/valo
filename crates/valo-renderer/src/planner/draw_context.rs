//! Draw contexts: one per texture being drawn — the main target at the
//! bottom of the stack, one per open layer, snapshot or raster fill above
//! it — each recording the draws into its texture until they become a pass
//! (Skia Graphite's `DrawContext` and its `DrawList`), and the clips a fresh
//! pass replays.
//!
//! A context knows where its texels lie ([`PixelArea`]) and which slots of
//! the depth line it hosts ([`DepthRange`]); what the walk is doing (its
//! transform, group alpha) rides the scope entries in `replay`, and what the
//! GPU has been told is the plan writer's.

use valo_dl::DisplayList;
use valo_geometry::{Color, Matrix, Point, Rect};

use crate::frame::{ContextAttachments, Draw, Scissor};
use crate::pool::{Attachments, LayerTarget};
use crate::raster::FillTarget;
use crate::renderer::RenderTarget;

use super::snapshot::{Placement, Snapshot};

/// `PixelArea` is where a texture's texels lie in the space beneath it: a
/// rect, and the whole texels that hold it. It is also what a draw into the
/// texture is culled by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PixelArea {
    rect: Rect,
    size: [u32; 2],
}

impl PixelArea {
    /// `over` is the area of a texture over `rect`: its origin at the rect's,
    /// its size the rect's rounded up — the one formula for a layer's or a
    /// pass's texture, which allocation, sampling and the filter recipes
    /// must agree on, or edge texels stretch.
    pub fn over(rect: Rect) -> Self {
        let size = [
            rect.width.ceil().max(1.0) as u32,
            rect.height.ceil().max(1.0) as u32,
        ];
        Self { rect, size }
    }

    /// `of_size` is a texture of `size` texels at the origin of its space:
    /// a caller's target, or a cache texture.
    pub fn of_size(size: [u32; 2]) -> Self {
        Self {
            rect: Rect::new(0.0, 0.0, size[0] as f32, size[1] as f32),
            size,
        }
    }

    /// `rect` is the area in the space beneath the texture.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// `origin` is where texel (0, 0) lies.
    pub fn origin(&self) -> Point {
        Point::new(self.rect.x, self.rect.y)
    }

    /// `size` is the texture's extent in texels.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// `placement` places the texture's texels in the space beneath it.
    pub fn placement(&self) -> Placement {
        Placement {
            size: self.size,
            transform: Matrix::translation(self.rect.x, self.rect.y),
            placed: false,
        }
    }

    /// `culls` reports whether a draw whose bounds in the space beneath are
    /// `bounds` shows nowhere in the area.
    pub fn culls(&self, bounds: &Rect) -> bool {
        !bounds.intersects(&self.rect)
    }

    /// `texels` is the whole texture in the space beneath it: the area's
    /// rect, with the texels its rounding added.
    pub fn texels(&self) -> Rect {
        let [width, height] = self.size;
        Rect::new(self.rect.x, self.rect.y, width as f32, height as f32)
    }

    /// `scissor` is the whole texels that hold `rect`, a rect of the space
    /// beneath, cut to the texture; `None` when the texture holds none of
    /// it.
    pub fn scissor(&self, rect: &Rect) -> Option<Scissor> {
        let [width, height] = self.size.map(|extent| extent as f32);
        let left = (rect.x - self.rect.x).floor().max(0.0);
        let top = (rect.y - self.rect.y).floor().max(0.0);
        let right = (rect.right() - self.rect.x).ceil().min(width);
        let bottom = (rect.bottom() - self.rect.y).ceil().min(height);
        (left < right && top < bottom).then_some(Scissor {
            x: left as u32,
            y: top as u32,
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        })
    }

    /// `snapped` is `rect`, a rect of the space beneath, with each edge
    /// moved to the texel edge nearest it; `None` when an edge lies further
    /// than `tolerance` from one.
    pub fn snapped(&self, rect: &Rect, tolerance: f32) -> Option<Rect> {
        let snap = |edge: f32, origin: f32| {
            let texel = (edge - origin).round();
            ((edge - origin - texel).abs() <= tolerance).then_some(origin + texel)
        };
        Some(Rect::from_ltrb(
            snap(rect.x, self.rect.x)?,
            snap(rect.y, self.rect.y)?,
            snap(rect.right(), self.rect.x)?,
            snap(rect.bottom(), self.rect.y)?,
        ))
    }
}

/// `DepthRange` is the slots of the depth line a texture hosts: `slots` of
/// them from `first_slot`, mapped onto depths in [0, 1). A slot is a
/// position on the line of the outermost list being replayed into the
/// texture, nested lists' slots placed after their embedder's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DepthRange {
    first_slot: u32,
    slots: u32,
}

impl DepthRange {
    /// `new` is the range of `slots` slots from `first_slot`.
    pub const fn new(first_slot: u32, slots: u32) -> Self {
        Self { first_slot, slots }
    }

    /// `of_list` is the range a texture that `list` alone is replayed into
    /// hosts: its slots from zero, and the end slot its root clips expire at.
    pub fn of_list(list: &DisplayList) -> Self {
        Self::new(0, list.depth_slots() + 1)
    }

    /// `z` is `slot`'s depth.
    pub fn z(&self, slot: u32) -> f32 {
        (slot as f32 - self.first_slot as f32) / self.slots as f32
    }

    /// `floor` is the depth below every slot: what the texture holds before
    /// its first draw, a backdrop's glass or pixels drawn back.
    pub fn floor(&self) -> f32 {
        0.0
    }

    /// `half_slot_above` is the depth half a slot above `z`: past a draw at
    /// `z`, below the next slot.
    pub fn half_slot_above(&self, z: f32) -> f32 {
        z + 0.5 / self.slots as f32
    }
}

/// `DrawContext` is one texture being drawn and the draws it holds.
///
/// Coordinates are those of the space beneath the texture — the transform
/// stack is never rebased; the area's origin is subtracted at MVP time
/// instead, so children land in the texture's texels.
pub(super) struct DrawContext {
    pub attachments: ContextAttachments,
    /// The texture the context's last pass resolves into: what names the
    /// context to a shared backdrop, and what a target drawn over without
    /// clearing has its pixels copied out of.
    pub src_texture: wgpu::Texture,
    pub area: PixelArea,
    pub depth: DepthRange,
    /// What the context's first pass clears its multisample scratch to.
    /// Every pass clears: scratch never carries a picture between passes,
    /// so a target that keeps its pixels has them drawn back as its first
    /// draw instead (`start_from_existing_pixels`).
    pub clear: Color,
    pub first_pass_emitted: bool,
    /// The draws of the pass being recorded.
    pub draws: Vec<Draw>,
    /// The clips drawn so far, which a fresh pass after a split replays
    /// while their ceilings still hold.
    pub clips: Vec<Draw>,
}

impl DrawContext {
    /// `main` is the bottom context: the caller's target, drawn through
    /// pooled tile-only `attachments`. A caller's target that keeps its
    /// pixels starts from transparent, and its pixels are drawn back over
    /// that.
    pub fn main(attachments: Attachments, target: &RenderTarget, list: &DisplayList) -> Self {
        let mut context = Self::new(
            attachments.resolving_into(target.view.clone()),
            target.texture.clone(),
            PixelArea::of_size(target.size),
            DepthRange::of_list(list),
        );
        context.clear = target.clear.unwrap_or(Color::TRANSPARENT);
        context.draws.reserve(list.draw_count() as usize);
        context
    }

    /// `offscreen` is a pooled layer texture over `area`, hosting `depth`,
    /// cleared to transparent.
    pub fn offscreen(layer: LayerTarget, area: PixelArea, depth: DepthRange) -> Self {
        Self::new(
            ContextAttachments {
                msaa: layer.msaa,
                depth: layer.depth,
                resolve: layer.resolve,
            },
            layer.resolve_texture,
            area,
            depth,
        )
    }

    /// `raster` is a list-raster cache texture, filled from its own origin
    /// through pooled `attachments`.
    pub fn raster(attachments: Attachments, fill: &FillTarget, depth: DepthRange) -> Self {
        Self::new(
            attachments.resolving_into(fill.view.clone()),
            fill.texture.clone(),
            PixelArea::of_size(fill.size),
            depth,
        )
    }

    fn new(
        attachments: ContextAttachments,
        src_texture: wgpu::Texture,
        area: PixelArea,
        depth: DepthRange,
    ) -> Self {
        Self {
            attachments,
            src_texture,
            area,
            depth,
            clear: Color::TRANSPARENT,
            first_pass_emitted: false,
            draws: Vec::new(),
            clips: Vec::new(),
        }
    }

    /// `picture` is what this context holds once its last pass is emitted:
    /// its resolve texture over its area.
    pub fn picture(&self) -> Snapshot {
        Snapshot {
            view: self.attachments.resolve.clone(),
            placement: self.area.placement(),
        }
    }
}

/// `DrawContexts` is the contexts being drawn: never empty, the main target
/// at the bottom, the innermost layer last.
pub(super) struct DrawContexts {
    contexts: Vec<DrawContext>,
}

impl DrawContexts {
    /// `new` is the stack of `main` alone.
    pub fn new(main: DrawContext) -> Self {
        Self {
            contexts: vec![main],
        }
    }

    /// `top` is the innermost open context, the one draws append to.
    pub fn top(&self) -> &DrawContext {
        self.contexts
            .last()
            .expect("the main target is never popped")
    }

    pub fn top_mut(&mut self) -> &mut DrawContext {
        self.contexts
            .last_mut()
            .expect("the main target is never popped")
    }

    pub fn push(&mut self, context: DrawContext) {
        self.contexts.push(context);
    }

    /// `pop` closes the innermost context; the main target stays.
    pub fn pop(&mut self) -> DrawContext {
        debug_assert!(self.contexts.len() > 1, "the main target is never popped");
        self.contexts.pop().expect("a context above the main one")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_area_rounds_its_texels_up_and_keeps_its_origin() {
        let area = PixelArea::over(Rect::new(10.0, -4.0, 20.5, 0.25));
        assert_eq!(area.size(), [21, 1]);
        assert_eq!(area.origin(), Point::new(10.0, -4.0));
        assert!(area.culls(&Rect::new(40.0, 0.0, 5.0, 5.0)));
        assert!(!area.culls(&Rect::new(25.0, -10.0, 5.0, 7.0)));
    }

    /// A rect's scissor is the whole texels holding it, from the area's
    /// origin and cut to the texture; a rect goes to the texel edges only
    /// when all its edges are near them.
    #[test]
    fn an_area_finds_the_texels_of_a_rect() {
        let area = PixelArea::over(Rect::new(10.0, 20.0, 30.5, 8.0));
        assert_eq!(area.texels(), Rect::new(10.0, 20.0, 31.0, 8.0));
        assert_eq!(
            area.scissor(&Rect::from_ltrb(12.25, 0.0, 100.0, 23.5)),
            Some(Scissor {
                x: 2,
                y: 0,
                width: 29,
                height: 4,
            })
        );
        assert_eq!(area.scissor(&Rect::new(50.0, 20.0, 5.0, 5.0)), None);
        assert_eq!(
            area.snapped(&Rect::from_ltrb(12.1, 21.9, 20.0, 25.0), 0.124),
            Some(Rect::from_ltrb(12.0, 22.0, 20.0, 25.0))
        );
        assert_eq!(
            area.snapped(&Rect::from_ltrb(12.5, 22.0, 20.0, 25.0), 0.124),
            None
        );
        let between_pixels = PixelArea::over(Rect::new(0.5, 0.0, 10.0, 10.0));
        assert_eq!(
            between_pixels.snapped(&Rect::from_ltrb(2.5, 1.0, 4.5, 3.0), 0.124),
            Some(Rect::from_ltrb(2.5, 1.0, 4.5, 3.0)),
            "the texel edges are the area's, not the space's"
        );
    }

    /// A layer hosts the slots after its base up to its composite's; its
    /// children land strictly between 0 and 1, above what it opened with.
    #[test]
    fn a_layers_depth_range_rebases_its_slots() {
        let layer = DepthRange::new(7, 4);
        assert_eq!(layer.z(8), 0.25);
        assert_eq!(layer.z(10), 0.75);
        assert_eq!(layer.floor(), 0.0);
        assert_eq!(layer.half_slot_above(layer.z(8)), 0.375);
    }
}
