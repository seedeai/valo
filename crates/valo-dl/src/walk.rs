use valo_geometry::Matrix;

use crate::{Bounds, DisplayList, DisplayListBuilder, Op};

/// `RESTORE` closes a nested list a walk goes into, which has no `Restore` of its own.
const RESTORE: &Op = &Op::Restore;

/// `DisplayListWalker` iterates a list's ops in order, nested lists' included, keeping the
/// transform and clips in force (`matrix_clip`): walkdir's iterator, with `skip_scope` for its
/// `skip_current_dir`. Vector writers walk a list this way, writing what they can and drawing the
/// rest as pixels from [`MatrixClipState::isolate`].
///
/// A nested list's `DrawDisplayList` opens a scope, closed by a `Restore` after its ops, so every
/// scope a walk meets closes as a `Save`'s does.
pub struct DisplayListWalker<'a> {
    /// `lists` holds the lists being walked, the innermost last.
    lists: Vec<ListPosition<'a>>,
    /// `current` is the op `next` gave last; the next call makes it take effect.
    current: Option<CurrentOp<'a>>,
    matrix_clip: MatrixClipState,
}

/// `ListPosition` is a list being walked and the index of its next op.
#[derive(Clone, Copy)]
struct ListPosition<'a> {
    ops: &'a [Op],
    next: usize,
}

/// `CurrentOp` is the op `next` gave last, with its list and index, so the scope it opens can be
/// found and skipped.
struct CurrentOp<'a> {
    op: &'a Op,
    list: &'a [Op],
    index: usize,
    skip_scope: bool,
}

/// `MatrixClipState` is where an op is drawn during a walk: the transforms and clips in force, in
/// the walked list's space. Flutter's `DisplayListMatrixClipState`; Skia's canvas keeps the same
/// as its `MCRec`.
pub struct MatrixClipState {
    scopes: Vec<Scope>,
}

/// `Scope` is a scope open around the op: the transforms and clips made in it, the transform
/// when it opened, and the transform from the space its list's ops record their bounds in to the
/// walked list's.
#[derive(Default)]
struct Scope {
    transforms_and_clips: Vec<Op>,
    transform_at_open: Matrix,
    list_transform: Matrix,
}

impl DisplayList {
    /// `walk` iterates the list's ops with the transform and clips in force at each.
    pub fn walk(&self) -> DisplayListWalker<'_> {
        DisplayListWalker {
            lists: vec![ListPosition {
                ops: self.ops(),
                next: 0,
            }],
            current: None,
            matrix_clip: MatrixClipState {
                scopes: vec![Scope::default()],
            },
        }
    }
}

impl<'a> DisplayListWalker<'a> {
    /// `scope` is the ops of the scope the op `next` gave last opens: a `Save` or `SaveLayer`
    /// through its `Restore`, or the op alone.
    pub fn scope(&self) -> &'a [Op] {
        let Some(current) = &self.current else {
            return &[];
        };
        match current.op {
            Op::Save | Op::SaveLayer { .. } => {
                &current.list[current.index..=scope_end(current.list, current.index)]
            }
            _ => std::slice::from_ref(current.op),
        }
    }

    /// `skip_scope` skips the scope the op `next` gave last opens: a `Save`'s or `SaveLayer`'s
    /// ops and `Restore`, or a nested list. It does nothing after any other op.
    pub fn skip_scope(&mut self) {
        if let Some(current) = &mut self.current {
            current.skip_scope = true;
        }
    }

    /// `matrix_clip` is the transforms and clips in force where the op `next` gave last is
    /// drawn, before it takes effect.
    pub fn matrix_clip(&self) -> &MatrixClipState {
        &self.matrix_clip
    }

    /// `finish_current` makes the op `next` gave last take effect, or skips its scope.
    fn finish_current(&mut self) {
        let Some(current) = self.current.take() else {
            return;
        };
        let state = &mut self.matrix_clip;
        match (current.op, current.skip_scope) {
            (Op::Save | Op::SaveLayer { .. }, true) => {
                if let Some(position) = self.lists.last_mut() {
                    position.next = scope_end(current.list, current.index) + 1;
                }
            }
            (Op::Save | Op::SaveLayer { .. }, false) => {
                state.open_scope(state.innermost().list_transform)
            }
            (Op::Restore, _) => {
                state.scopes.pop();
            }
            (Op::DrawDisplayList { list, .. }, false) => {
                state.open_scope(state.transform());
                self.lists.push(ListPosition {
                    ops: list.ops(),
                    next: 0,
                });
            }
            (Op::Transform(_) | Op::ClipPath { .. }, _) => {
                let scope = state.innermost_mut();
                scope.transforms_and_clips.push(current.op.clone());
            }
            _ => {}
        }
    }
}

