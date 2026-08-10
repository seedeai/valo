use std::sync::{Arc, Mutex};

/// Frame GPU timing via `TIMESTAMP_QUERY` — inert (all zeros) when the
/// device lacks the feature. Timestamps ride the frame's FIRST and LAST
/// render passes as pass-boundary writes (the portable path — Apple GPUs
/// only sample at stage boundaries; encoder-level timestamps hang their
/// command buffers). Results resolve through a small staging ring and
/// surface one-plus frames later as `RenderStats::gpu_ms` — telemetry,
/// never backpressure.
pub struct GpuTimer {
    inner: Option<Timer>,
}

struct Timer {
    query_set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    staging: Vec<Slot>,
    /// Nanoseconds per timestamp tick (device-specific).
    period: f32,
    frame: usize,
    last_ms: f32,
}

struct Slot {
    buffer: wgpu::Buffer,
    state: Arc<Mutex<SlotState>>,
}

/// Free → (copy encoded) Copied → (map requested) Pending → Ready → Free.
/// The explicit Copied stage is what guarantees ONE `map_async` per copy —
/// a slot that skipped its copy never gets a stray map request.
#[derive(PartialEq)]
enum SlotState {
    Free,
    Copied,
    Pending,
    Ready,
}

const RING: usize = 3;

impl GpuTimer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return Self { inner: None };
        }
        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("valo.gpu_timer"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("valo.gpu_timer.resolve"),
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = (0..RING)
            .map(|_| Slot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("valo.gpu_timer.staging"),
                    size: 16,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(Mutex::new(SlotState::Free)),
            })
            .collect();
        Self {
            inner: Some(Timer {
                query_set,
                resolve,
                staging,
                period: queue.get_timestamp_period(),
                frame: 0,
                last_ms: 0.0,
            }),
        }
    }

    /// Timestamp writes for pass `index` of `count`: the first pass stamps
    /// query 0 at its beginning, the last stamps query 1 at its end.
    pub fn pass_writes(
        &self,
        index: usize,
        count: usize,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let timer = self.inner.as_ref()?;
        let first = index == 0;
        let last = index + 1 == count;
        if !first && !last {
            return None;
        }
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &timer.query_set,
            beginning_of_pass_write_index: first.then_some(0),
            end_of_pass_write_index: last.then_some(1),
        })
    }

    /// After the frame's passes: resolve the pair and stage the readback
    /// (skipped when this frame's ring slot is still in flight).
    pub fn end_frame(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(timer) = &mut self.inner else {
            return;
        };
        encoder.resolve_query_set(&timer.query_set, 0..2, &timer.resolve, 0);
        timer.frame += 1;
        let slot = &timer.staging[timer.frame % RING];
        let mut state = slot.state.lock().expect("timer lock");
        if *state != SlotState::Free {
            return;
        }
        *state = SlotState::Copied;
        encoder.copy_buffer_to_buffer(&timer.resolve, 0, &slot.buffer, 0, 16);
    }

    /// After submit: request the map for the slot this frame copied into.
    pub fn after_submit(&self) {
        let Some(timer) = &self.inner else {
            return;
        };
        let slot = &timer.staging[timer.frame % RING];
        {
            let mut state = slot.state.lock().expect("timer lock");
            if *state != SlotState::Copied {
                return;
            }
            *state = SlotState::Pending;
        }
        let state = slot.state.clone();
        slot.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let mut state = state.lock().expect("timer lock");
                *state = if result.is_ok() {
                    SlotState::Ready
                } else {
                    SlotState::Free
                };
            });
    }

    /// Harvest finished readbacks and return the freshest frame time.
    pub fn latest_ms(&mut self, device: &wgpu::Device) -> f32 {
        let Some(timer) = &mut self.inner else {
            return 0.0;
        };
        let _ = device.poll(wgpu::PollType::Poll);
        for slot in &timer.staging {
            if *slot.state.lock().expect("timer lock") != SlotState::Ready {
                continue;
            }
            let ticks: Vec<u64> = {
                let view = slot.buffer.get_mapped_range(..);
                bytemuck::cast_slice(&view).to_vec()
            };
            slot.buffer.unmap();
            *slot.state.lock().expect("timer lock") = SlotState::Free;
            timer.last_ms = ticks[1].saturating_sub(ticks[0]) as f32 * timer.period / 1e6;
        }
        timer.last_ms
    }
}
