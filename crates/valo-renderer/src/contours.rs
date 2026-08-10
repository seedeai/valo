use std::collections::HashMap;
use std::sync::{Arc, Weak};

use valo_geometry::{Contour, Path};

/// Frames a flattening survives unused before dropping.
const EVICT_AFTER_FRAMES: u64 = 3;

/// Flattened-contour cache: paths re-used across frames — every
/// retained DL during a pan — flatten ONCE per tolerance bucket instead of
/// every frame (Skia caches by shape genID; our identity is the `Arc`).
/// Zooming crosses buckets and re-flattens, as it must.
pub struct ContourCache {
    map: HashMap<(usize, i32), Entry>,
    frame: u64,
}

struct Entry {
    /// Guards against allocator reuse of a dropped path's address.
    identity: Weak<Path>,
    contours: Arc<Vec<Contour>>,
    last_used: u64,
}

impl ContourCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            frame: 0,
        }
    }

    /// The path's polylines at (bucketed) `tolerance` — cached per identity.
    pub fn contours(&mut self, path: &Arc<Path>, tolerance: f32) -> Arc<Vec<Contour>> {
        let bucket = tolerance_bucket(tolerance);
        let key = (Arc::as_ptr(path) as usize, bucket);
        if let Some(entry) = self.map.get_mut(&key) {
            if entry.identity.as_ptr() == Arc::as_ptr(path) {
                entry.last_used = self.frame;
                return entry.contours.clone();
            }
        }
        let contours = Arc::new(path.flatten(bucket_tolerance(bucket)));
        self.map.insert(
            key,
            Entry {
                identity: Arc::downgrade(path),
                contours: contours.clone(),
                last_used: self.frame,
            },
        );
        contours
    }

    /// Drop idle and dead entries.
    pub fn end_frame(&mut self) {
        self.frame += 1;
        let cutoff = self.frame.saturating_sub(EVICT_AFTER_FRAMES);
        self.map
            .retain(|_, e| e.last_used >= cutoff && e.identity.strong_count() > 0);
    }
}

impl Default for ContourCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Quarter-log2 steps: the used tolerance is within 2^(1/8) ≈ 1.09× of the
/// requested one (visually nil), and a steady scale maps to a steady key.
fn tolerance_bucket(tolerance: f32) -> i32 {
    (tolerance.max(1e-4).log2() * 4.0).round() as i32
}

fn bucket_tolerance(bucket: i32) -> f32 {
    (bucket as f32 / 4.0).exp2()
}

impl ContourCache {
    /// Entries + flattened point bytes (each point is two f32s).
    pub(crate) fn report(&self) -> crate::PoolReport {
        let bytes: usize = self
            .map
            .values()
            .map(|e| {
                e.contours
                    .iter()
                    .map(|c| c.points.len() * std::mem::size_of::<valo_geometry::Point>())
                    .sum::<usize>()
            })
            .sum();
        crate::PoolReport {
            count: self.map.len() as u32,
            bytes: bytes as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use valo_geometry::PathBuilder;

    #[test]
    fn hits_on_same_path_and_scale_reflattens_on_zoom() {
        let mut cache = ContourCache::new();
        let mut b = PathBuilder::new();
        b.circle((0.0, 0.0), 50.0);
        let path = b.build();

        let a = cache.contours(&path, 0.25);
        let b2 = cache.contours(&path, 0.25);
        assert!(Arc::ptr_eq(&a, &b2), "pan = cache hit");

        let zoomed = cache.contours(&path, 0.05);
        assert!(!Arc::ptr_eq(&a, &zoomed), "zoom re-buckets");
        assert!(
            zoomed[0].points.len() > a[0].points.len(),
            "finer tolerance, more segments"
        );
    }

    #[test]
    fn dead_and_idle_entries_evict() {
        let mut cache = ContourCache::new();
        let mut b = PathBuilder::new();
        b.rect(valo_geometry::Rect::new(0.0, 0.0, 10.0, 10.0));
        let path = b.build();
        cache.contours(&path, 0.25);
        assert_eq!(cache.map.len(), 1);
        drop(path);
        cache.end_frame();
        assert_eq!(cache.map.len(), 0, "dropped paths vacate immediately");
    }
}