impl<'a> Iterator for DisplayListWalker<'a> {
    type Item = &'a Op;

    fn next(&mut self) -> Option<&'a Op> {
        self.finish_current();
        let position = *self.lists.last()?;
        let (op, list, index) = match position.ops.get(position.next) {
            Some(op) => {
                if let Some(position) = self.lists.last_mut() {
                    position.next += 1;
                }
                (op, position.ops, position.next)
            }
            None if self.lists.len() > 1 => {
                self.lists.pop();
                (RESTORE, std::slice::from_ref(RESTORE), 0)
            }
            None => return None,
        };
        self.current = Some(CurrentOp {
            op,
            list,
            index,
            skip_scope: false,
        });
        Some(op)
    }
}

impl MatrixClipState {
    /// `transform` maps the op's space to the walked list's.
    pub fn transform(&self) -> Matrix {
        let scope = self.innermost();
        scope
            .transforms_and_clips
            .iter()
            .fold(scope.transform_at_open, |transform, op| match op {
                Op::Transform(matrix) => transform.then(matrix),
                _ => transform,
            })
    }

    /// `bounds` is where `op` can show in the walked list's space, cut by the clips in force: a
    /// draw's, a nested list's, or the layer's an op opens, its effects included. A save,
    /// restore, transform or clip draws nothing itself, so it shows nowhere.
    pub fn bounds(&self, op: &Op) -> Bounds {
        let bounds = match op {
            Op::SaveLayer {
                paint,
                scope_bounds,
                clip_bounds,
                floods_clip,
                backdrop,
                ..
            } => {
                if *floods_clip || backdrop.is_some() {
                    *clip_bounds
                } else {
                    let padding = paint.device_effect_padding(&self.transform());
                    match scope_bounds.rect() {
                        Some(rect) => Bounds::of(rect.expand(padding)).intersect(clip_bounds),
                        None => *scope_bounds,
                    }
                }
            }
            Op::DrawRect { bounds, .. }
            | Op::DrawPath { bounds, .. }
            | Op::RRectBlur { bounds, .. }
            | Op::DrawImage { bounds, .. }
            | Op::GlyphRun { bounds, .. }
            | Op::DrawDisplayList { bounds, .. } => Bounds::of(*bounds),
            Op::Save | Op::Restore | Op::Transform(_) | Op::ClipPath { .. } => Bounds::Empty,
        };
        bounds.map(&self.innermost().list_transform)
    }

    /// `isolate` is `ops` alone, drawn where they are drawn in the walked list: between the
    /// transforms and clips in force, in the walked list's space.
    pub fn isolate(&self, ops: &[Op]) -> DisplayList {
        let mut b = DisplayListBuilder::new();
        for scope in &self.scopes {
            b.save();
            scope
                .transforms_and_clips
                .iter()
                .for_each(|op| b.replay(op));
        }
        ops.iter().for_each(|op| b.replay(op));
        for _ in &self.scopes {
            b.restore();
        }
        b.build()
    }

    fn open_scope(&mut self, list_transform: Matrix) {
        let transform_at_open = self.transform();
        self.scopes.push(Scope {
            transforms_and_clips: Vec::new(),
            transform_at_open,
            list_transform,
        });
    }

    fn innermost(&self) -> &Scope {
        self.scopes.last().expect("the walked list's scope is open")
    }

    fn innermost_mut(&mut self) -> &mut Scope {
        self.scopes
            .last_mut()
            .expect("the walked list's scope is open")
    }
}

