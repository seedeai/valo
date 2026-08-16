//! Interactive native window (no browser): animated transform stack + retained
//! nested list. `cargo run -p valo --example window`

use std::sync::Arc;
use std::time::Instant;

use valo::{Color, Context, DisplayListBuilder, Paint, Rect, Surface};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

struct State {
    window: Arc<Window>,
    surface: Surface,
    ctx: Context,
    card: Arc<valo::DisplayList>,
    start: Instant,
}

#[derive(Default)]
struct App {
    state: Option<State>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("valo — M1")
                        .with_inner_size(winit::dpi::LogicalSize::new(900.0, 600.0)),
                )
                .expect("create window"),
        );

        // Metal/DX12 need no display handle; a Wayland host would pass the
        // winit display via `new_with_display_handle`.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let (device, queue, adapter) = pollster::block_on(async {
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    ..Default::default()
                })
                .await
                .expect("adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("valo.window"),
                    required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
                    ..Default::default()
                })
                .await
                .expect("device");
            (device, queue, adapter)
        });

        let size = window.inner_size();
        let surface = Surface::new(
            &instance,
            &adapter,
            &device,
            window.clone(),
            [size.width, size.height],
        )
        .expect("valo surface");

        let card = {
            let mut b = DisplayListBuilder::new();
            b.draw_rect(
                Rect::new(0.0, 0.0, 140.0, 100.0),
                &Paint::from_color(Color::rgb(0.16, 0.17, 0.22)),
            );
            b.draw_rect(
                Rect::new(12.0, 12.0, 116.0, 24.0),
                &Paint::from_color(Color::rgb(0.35, 0.55, 1.0)),
            );
            b.draw_rect(
                Rect::new(12.0, 46.0, 90.0, 12.0),
                &Paint::from_color(Color::rgb(0.45, 0.48, 0.58)),
            );
            Arc::new(b.build())
        };

        self.state = Some(State {
            window,
            surface,
            ctx: Context::new(device, queue),
            card,
            start: Instant::now(),
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.surface.resize([size.width, size.height]),
            WindowEvent::RedrawRequested => {
                let t = state.start.elapsed().as_secs_f32();
                let [w, h] = state.surface.size();
                let dl = record(t, [w as f32, h as f32], &state.card);

                if let Some(frame) = state.surface.acquire() {
                    let stats = state
                        .ctx
                        .render(&dl, &frame.target(Some(Color::rgb(0.07, 0.07, 0.09))));
                    state.ctx.present(frame);
                    state.window.set_title(&format!(
                        "valo — draws {} · culled {} · {:.2}ms cpu",
                        stats.draws, stats.culled, stats.cpu_ms
                    ));
                }
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}

fn record(t: f32, size: [f32; 2], card: &Arc<valo::DisplayList>) -> valo::DisplayList {
    let mut b = DisplayListBuilder::new();

    // Orbiting rotated bars around the center.
    for i in 0..12 {
        b.save();
        b.translate(size[0] * 0.5, size[1] * 0.45);
        b.rotate(t * 0.6 + i as f32 * std::f32::consts::TAU / 12.0);
        b.draw_rect(
            Rect::new(60.0, -10.0, 150.0 + 40.0 * (t * 1.3).sin(), 20.0),
            &Paint::from_color(Color::rgba(0.2, 0.75, 0.55, 0.8)),
        );
        b.restore();
    }

    // Alpha overlap row that breathes.
    let spread = 40.0 + 18.0 * (t * 0.8).sin();
    for i in 0..6 {
        b.draw_rect(
            Rect::new(40.0 + i as f32 * spread, 40.0, 90.0, 60.0),
            &Paint::from_color(Color::rgba(0.9, 0.3, 0.3, 0.5)),
        );
    }

    // Spinning self-intersecting star: stencil-then-cover, even-odd, live.
    let star = {
        let mut p = valo::PathBuilder::new();
        for i in 0..5 {
            let a = i as f32 * 4.0 * std::f32::consts::PI / 5.0;
            let pt = (80.0 * a.cos(), 80.0 * a.sin());
            if i == 0 {
                p.move_to(pt);
            } else {
                p.line_to(pt);
            }
        }
        p.close();
        p.build()
    };
    b.save();
    b.translate(size[0] - 150.0, 150.0);
    b.rotate(t * 0.7);
    b.draw_path(
        &star,
        valo::FillRule::EvenOdd,
        &Paint::from_color(Color::rgba(0.95, 0.7, 0.3, 0.9)),
    );
    b.restore();

    // The retained card, stamped in a moving row (re-recording touches only
    // these few composition ops; the card's own ops never re-record).
    for i in 0..3 {
        b.save();
        b.translate(
            40.0 + i as f32 * 180.0,
            size[1] - 160.0 + 20.0 * ((t + i as f32) * 1.1).sin(),
        );
        b.draw_display_list(card);
        b.restore();
    }

    b.build()
}

fn main() {
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.run_app(&mut App::default()).expect("run");
}
