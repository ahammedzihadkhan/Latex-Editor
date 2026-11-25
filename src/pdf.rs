use eframe::egui::ColorImage;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use std::path::Path;

pub struct RenderedPage {
    pub image: ColorImage,
    pub pt_width: f32,
    pub pt_height: f32,
}

/// Render up to `max_pages` pages of a PDF file into egui ColorImages using pure-Rust hayro.
pub fn render_pdf_file(path: &Path, scale: f32, max_pages: usize) -> Result<Vec<RenderedPage>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read PDF: {e}"))?;
    let pdf = Pdf::new(bytes).map_err(|e| format!("Failed to parse PDF: {e:?}"))?;

    let cache = RenderCache::new();
    let interpreter_settings = InterpreterSettings::default();
    let render_settings = RenderSettings::default();
    let pixmap_settings = PixmapSettings {
        x_scale: scale,
        y_scale: scale,
        bg_color: WHITE,
    };

    let mut out = Vec::with_capacity(pdf.pages().len().min(max_pages));
    for page in pdf.pages().iter().take(max_pages) {
        let (pt_w, pt_h) = page.render_dimensions();
        let pixmap = render(page, &cache, &interpreter_settings, &render_settings, &pixmap_settings);
        let w = pixmap.width() as usize;
        let h = pixmap.height() as usize;
        let image = ColorImage::from_rgba_premultiplied([w, h], pixmap.data_as_u8_slice());
        out.push(RenderedPage {
            image,
            pt_width: pt_w,
            pt_height: pt_h,
        });
    }
    Ok(out)
}
