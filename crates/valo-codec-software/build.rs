//! Defines `cfg(animation)`: true when any format with animation support is compiled in, so
//! the compositing path is gated once rather than by repeating the feature list.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(animation)");
    let animated = [
        "CARGO_FEATURE_GIF",
        "CARGO_FEATURE_PNG",
        "CARGO_FEATURE_WEBP",
    ];
    if animated.iter().any(|name| std::env::var_os(name).is_some()) {
        println!("cargo::rustc-cfg=animation");
    }
}
