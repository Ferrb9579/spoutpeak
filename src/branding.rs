//! All app branding is rendered from the supplied SVG, including Windows icons.

const LOGO_SVG: &[u8] = include_bytes!("../assets/logo.svg");

/// Render straight-alpha RGBA. Square canvases centre the logo for OS icons;
/// in-app logos retain the SVG's original aspect ratio.
pub fn render_logo(max_edge: u32, square: bool) -> Option<(u32, u32, Vec<u8>)> {
    let tree = resvg::usvg::Tree::from_data(LOGO_SVG, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = max_edge.max(1) as f32 / size.width().max(size.height());
    let (width, height) = if square {
        (max_edge.max(1), max_edge.max(1))
    } else {
        ((size.width() * scale).ceil() as u32, (size.height() * scale).ceil() as u32)
    };
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    let transform = resvg::tiny_skia::Transform::from_row(
        scale, 0.0, 0.0, scale,
        (width as f32 - size.width() * scale) / 2.0,
        (height as f32 - size.height() * scale) / 2.0,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut rgba = pixmap.take();
    // resvg produces premultiplied pixels; native image and icon APIs expect
    // straight alpha. Demultiplying preserves clean edges on bright surfaces.
    for pixel in rgba.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha > 0 && alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    Some((width, height, rgba))
}
