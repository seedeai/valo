//! What every draw is handed instead of reading it off the planner: where it
//! lands — its transform and its depth — and how much of it shows, the group
//! alpha of the elided opacity layers around it (Impeller's
//! `distributed_opacity`). The walk makes one from its scope stack for each
//! recorded draw.
//!
//! A draw that a layer of its own holds — an implicit layer, an effect layer,
//! a snapshot — is handed alpha 1: the layer's composite carries the group
//! alpha, and taking it twice would fade the draw twice.

use valo_geometry::Matrix;

use super::draw_context::DepthRange;

/// `ONE_DRAW_LAYER` is the depth line of a layer that holds one draw (an
/// implicit or effect layer): the draw, then nothing above it.
pub(super) const ONE_DRAW_LAYER: DepthRange = DepthRange::new(0, 2);

/// `DrawState` is where one draw lands and how much of it shows.
#[derive(Clone, Copy, Debug)]
pub(super) struct DrawState {
    /// Local coordinates into the current target's replay coordinates.
    pub transform: Matrix,
    /// The draw's depth on the current target's depth line.
    pub z: f32,
    /// The group alpha the draw is drawn at.
    pub alpha: f32,
}

impl DrawState {
    /// `in_layer` is a draw at `z` under `transform` inside a layer whose
    /// composite takes the group alpha, so the draw takes none: alpha 1.
    pub fn in_layer(transform: Matrix, z: f32) -> Self {
        Self {
            transform,
            z,
            alpha: 1.0,
        }
    }

    /// `alone_in_layer` is the state of the one draw a layer of its own
    /// holds, under `transform`: the first slot of [`ONE_DRAW_LAYER`].
    pub fn alone_in_layer(transform: Matrix) -> Self {
        Self::in_layer(transform, ONE_DRAW_LAYER.z(1))
    }

    /// `with_transform` is this state with `transform` in place of its own.
    pub fn with_transform(&self, transform: Matrix) -> Self {
        Self { transform, ..*self }
    }

    /// `faded` is this state drawn at `alpha` times its group alpha: a
    /// composite that takes its paint's alpha as well.
    pub fn faded(&self, alpha: f32) -> Self {
        Self {
            alpha: self.alpha * alpha,
            ..*self
        }
    }
}
