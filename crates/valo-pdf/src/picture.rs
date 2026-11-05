//! The first pass: the parts of a page PDF can't hold, each with the list that draws it alone and
//! where it goes. The writer walks a list the same way (`pictured_scope`, `pictured_draw`), so the
//! nth part it meets is the nth found here.

use std::sync::Arc;

use valo::{Bounds, DisplayList, DisplayListBuilder, Matrix, Op, Paint, Rect, Shader};

/// `Site` is one part of a page drawn as pixels: the list that draws it, already moved and
/// scaled into its `pixels`, and its `bounds` on the page, in page units.
pub(crate) struct Site {
    pub list: Arc<DisplayList>,
    pub pixels: [u32; 2],
    pub bounds: Rect,
}

/// `sites` are the parts of `list`, a page of `size` units, PDF can't hold, in the order the
/// list holds them: `None` for one that shows nowhere on the page.
pub(crate) fn sites(list: &DisplayList, size: [f32; 2], pixels_per_unit: f32) -> Vec<Option<Site>> {
    let mut finder = Finder {
        page: Rect::new(0.0, 0.0, size[0], size[1]),
        pixels_per_unit,
        frames: vec![Frame::default()],
        sites: Vec::new(),
    };
    finder.list(list, Matrix::IDENTITY);
    finder.sites
}

/// `pictured_scope` is whether the scope `ops[at]` opens is drawn as pixels: a layer with a
/// filter, a backdrop or a mask, or one a mask inside it masks.
pub(crate) fn pictured_scope(ops: &[Op], at: usize) -> bool {
    let Op::SaveLayer {
        paint,
        mask_composite,
        backdrop,
        ..
    } = &ops[at]
    else {
        return false;
    };
    mask_composite.is_some()
        || backdrop.is_some()
        || paint.effective_image_filter().is_some()
        || paint.color_filter.is_some()
        || holds_a_mask(ops, at)
}

/// `pictured_draw` is whether a draw is drawn as pixels: a blur, a filter, an image, or a
/// shader PDF has no word for.
pub(crate) fn pictured_draw(op: &Op) -> bool {
    match op {
        Op::RRectBlur { .. } | Op::DrawImage { .. } => true,
        Op::DrawRect { paint, .. } | Op::DrawPath { paint, .. } | Op::GlyphRun { paint, .. } => {
            needs_pixels(paint)
        }
        _ => false,
    }
}

/// `scope_end` is the index of the `Restore` that closes the scope `ops[at]` opens.
pub(crate) fn scope_end(ops: &[Op], at: usize) -> usize {
    let mut depth = 0usize;
    for (index, op) in ops.iter().enumerate().skip(at) {
        match op {
            Op::Save | Op::SaveLayer { .. } => depth += 1,
            Op::Restore => {
                depth -= 1;
                if depth == 0 {
                    return index;
                }
            }
            _ => {}
        }
    }
    ops.len() - 1
}

fn needs_pixels(paint: &Paint) -> bool {
    paint.mask_blur.is_some()
        || paint.effective_image_filter().is_some()
        || paint.color_filter.is_some()
        || matches!(
            paint.shader,
            Some(Shader::Image { .. } | Shader::Runtime(_))
        )
}

/// `holds_a_mask` is whether a mask layer opens directly inside the scope `ops[at]` opens: a mask
/// applies to the layer it is in.
fn holds_a_mask(ops: &[Op], at: usize) -> bool {
    let end = scope_end(ops, at);
    let mut index = at + 1;
    while index < end {
        match &ops[index] {
            Op::SaveLayer {
                mask_composite: Some(_),
                ..
            } => return true,
            Op::Save | Op::SaveLayer { .. } => index = scope_end(ops, index) + 1,
            _ => index += 1,
        }
    }
    false
}

/// `Frame` is a scope open on the way to a part: the transforms and clips it made, which the
/// part's own list makes again, and the transform when it opened.
#[derive(Default)]
struct Frame {
    ops: Vec<Op>,
    transform: Matrix,
}

struct Finder {
    page: Rect,
    pixels_per_unit: f32,
    frames: Vec<Frame>,
    sites: Vec<Option<Site>>,
}

