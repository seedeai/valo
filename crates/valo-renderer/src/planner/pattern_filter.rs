//! A pattern paint's colour filter, baked into a cached filtered copy of its
//! image (Impeller's `TiledTextureContents`): the filter runs once over one
//! immutable source, and the pattern transform and tile sampler then apply
//! to the copy, so the filter is on what the paint fills — where a mask blur
//! wants it.
//!
//! A cache miss renders the copy with a pass of its own, appended at once.
//! It runs before the top target's segment, whose steps so far cannot read a
//! texture made this frame, so the segment goes on unsplit.

use valo_dl::{ColorFilter, Image, Paint, Shader};
use valo_geometry::Rect;

use crate::images::IMAGE_FORMAT;

use super::filter_passes::FilterPasses;
use super::Planner;

impl Planner<'_> {
    /// `bake_pattern_colour_filter` is a pattern paint with its colour
    /// filter baked into a cached filtered copy of its image; `None` for any
    /// other paint.
    pub(super) fn bake_pattern_colour_filter(&mut self, paint: &Paint) -> Option<Paint> {
        let filter = paint.color_filter?;
        let Some(Shader::Image {
            image,
            sampling,
            local,
        }) = &paint.shader
        else {
            return None;
        };
        let baked = Shader::Image {
            image: self.bake_filtered_image(image, filter),
            sampling: *sampling,
            local: *local,
        };
        Some(Paint {
            shader: Some(baked),
            color_filter: None,
            ..paint.clone()
        })
    }

    /// `bake_filtered_image` is the cached copy of `source` through
    /// `filter`, rendered now on a cache miss.
    fn bake_filtered_image(&mut self, source: &Image, filter: ColorFilter) -> Image {
        let (filtered, created) = self.tools.emit.filtered_image(source, filter);
        if created {
            self.filter_passes()
                .push_recolour_into(source, filter, &filtered);
        }
        filtered
    }
}

impl FilterPasses<'_, '_> {
    /// `push_recolour_into` recolours the whole of `source` into `copy`, a
    /// texture of its size in the image format.
    fn push_recolour_into(&mut self, source: &Image, filter: ColorFilter, copy: &Image) {
        let whole = Rect::new(0.0, 0.0, source.width(), source.height());
        let shading = self.emit.bake_shading(source, filter);
        self.push_pass(
            copy.view().clone(),
            IMAGE_FORMAT,
            &whole,
            source.size(),
            shading,
        );
    }
}