/// `scope_end` is the index of the `Restore` that closes the scope `ops[at]` opens. A built list
/// closes every scope it opens.
fn scope_end(ops: &[Op], at: usize) -> usize {
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
    unreachable!("a built list closes every scope it opens")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{ClipOp, Paint};
    use valo_geometry::{Color, Point, Rect};

    fn kinds<'a>(ops: impl IntoIterator<Item = &'a Op>) -> Vec<&'static str> {
        ops.into_iter()
            .map(|op| match op {
                Op::Save => "save",
                Op::SaveLayer { .. } => "layer",
                Op::Restore => "restore",
                Op::Transform(_) => "transform",
                Op::ClipPath { .. } => "clip",
                Op::DrawRect { .. } => "rect",
                Op::DrawPath { .. } => "path",
                Op::RRectBlur { .. } => "blur",
                Op::DrawImage { .. } => "image",
                Op::GlyphRun { .. } => "glyphs",
                Op::DrawDisplayList { .. } => "list",
            })
            .collect()
    }

    fn red() -> Paint {
        Paint::from_color(Color::rgb(1.0, 0.0, 0.0))
    }

    #[test]
    fn a_walk_gives_each_op_with_the_transform_in_force() {
        let mut b = DisplayListBuilder::new();
        b.save();
        b.translate(10.0, 20.0);
        b.draw_rect(Rect::new(0.0, 0.0, 5.0, 5.0), &red());
        b.restore();
        b.draw_rect(Rect::new(0.0, 0.0, 5.0, 5.0), &red());
        let list = b.build();

        let mut seen = Vec::new();
        let mut ops = list.walk();
        while let Some(op) = ops.next() {
            if matches!(op, Op::DrawRect { .. }) {
                let origin = ops
                    .matrix_clip()
                    .transform()
                    .map_point(Point::from((0.0, 0.0)));
                seen.push(origin);
            }
        }
        assert_eq!(
            seen,
            vec![Point::from((10.0, 20.0)), Point::from((0.0, 0.0))]
        );
    }

    #[test]
    fn skipping_a_scope_skips_its_ops_and_its_restore() {
        let mut b = DisplayListBuilder::new();
        b.save_layer(None, &Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.5)));
        b.draw_rect(Rect::new(0.0, 0.0, 5.0, 5.0), &red());
        b.restore();
        b.draw_rect(Rect::new(0.0, 0.0, 5.0, 5.0), &red());
        let list = b.build();

        let mut visited = Vec::new();
        let mut scopes = Vec::new();
        let mut ops = list.walk();
        while let Some(op) = ops.next() {
            visited.push(op);
            if matches!(op, Op::SaveLayer { .. }) {
                scopes.push(kinds(ops.scope()));
                ops.skip_scope();
            }
        }
        assert_eq!(kinds(visited), vec!["layer", "rect"]);
        assert_eq!(scopes, vec![vec!["layer", "rect", "restore"]]);
    }

    #[test]
    fn a_nested_list_is_a_scope_a_restore_closes() {
        let mut inner = DisplayListBuilder::new();
        inner.draw_rect(Rect::new(0.0, 0.0, 5.0, 5.0), &red());
        let inner = Arc::new(inner.build());
        let mut b = DisplayListBuilder::new();
        b.translate(100.0, 0.0);
        b.draw_display_list(&inner);
        let list = b.build();

        let mut visited = Vec::new();
        let mut rect_bounds = None;
        let mut ops = list.walk();
        while let Some(op) = ops.next() {
            visited.push(op);
            if matches!(op, Op::DrawRect { .. }) {
                rect_bounds = ops.matrix_clip().bounds(op).rect();
            }
        }
        assert_eq!(kinds(visited), vec!["transform", "list", "rect", "restore"]);
        assert_eq!(rect_bounds, Some(Rect::new(100.0, 0.0, 5.0, 5.0)));
    }

    #[test]
    fn an_isolated_draw_keeps_the_transforms_and_clips_around_it() {
        let mut b = DisplayListBuilder::new();
        b.save();
        b.translate(10.0, 0.0);
        b.clip_rect(Rect::new(0.0, 0.0, 4.0, 4.0), ClipOp::Intersect);
        b.draw_rect(Rect::new(0.0, 0.0, 8.0, 8.0), &red());
        b.draw_rect(Rect::new(20.0, 0.0, 8.0, 8.0), &red());
        b.restore();
        let list = b.build();

        let mut ops = list.walk();
        let isolated = loop {
            let op = ops.next().expect("the list draws a rect");
            if matches!(op, Op::DrawRect { .. }) {
                break ops.matrix_clip().isolate(ops.scope());
            }
        };
        assert_eq!(
            kinds(isolated.ops()),
            vec![
                "save",
                "save",
                "transform",
                "clip",
                "rect",
                "restore",
                "restore"
            ]
        );
        assert_eq!(
            isolated.bounds().rect(),
            Some(Rect::new(10.0, 0.0, 4.0, 4.0))
        );
    }
}