impl Finder {
    /// `list` walks `list`'s ops; `root` maps its root space to the page's.
    fn list(&mut self, list: &DisplayList, root: Matrix) {
        let ops = list.ops();
        let mut at = 0;
        while at < ops.len() {
            at = self.op(ops, at, root);
        }
    }

    /// `op` takes in `ops[at]` and returns the index of the next op to walk.
    fn op(&mut self, ops: &[Op], at: usize, root: Matrix) -> usize {
        let op = &ops[at];
        if pictured_scope(ops, at) {
            let end = scope_end(ops, at);
            let Op::SaveLayer {
                paint,
                scope_bounds,
                clip_bounds,
                floods_clip,
                backdrop,
                ..
            } = op
            else {
                unreachable!("only a layer is a pictured scope");
            };
            let reach = if *floods_clip || backdrop.is_some() {
                *clip_bounds
            } else {
                let padding = paint.device_effect_padding(&self.transform());
                match scope_bounds.rect() {
                    Some(rect) => Bounds::of(rect.expand(padding)).intersect(clip_bounds),
                    None => *scope_bounds,
                }
            };
            self.site(&ops[at..=end], reach.map(&root));
            return end + 1;
        }
        if pictured_draw(op) {
            let bounds = draw_bounds(op).map_or(Bounds::Unbounded, Bounds::of);
            self.site(std::slice::from_ref(op), bounds.map(&root));
            return at + 1;
        }
        match op {
            Op::Save | Op::SaveLayer { .. } => {
                let transform = self.transform();
                self.frames.push(Frame {
                    ops: Vec::new(),
                    transform,
                });
            }
            Op::Restore => {
                self.frames.pop();
            }
            Op::Transform(_) | Op::ClipPath { .. } => self.frame().ops.push(op.clone()),
            Op::DrawDisplayList { list, .. } => {
                let inner = self.transform();
                self.frames.push(Frame {
                    ops: Vec::new(),
                    transform: inner,
                });
                self.list(list, inner);
                self.frames.pop();
            }
            _ => {}
        }
        at + 1
    }

    /// `site` keeps `ops`, a part showing within `bounds` (page space), with the list that draws
    /// it alone at the page's pixel scale.
    fn site(&mut self, ops: &[Op], bounds: Bounds) {
        let shown = match bounds {
            Bounds::Unbounded => Some(self.page),
            Bounds::Bounded(rect) => rect.intersect(&self.page),
            Bounds::Empty => None,
        };
        let site = shown.filter(|rect| !rect.is_empty()).map(|shown| {
            let scale = self.pixels_per_unit;
            // On whole pixels, so the picture lands where it was drawn.
            let left = (shown.x * scale).floor();
            let top = (shown.y * scale).floor();
            let right = ((shown.x + shown.width) * scale).ceil();
            let bottom = ((shown.y + shown.height) * scale).ceil();
            let bounds = Rect::new(
                left / scale,
                top / scale,
                (right - left) / scale,
                (bottom - top) / scale,
            );
            let mut b = DisplayListBuilder::new();
            b.translate(-left, -top);
            b.scale(scale, scale);
            for frame in &self.frames {
                b.save();
                frame.ops.iter().for_each(|op| b.replay(op));
            }
            ops.iter().for_each(|op| b.replay(op));
            for _ in &self.frames {
                b.restore();
            }
            Site {
                list: Arc::new(b.build()),
                pixels: [(right - left) as u32, (bottom - top) as u32],
                bounds,
            }
        });
        self.sites.push(site);
    }

    /// `transform` maps the current space to the page's.
    fn transform(&self) -> Matrix {
        let frame = self.frames.last().expect("the page's frame is open");
        frame
            .ops
            .iter()
            .fold(frame.transform, |transform, op| match op {
                Op::Transform(matrix) => transform.then(matrix),
                _ => transform,
            })
    }

    fn frame(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("the page's frame is open")
    }
}

/// `draw_bounds` is where a draw may show, in its list's root space, cropped by its clip.
fn draw_bounds(op: &Op) -> Option<Rect> {
    match op {
        Op::DrawRect { bounds, .. }
        | Op::DrawPath { bounds, .. }
        | Op::RRectBlur { bounds, .. }
        | Op::DrawImage { bounds, .. }
        | Op::GlyphRun { bounds, .. } => Some(*bounds),
        _ => None,
    }
}
