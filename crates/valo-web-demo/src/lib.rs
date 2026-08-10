//! The interactive board in a real browser — the same integration shape a
//! host app uses: one canvas, a WebGPU surface, a requestAnimationFrame
//! loop, pointer/wheel events. No winit; the DOM is the platform.
//!
//! Web-only: native workspace builds see an empty crate.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use valo::{Color, Context, DisplayList, DisplayListBuilder, FontCollection, Hud, Point, Surface};
use valo_harness::interactive::Camera;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[wasm_bindgen(start)]
pub async fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let canvas = canvas()?;
    let size = fit_canvas_to_window(&canvas);

    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        })
        .await
        .map_err(|e| JsValue::from_str(&format!("no WebGPU adapter: {e:?}")))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("valo.web"),
            required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
            ..Default::default()
        })
        .await
        .map_err(|e| JsValue::from_str(&format!("no device: {e:?}")))?;
    let target = wgpu::SurfaceTarget::Canvas(canvas.clone());
    let surface = Surface::new(&instance, &adapter, &device, target, size)
        .map_err(|e| JsValue::from_str(&format!("no surface: {e:?}")))?;

    let fonts = embedded_fonts();
    let mut ctx = Context::new(device, queue);
    ctx.set_fonts(fonts.clone());
    let board = Arc::new(valo_harness::scenes::figma_board(&fonts));
    let mut camera = Camera {
        offset: Point::ZERO,
        zoom: 1.0,
    };
    if let Some(world) = board.bounds() {
        camera.fit(world, [size[0] as f32, size[1] as f32]);
    }

    let app = Rc::new(RefCell::new(App {
        ctx,
        surface,
        canvas: canvas.clone(),
        fonts,
        board,
        camera,
        cursor: Point::ZERO,
        dragging: false,
        hud: Hud::new("JetBrains Mono"),
        last: valo::RenderStats::default(),
        last_memory: None,
        frames: 0,
    }));
    hook_pointer_events(&canvas, &app)?;
    hook_raf(&app)?;
    Ok(())
}

struct App {
    ctx: Context,
    surface: Surface,
    canvas: web_sys::HtmlCanvasElement,
    fonts: Arc<FontCollection>,
    board: Arc<DisplayList>,
    camera: Camera,
    cursor: Point,
    dragging: bool,
    hud: Hud,
    last: valo::RenderStats,
    last_memory: Option<valo::MemoryReport>,
    frames: u32,
}

impl App {
    fn frame(&mut self) {
        let size = fit_canvas_to_window(&self.canvas);
        if size != self.surface.size() {
            self.surface.resize(size);
        }
        let mut b = DisplayListBuilder::new();
        b.save();
        b.translate(self.camera.offset.x, self.camera.offset.y);
        b.scale(self.camera.zoom, self.camera.zoom);
        b.draw_display_list(&self.board);
        b.restore();
        let note = format!("{:>6.2}x", self.camera.zoom);
        self.hud.draw(
            &mut b,
            &self.fonts,
            &self.last,
            self.last_memory.as_ref(),
            &note,
            size[0] as f32,
        );
        let Some(frame) = self.surface.acquire() else {
            return;
        };
        let stats = self
            .ctx
            .render(&b.build(), &frame.target(Some(Color::rgb(0.08, 0.08, 0.1))));
        frame.present();
        self.last = stats;
        self.frames += 1;
        if self.frames.is_multiple_of(30) {
            self.last_memory = Some(self.ctx.memory_report());
        }
    }
}

fn canvas() -> Result<web_sys::HtmlCanvasElement, JsValue> {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("board"))
        .and_then(|e| e.dyn_into().ok())
        .ok_or_else(|| JsValue::from_str("no <canvas id=\"board\">"))
}

/// CSS size × devicePixelRatio → physical backing size (min 1×1).
fn fit_canvas_to_window(canvas: &web_sys::HtmlCanvasElement) -> [u32; 2] {
    let window = web_sys::window().expect("window");
    let dpr = window.device_pixel_ratio();
    let w = (canvas.client_width() as f64 * dpr).max(1.0) as u32;
    let h = (canvas.client_height() as f64 * dpr).max(1.0) as u32;
    if canvas.width() != w {
        canvas.set_width(w);
    }
    if canvas.height() != h {
        canvas.set_height(h);
    }
    [w, h]
}

