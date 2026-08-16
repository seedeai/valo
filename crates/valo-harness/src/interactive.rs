//! Interactive pan/zoom runner: any retained scene becomes a playable demo
//! (one scene fn + one call, like `run_example` for stills). Drag pans,
//! scroll/pinch zooms about the cursor, `1` = 100%, `0` = fit, Esc quits.
//! The HUD is drawn BY valo (frosted glass + text) — the demo dogfoods
//! backdrop blur and the text tiers while reporting its own stats.

use std::sync::Arc;

use valo::{
    Color, Context, DisplayList, DisplayListBuilder, FontCollection, Hud, MemoryReport, Point,
    Rect, RenderStats, Surface,
};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// world → screen: `screen = world · zoom + offset` (physical pixels).
/// Shared by the native runner and the web demo — one zoom math.
pub struct Camera {
    pub offset: Point,
    pub zoom: f32,
}

impl Camera {
    /// Zoom so the world point under `cursor` stays put on screen.
    pub fn zoom_about(&mut self, cursor: Point, factor: f32) {
        let zoom = (self.zoom * factor).clamp(0.02, 64.0);
        let k = zoom / self.zoom;
        self.offset = Point::new(
            cursor.x - (cursor.x - self.offset.x) * k,
            cursor.y - (cursor.y - self.offset.y) * k,
        );
        self.zoom = zoom;
    }

    /// Fit `world` into `screen` with a margin, centered.
    pub fn fit(&mut self, world: Rect, screen: [f32; 2]) {
        let zoom = (screen[0] / world.width)
            .min(screen[1] / world.height)
            .min(1.0)
            * 0.94;
        self.zoom = zoom;
        self.offset = Point::new(
            (screen[0] - world.width * zoom) * 0.5 - world.x * zoom,
            (screen[1] - world.height * zoom) * 0.5 - world.y * zoom,
        );
    }
}

/// Open a window and run `scene` under a pan/zoom camera until closed.
pub fn run_pan_zoom(title: &str, fonts: FontCollection, scene: Arc<DisplayList>) {
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App {
        title: title.to_owned(),
        fonts,
        scene,
        state: None,
    };
    event_loop.run_app(&mut app).expect("event loop run");
}

struct App {
    title: String,
    fonts: FontCollection,
    scene: Arc<DisplayList>,
    state: Option<State>,
}

struct State {
    window: Arc<Window>,
    surface: Surface,
    ctx: Context,
    camera: Camera,
    cursor: Point,
    dragging: bool,
    hud: Hud,
    /// Previous frame's numbers — this frame's HUD content.
    last: RenderStats,
    last_memory: Option<MemoryReport>,
    frames: u32,
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
                        .with_title(&self.title)
                        .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0)),
                )
                .expect("create window"),
        );
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
                    label: Some("valo.interactive"),
                    // Without this the GPU column reads 0.0 forever.
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

        let ctx = Context::new(device, queue);
        let mut camera = Camera {
            offset: Point::ZERO,
            zoom: 1.0,
        };
        if let Some(world) = self.scene.bounds() {
            camera.fit(world, [size.width as f32, size.height as f32]);
        }
        self.state = Some(State {
            window,
            surface,
            ctx,
            camera,
            cursor: Point::ZERO,
            dragging: false,
            hud: Hud::new("JetBrains Mono"),
            last: RenderStats::default(),
            last_memory: None,
            frames: 0,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.surface.resize([size.width, size.height]),
            WindowEvent::MouseInput {
                button: MouseButton::Left,
                state: pressed,
                ..
            } => state.dragging = pressed == ElementState::Pressed,
            WindowEvent::CursorMoved { position, .. } => {
                let p = Point::new(position.x as f32, position.y as f32);
                if state.dragging {
                    state.camera.offset = Point::new(
                        state.camera.offset.x + p.x - state.cursor.x,
                        state.camera.offset.y + p.y - state.cursor.y,
                    );
                }
                state.cursor = p;
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 24.0,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                state.camera.zoom_about(state.cursor, 1.0 + dy * 0.0015);
            }
            WindowEvent::PinchGesture { delta, .. } => {
                state.camera.zoom_about(state.cursor, 1.0 + delta as f32);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Character(ref c) if c == "1" => state.camera.zoom = 1.0,
                    Key::Character(ref c) if c == "0" => {
                        if let Some(world) = self.scene.bounds() {
                            let [w, h] = state.surface.size();
                            state.camera.fit(world, [w as f32, h as f32]);
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                state.redraw(&mut self.fonts, &self.scene);
                state.window.request_redraw(); // free-run: fps stays honest
            }
            _ => {}
        }
    }
}

impl State {
    fn redraw(&mut self, fonts: &mut FontCollection, scene: &Arc<DisplayList>) {
        let [w, _h] = self.surface.size();
        let mut b = DisplayListBuilder::new();
        b.save();
        b.translate(self.camera.offset.x, self.camera.offset.y);
        b.scale(self.camera.zoom, self.camera.zoom);
        b.draw_display_list(scene);
        b.restore();
        // valo's own overlay: last frame's stats, memory refreshed sparsely.
        let note = format!("{:>6.2}x", self.camera.zoom);
        self.hud.draw(
            &mut b,
            fonts,
            &self.last,
            self.last_memory.as_ref(),
            &note,
            w as f32,
        );

        let Some(frame) = self.surface.acquire() else {
            return;
        };
        let stats = self
            .ctx
            .render(&b.build(), &frame.target(Some(Color::rgb(0.08, 0.08, 0.1))));
        self.ctx.present(frame);
        self.last = stats;
        self.frames += 1;
        if self.frames.is_multiple_of(30) {
            self.last_memory = Some(self.ctx.memory_report());
        }
    }
}
