//! The raster cost picture. Times the two CPU raster
//! paths per glyph across the tier sizes, latin vs CJK, so tier thresholds
//! and generator work are chosen from numbers instead of vibes.
//!
//! Run: `cargo run --release -p valo-text --example raster_bench`

use std::time::Instant;

use valo_text::{FaceSet, FontId, Rasterizer};

const SIZES: [f32; 6] = [16.0, 32.0, 72.0, 162.0, 256.0, 324.0];
const LATIN: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmn";
const CJK: &str =
    "春夏秋冬风花雪月山水天地日出而作息万物生长设计海报标题正文字体渲染引擎性能测试基准数据";

fn main() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    // A CJK face is 20k+ glyphs and too big to vendor: point VALO_CJK_FONT
    // at one to get the CJK column, otherwise the bench runs latin only.
    let cjk_file = std::env::var("VALO_CJK_FONT").unwrap_or_default();
    let mut fonts = FaceSet::default();
    let latin = fonts
        .register(
            "Latin",
            std::fs::read(format!("{dir}/fira_sans.ttf")).unwrap(),
        )
        .unwrap();
    let cjk = std::fs::read(cjk_file)
        .ok()
        .and_then(|bytes| fonts.register("CJK", bytes));

    let mut sets = vec![("latin", latin, glyphs(&fonts, latin, LATIN))];
    if let Some(id) = cjk {
        sets.push(("cjk", id, glyphs(&fonts, id, CJK)));
    }

    println!(
        "{:>6} {:>6}  {:>12} {:>12}  (µs/glyph, n={})",
        "set",
        "px",
        "alpha",
        "sdf",
        LATIN.len()
    );
    let mut raster = Rasterizer::new();
    for (label, font, ids) in &sets {
        for px in SIZES {
            let alpha = time_per_glyph(ids, |g| {
                raster.alpha(fonts.get(*font), g, px, 0.0);
            });
            let mut raster2 = Rasterizer::new();
            let sdf = time_per_glyph(ids, |g| {
                raster2.sdf(fonts.get(*font), g, px);
            });
            println!("{label:>6} {px:>6.0}  {alpha:>10.1}µs {sdf:>10.1}µs");
        }
    }
}

fn glyphs(fonts: &FaceSet, id: FontId, text: &str) -> Vec<u32> {
    let font = fonts.get(id);
    text.chars().filter_map(|ch| font.glyph_for(ch)).collect()
}

fn time_per_glyph(ids: &[u32], mut raster: impl FnMut(u32)) -> f64 {
    let t0 = Instant::now();
    for &g in ids {
        raster(g);
    }
    t0.elapsed().as_secs_f64() * 1e6 / ids.len() as f64
}
