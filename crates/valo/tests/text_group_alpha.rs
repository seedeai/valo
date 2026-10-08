//! A glyph run under a group's alpha: the group fades as one picture. A run
//! never takes the alpha glyph by glyph (Flutter's rule), since its glyphs
//! may overlap and an overlap faded twice would darken, so an opacity group
//! around text keeps its layer.

use std::sync::Arc;

use valo::{
    Color, Context, DisplayListBuilder, DrawParagraphExt, FontCollection, Offscreen, Paint,
    ParagraphBuilder, Rect, TextStyle,
};
use valo_dl::GlyphPos;

const SIZE: [u32; 2] = [160, 96];
const BACKGROUND: Color = Color::WHITE;

fn context(test: &str) -> Option<Context> {
    let Some((device, queue)) = valo_harness::headless_device() else {
        eprintln!("SKIP {test}: no GPU adapter");
        return None;
    };
    Some(Context::new(device, queue))
}

fn fonts() -> FontCollection {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    let mut fonts = FontCollection::new();
    fonts
        .register(
            "Fira Sans",
            std::fs::read(format!("{dir}/fira_sans.ttf")).expect("fira_sans.ttf"),
        )
        .expect("register Fira Sans");
    fonts
}

/// `half_alpha` is a layer paint that fades its group to half.
fn half_alpha() -> Paint {
    Paint::from_color(Color::rgba(0.0, 0.0, 0.0, 0.5))
}

/// `Pixels` is a straight-alpha RGBA8 rendering.
struct Pixels(Vec<u8>);

impl Pixels {
    fn at(&self, (x, y): (u32, u32)) -> [u8; 4] {
        let at = ((y * SIZE[0] + x) * 4) as usize;
        [self.0[at], self.0[at + 1], self.0[at + 2], self.0[at + 3]]
    }

    fn covered(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        (0..SIZE[1])
            .flat_map(|y| (0..SIZE[0]).map(move |x| (x, y)))
            .filter(|&at| self.at(at)[3] == 255)
    }
}

/// Two "H"s placed so the second's left stem lies on the first's right
/// one, in a half-alpha group over white: where they overlap the group shows
/// the same blue as where one of them inks alone, with no darker seam.
#[test]
fn overlapping_glyphs_under_a_group_alpha_blend_once() {
    let Some(mut context) = context("overlapping_glyphs_under_a_group_alpha_blend_once") else {
        return;
    };
    let fonts = fonts();
    let id = fonts.family("Fira Sans").expect("Fira Sans");
    let font = fonts.faces().get_arc(id);
    let h = font.glyph_for('H').expect("an H");
    let blue = Paint::from_color(Color::rgb(0.0, 0.0, 1.0));
    let run = |b: &mut DisplayListBuilder, xs: &[f32], paint: &Paint| {
        let glyphs = xs.iter().map(|&x| GlyphPos { id: h, x, y: 72.0 }).collect();
        let bounds = Rect::new(0.0, 0.0, SIZE[0] as f32, SIZE[1] as f32);
        b.draw_glyph_run(Arc::clone(&font), 64.0, paint, Arc::new(glyphs), bounds);
    };
    let alone = |context: &mut Context, x: f32| {
        let mut b = DisplayListBuilder::new();
        run(&mut b, &[x], &blue);
        Pixels(context.render_to_rgba(&b.build(), SIZE, Some(Color::TRANSPARENT)))
    };
    let (first, second) = (alone(&mut context, 20.0), alone(&mut context, 45.0));
    let overlap = first
        .covered()
        .find(|&at| second.at(at)[3] == 255)
        .expect("the two Hs overlap");
    let first_alone = first
        .covered()
        .find(|&at| second.at(at)[3] == 0)
        .expect("the first H inks somewhere alone");

    let mut b = DisplayListBuilder::new();
    b.save_layer(None, &half_alpha());
    run(&mut b, &[20.0, 45.0], &blue);
    b.restore();
    let group = Pixels(context.render_to_rgba(&b.build(), SIZE, Some(BACKGROUND)));
    assert_eq!(
        group.at(overlap),
        group.at(first_alone),
        "the overlap at {overlap:?} against one H alone at {first_alone:?}"
    );
}

/// Plain, well-spaced Latin text inside the same group still keeps the
/// group's layer: a run never takes a group's alpha, however its glyphs lie.
#[test]
fn an_opacity_group_around_text_takes_a_layer() {
    let Some(mut context) = context("an_opacity_group_around_text_takes_a_layer") else {
        return;
    };
    let mut fonts = fonts();
    let mut paragraph = ParagraphBuilder::new(&mut fonts);
    paragraph.add_text(
        "Hum",
        &TextStyle::new("Fira Sans", 48.0, Color::rgb(0.0, 0.0, 1.0)),
    );
    let mut paragraph = paragraph.build();
    paragraph.layout(f32::INFINITY);
    let mut b = DisplayListBuilder::new();
    b.save_layer(None, &half_alpha());
    b.draw_paragraph(&paragraph, (16.0, 16.0));
    b.restore();
    let offscreen = Offscreen::new(context.device(), SIZE);
    let stats = context.render(&b.build(), &offscreen.target(Some(BACKGROUND)));
    assert_eq!(stats.layers_elided, 0);
    assert_eq!(stats.layers_rendered, 1, "the group keeps its layer");
}
