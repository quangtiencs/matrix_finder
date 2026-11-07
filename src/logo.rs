//! The Matrix Finder logo (SVG, rendered with resvg) for the window icon and UI.
use resvg::{tiny_skia, usvg};

pub const LOGO_SVG: &[u8] = include_bytes!("../assets/logo/matrix_finder.svg");
/// macOS variant on Apple's icon grid (smaller body plus shadow), for the Dock.
pub const LOGO_MACOS_SVG: &[u8] = include_bytes!("../assets/logo/matrix_finder-macos.svg");
/// Simplified variant that stays legible at 16–32 px (exported as the small PNGs).
#[cfg_attr(not(test), allow(dead_code))]
pub const LOGO_SMALL_SVG: &[u8] = include_bytes!("../assets/logo/matrix_finder-small.svg");
/// The folder without its tile, for drawing inside the UI.
pub const GLYPH_SVG: &[u8] = include_bytes!("../assets/logo/matrix_finder-glyph.svg");
pub const GLYPH_SMALL_SVG: &[u8] = include_bytes!("../assets/logo/matrix_finder-small-glyph.svg");

/// Render an SVG to straight (non-premultiplied) RGBA at `size`×`size`.
pub fn render(svg: &[u8], size: u32) -> Option<Vec<u8>> {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).ok()?;
    let s = size as f32 / tree.size().width().max(tree.size().height());
    render_tree(&tree, size, size, s)
}

fn render_tree(tree: &usvg::Tree, w: u32, h: u32, s: f32) -> Option<Vec<u8>> {
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(tree, tiny_skia::Transform::from_scale(s, s), &mut pixmap.as_mut());
    // tiny-skia stores premultiplied alpha; window icons expect straight alpha.
    let mut data = pixmap.take();
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Some(data)
}

/// Window / taskbar / dock icon. macOS gets the variant that follows its icon grid,
/// so the Dock icon is the same size as other apps'.
pub fn icon_data() -> Option<egui::IconData> {
    let size = 256;
    let svg = if cfg!(target_os = "macos") { LOGO_MACOS_SVG } else { LOGO_SVG };
    render(svg, size).map(|rgba| egui::IconData { rgba, width: size, height: size })
}

/// Build an Apple `.icns` file (PNG-encoded entries) from the macOS SVG.
#[cfg(test)]
pub fn icns(svg: &[u8]) -> Option<Vec<u8>> {
    // (type, pixel size): 1x and @2x entries from 16 to 1024 px.
    const ENTRIES: [(&[u8; 4], u32); 10] = [
        (b"icp4", 16),
        (b"icp5", 32),
        (b"ic11", 32),
        (b"icp6", 64),
        (b"ic12", 64),
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic13", 256),
        (b"ic09", 512),
        (b"ic10", 1024),
    ];
    let mut body = Vec::new();
    for (kind, size) in ENTRIES {
        let png = png_bytes(&render(svg, size)?, size, size)?;
        body.extend_from_slice(kind);
        body.extend_from_slice(&(png.len() as u32 + 8).to_be_bytes());
        body.extend_from_slice(&png);
    }
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(&body);
    Some(out)
}

#[cfg(test)]
pub fn png_bytes(rgba: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(rgba, w, h, image::ExtendedColorType::Rgba8).ok()?;
    Some(out)
}

/// Render an SVG that contains text, with the bundled UI font.
#[cfg(test)]
pub fn render_with_fonts(svg: &[u8], scale: f32) -> Option<(Vec<u8>, u32, u32)> {
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_font_data(include_bytes!("../assets/fonts/SourceSans3-Regular.otf").to_vec());
    let tree = usvg::Tree::from_data(svg, &opt).ok()?;
    let (w, h) = ((tree.size().width() * scale).round() as u32, (tree.size().height() * scale).round() as u32);
    Some((render_tree(&tree, w, h, scale)?, w, h))
}

/// Texture for drawing the logo inside the UI (`px` = physical size): the folder
/// alone, without the icon tile, so it sits cleanly on any theme.
pub fn texture(ctx: &egui::Context, small: bool, px: u32) -> Option<egui::TextureHandle> {
    let svg = if small { GLYPH_SMALL_SVG } else { GLYPH_SVG };
    let rgba = render(svg, px)?;
    let img = egui::ColorImage::from_rgba_unmultiplied([px as usize, px as usize], &rgba);
    Some(ctx.load_texture(format!("logo-{small}-{px}"), img, egui::TextureOptions::LINEAR))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders both variants; set `MATRIX_FINDER_LOGO_OUT=dir` to also write PNGs.
    #[test]
    fn logo_renders() {
        for (name, svg) in [("matrix_finder", LOGO_SVG), ("matrix_finder-macos", LOGO_MACOS_SVG), ("matrix_finder-small", LOGO_SMALL_SVG), ("glyph", GLYPH_SVG), ("glyph-small", GLYPH_SMALL_SVG)] {
            for size in [16u32, 32, 64, 256, 512] {
                let rgba = render(svg, size).expect("logo renders");
                assert_eq!(rgba.len(), (size * size * 4) as usize);
                // Not blank: the yellow folder is there.
                assert!(rgba.chunks_exact(4).any(|p| p[0] > 220 && p[1] > 140 && p[2] < 130 && p[3] > 200), "{name} {size}");
                if let Some(dir) = std::env::var_os("MATRIX_FINDER_LOGO_OUT") {
                    let path = std::path::Path::new(&dir).join(format!("{name}-{size}.png"));
                    image::save_buffer(&path, &rgba, size, size, image::ColorType::Rgba8).unwrap();
                }
            }
        }
    }

    #[test]
    fn icns_layout() {
        let data = icns(LOGO_MACOS_SVG).unwrap();
        assert_eq!(&data[..4], b"icns");
        assert_eq!(u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize, data.len());
        assert_eq!(&data[8..12], b"icp4");
        // The macOS icon leaves Apple's ~10% margin transparent.
        let px = render(LOGO_MACOS_SVG, 1024).unwrap();
        let alpha = |x: usize, y: usize| px[(y * 1024 + x) * 4 + 3];
        assert_eq!(alpha(40, 512), 0);
        assert_eq!(alpha(512, 512), 255);
    }

    /// Regenerate the PNG / ICNS / DMG-background assets:
    /// `cargo test export_assets -- --ignored`
    #[test]
    #[ignore]
    fn export_assets() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let save = |path: std::path::PathBuf, rgba: &[u8], w: u32, h: u32| {
            image::save_buffer(&path, rgba, w, h, image::ColorType::Rgba8).unwrap();
        };
        for size in [64u32, 256, 512] {
            save(root.join(format!("logo/matrix_finder-{size}.png")), &render(LOGO_SVG, size).unwrap(), size, size);
        }
        for size in [16u32, 32] {
            save(root.join(format!("logo/matrix_finder-small-{size}.png")), &render(LOGO_SMALL_SVG, size).unwrap(), size, size);
        }
        save(root.join("logo/matrix_finder-macos-512.png"), &render(LOGO_MACOS_SVG, 512).unwrap(), 512, 512);
        std::fs::write(root.join("macos/MatrixFinder.icns"), icns(LOGO_MACOS_SVG).unwrap()).unwrap();
        let bg = std::fs::read(root.join("macos/dmg-background.svg")).unwrap();
        for (scale, name) in [(1.0, "dmg-background.png"), (2.0, "dmg-background@2x.png")] {
            let (rgba, w, h) = render_with_fonts(&bg, scale).unwrap();
            save(root.join("macos").join(name), &rgba, w, h);
        }
    }
}
