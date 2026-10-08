//! A filter stage's result, Impeller's `Snapshot`: a texture of exactly its
//! content, and its [`Placement`] — the transform that places its texels in
//! the space its filter tree runs in (a target's replay coordinates, or a
//! draw's source space, which the composite turns into the target's). A
//! blur's snapshot is bigger than its input (the halo) and at a lower
//! resolution, and its transform says so; a consumer finds what it needs of
//! a snapshot through that transform, never through a uv rect kept beside
//! it. Plans work on placements alone, so they need no GPU.
//!
//! The invariant a later editor will break: the whole texture is content.
//! A target bigger than what its pass writes, a bucketed or reused texture,
//! would hand a reader texels that no pass of this snapshot wrote.

use valo_dl::Image;
use valo_geometry::{Matrix, Rect};

/// `Snapshot` is a texture and where its texels go.
#[derive(Clone)]
pub(super) struct Snapshot {
    pub view: wgpu::TextureView,
    pub placement: Placement,
}

/// `Placement` is where a texture's texels go in the tree's space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Placement {
    /// The texture's extent in texels, all of it content.
    pub size: [u32; 2],
    /// Maps texel coordinates into the tree's space.
    pub transform: Matrix,
    /// The texels lie where the tree's composite draws them, the tree's
    /// space being the target's: a layer composited where it lies, a copy of
    /// the parent beneath a backdrop. A blur reading such a texture may cut
    /// it to its coverage hint (Impeller compares the input snapshot's
    /// transform with the snapshot entity's).
    pub placed: bool,
}

impl Snapshot {
    /// `of_image` is `image`'s own texture drawn over `dst` under
    /// `transform`: Impeller's `TextureContents` snapshot, the texture handed
    /// through as it is.
    pub fn of_image(image: &Image, dst: &Rect, transform: &Matrix) -> Self {
        let [width, height] = image.size().map(|size| size as f32);
        Self {
            view: image.view().clone(),
            placement: Placement {
                size: image.size(),
                transform: transform
                    .then(&Matrix::translation(dst.x, dst.y))
                    .then(&Matrix::scale(dst.width / width, dst.height / height)),
                placed: false,
            },
        }
    }

    /// `placed_where_drawn` is this snapshot, its texels lying where the
    /// composite draws them (see [`Placement::placed`]).
    pub fn placed_where_drawn(mut self, placed: bool) -> Self {
        self.placement.placed = placed;
        self
    }
}

impl Placement {
    /// `coverage` is Impeller's `Snapshot::GetCoverage`: the texture's
    /// bounds in the tree's space.
    pub fn coverage(&self) -> Rect {
        let texels = Rect::new(0.0, 0.0, self.size[0] as f32, self.size[1] as f32);
        self.transform.map_rect(&texels)
    }

    /// `texel_rect` maps `region`, in the tree's space, into texels.
    pub fn texel_rect(&self, region: &Rect) -> Rect {
        self.texel_transform().map_rect(region)
    }

    /// `texel_transform` maps the tree's space into texels: the inverse of
    /// [`Placement::transform`]. A placement's transform always scales by a
    /// nonzero amount, so the inverse exists.
    pub fn texel_transform(&self) -> Matrix {
        self.transform
            .invert()
            .expect("a snapshot's transform is invertible")
    }

    /// `uv_mapping` is Impeller's `GetUVTransform` in the form the fragments
    /// read it, `uv = p · [x, y] + [z, w]` for `p` in the tree's space. It
    /// holds for a transform that only translates and scales, as Impeller's
    /// `CalculateSnapshotUVs` requires of the snapshots it maps.
    pub fn uv_mapping(&self) -> [f32; 4] {
        let [a, b, c, d, tx, ty] = self.transform.to_affine();
        debug_assert!(b == 0.0 && c == 0.0, "uv mapping of a turned snapshot");
        let (w, h) = (self.size[0] as f32, self.size[1] as f32);
        [1.0 / (a * w), 1.0 / (d * h), -tx / (a * w), -ty / (d * h)]
    }

    /// `region_uv_mapping` is [`Placement::uv_mapping`] for a pass whose own
    /// pixels count from `region`'s origin: what a filter pass covering
    /// `region` samples this snapshot with.
    pub fn region_uv_mapping(&self, region: &Rect) -> [f32; 4] {
        let [x, y, z, w] = self.uv_mapping();
        [x, y, region.x * x + z, region.y * y + w]
    }
}