fn embedded_fonts() -> Arc<FontCollection> {
    let mut fonts = FontCollection::new();
    fonts
        .register(
            "Fira Sans",
            include_bytes!("../../../assets/fonts/fira_sans.ttf").to_vec(),
        )
        .expect("fira");
    fonts
        .register(
            "JetBrains Mono",
            include_bytes!("../../../assets/fonts/jetbrains_mono.ttf").to_vec(),
        )
        .expect("mono");
    Arc::new(fonts)
}

fn hook_pointer_events(
    canvas: &web_sys::HtmlCanvasElement,
    app: &Rc<RefCell<App>>,
) -> Result<(), JsValue> {
    let dpr = || web_sys::window().expect("window").device_pixel_ratio() as f32;

    // Handlers use try_borrow: the browser may dispatch events while the
    // frame borrow is live (and one leaked borrow after a panic must not
    // cascade) — dropping an input event beats aborting.
    let a = app.clone();
    let down = Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |e: web_sys::PointerEvent| {
        if let Ok(mut app) = a.try_borrow_mut() {
            app.dragging = true;
        }
        if let Some(t) = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        {
            let _ = t.set_pointer_capture(e.pointer_id());
        }
    });
    canvas.add_event_listener_with_callback("pointerdown", down.as_ref().unchecked_ref())?;
    down.forget();

    let a = app.clone();
    let up = Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |_| {
        if let Ok(mut app) = a.try_borrow_mut() {
            app.dragging = false;
        }
    });
    canvas.add_event_listener_with_callback("pointerup", up.as_ref().unchecked_ref())?;
    up.forget();

    let a = app.clone();
    let mv = Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |e: web_sys::PointerEvent| {
        let k = dpr();
        let p = Point::new(e.offset_x() as f32 * k, e.offset_y() as f32 * k);
        let Ok(mut app) = a.try_borrow_mut() else {
            return;
        };
        if app.dragging {
            app.camera.offset = Point::new(
                app.camera.offset.x + p.x - app.cursor.x,
                app.camera.offset.y + p.y - app.cursor.y,
            );
        }
        app.cursor = p;
    });
    canvas.add_event_listener_with_callback("pointermove", mv.as_ref().unchecked_ref())?;
    mv.forget();

    // Wheel = zoom about the cursor. Trackpad pinch arrives as wheel with
    // ctrlKey (the web convention), stronger per tick.
    let a = app.clone();
    let wheel = Closure::<dyn FnMut(web_sys::WheelEvent)>::new(move |e: web_sys::WheelEvent| {
        e.prevent_default();
        let Ok(mut app) = a.try_borrow_mut() else {
            return;
        };
        let strength = if e.ctrl_key() { 0.01 } else { 0.0015 };
        let factor = 1.0 - e.delta_y() as f32 * strength;
        let cursor = app.cursor;
        app.camera.zoom_about(cursor, factor);
    });
    canvas.add_event_listener_with_callback("wheel", wheel.as_ref().unchecked_ref())?;
    wheel.forget();
    Ok(())
}

fn hook_raf(app: &Rc<RefCell<App>>) -> Result<(), JsValue> {
    let window = web_sys::window().expect("window");
    let cb: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let cb2 = cb.clone();
    let a = app.clone();
    let w = window.clone();
    *cb.borrow_mut() = Some(Closure::new(move || {
        if let Ok(mut app) = a.try_borrow_mut() {
            app.frame();
        }
        w.request_animation_frame(
            cb2.borrow()
                .as_ref()
                .expect("raf closure")
                .as_ref()
                .unchecked_ref(),
        )
        .expect("raf");
    }));
    window.request_animation_frame(
        cb.borrow()
            .as_ref()
            .expect("raf closure")
            .as_ref()
            .unchecked_ref(),
    )?;
    Ok(())
}
