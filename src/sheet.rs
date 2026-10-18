//! Comparison sheets: one per scene (variants side by side) plus an index of every scene.
//!
//! Drawn on the CPU with `image` + `ab_glyph`, using the font Bevy embeds (FiraMono).

use std::path::{Path, PathBuf};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use image::{Rgb, RgbImage, imageops};

use crate::{meta::Meta, scenes::SceneKind, variant::Variant};

const BG: Rgb<u8> = Rgb([18, 18, 22]);
const PANEL: Rgb<u8> = Rgb([34, 34, 40]);
const WHITE: [u8; 3] = [240, 240, 240];
const GREY: [u8; 3] = [160, 160, 168];
const AMBER: [u8; 3] = [255, 205, 90];
const RED: [u8; 3] = [255, 120, 110];
const GREEN: [u8; 3] = [130, 220, 140];

pub struct Entry {
    pub variant: Variant,
    pub meta: Option<Meta>,
    /// Labelled capture, if it exists.
    pub labelled: Option<PathBuf>,
    /// Unlabelled capture, if it exists.
    pub raw: Option<PathBuf>,
}

pub struct SceneResult {
    pub kind: SceneKind,
    pub entries: Vec<Entry>,
}

struct Painter {
    font: FontRef<'static>,
}

impl Painter {
    fn new() -> Self {
        Self {
            font: FontRef::try_from_slice(bevy::text::DEFAULT_FONT_DATA)
                .expect("Bevy's embedded font parses"),
        }
    }

    fn text_width(&self, px: f32, text: &str) -> f32 {
        let font = self.font.as_scaled(PxScale::from(px));
        text.chars().map(|c| font.h_advance(font.glyph_id(c))).sum()
    }

    /// Draws one line of text with its top-left at (x, y); returns the line height.
    fn text(&self, img: &mut RgbImage, x: f32, y: f32, px: f32, color: [u8; 3], text: &str) -> f32 {
        let scale = PxScale::from(px);
        let font = self.font.as_scaled(scale);
        let mut caret = x;
        for c in text.chars() {
            let id = font.glyph_id(c);
            let glyph = id.with_scale_and_position(scale, point(caret, y + font.ascent()));
            caret += font.h_advance(id);
            let Some(outlined) = self.font.outline_glyph(glyph) else {
                continue;
            };
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i64 + gx as i64;
                let py = bounds.min.y as i64 + gy as i64;
                if px < 0 || py < 0 || px >= img.width() as i64 || py >= img.height() as i64 {
                    return;
                }
                let dst = img.get_pixel_mut(px as u32, py as u32);
                for (d, c) in dst.0.iter_mut().zip(color) {
                    let v = *d as f32;
                    *d = (v + (c as f32 - v) * coverage.min(1.0)) as u8;
                }
            });
        }
        font.height() + font.line_gap()
    }

    /// Word-wrapped text; returns the total height used.
    #[allow(clippy::too_many_arguments)]
    fn paragraph(
        &self,
        img: &mut RgbImage,
        x: f32,
        y: f32,
        max_w: f32,
        px: f32,
        color: [u8; 3],
        text: &str,
    ) -> f32 {
        let mut h = 0.0;
        for line in self.wrap(px, max_w, text) {
            h += self.text(img, x, y + h, px, color, &line);
        }
        h
    }

    fn wrap(&self, px: f32, max_w: f32, text: &str) -> Vec<String> {
        let mut lines = Vec::new();
        let mut cur = String::new();
        for word in text.split_whitespace() {
            let candidate = if cur.is_empty() {
                word.to_owned()
            } else {
                format!("{cur} {word}")
            };
            if self.text_width(px, &candidate) > max_w && !cur.is_empty() {
                lines.push(std::mem::replace(&mut cur, word.to_owned()));
            } else {
                cur = candidate;
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        lines
    }

    fn line_height(&self, px: f32) -> f32 {
        let font = self.font.as_scaled(PxScale::from(px));
        font.height() + font.line_gap()
    }
}

fn fill(img: &mut RgbImage, x: u32, y: u32, w: u32, h: u32, color: Rgb<u8>) {
    for py in y..(y + h).min(img.height()) {
        for px in x..(x + w).min(img.width()) {
            img.put_pixel(px, py, color);
        }
    }
}

fn load_tile(path: Option<&PathBuf>, w: u32, h: u32) -> Option<RgbImage> {
    let img = image::open(path?).ok()?.to_rgb8();
    Some(if img.width() == w && img.height() == h {
        img
    } else {
        imageops::resize(&img, w, h, imageops::FilterType::Triangle)
    })
}

fn status_color(meta: Option<&Meta>) -> [u8; 3] {
    match meta {
        Some(m) if m.is_ok() => GREEN,
        Some(m) if m.status.starts_with("dlss-unsupported") => AMBER,
        _ => RED,
    }
}

fn res_line(meta: Option<&Meta>) -> String {
    match meta {
        Some(m) if m.is_ok() => {
            let r = |v: bevy::math::UVec2| format!("{}x{}", v.x, v.y);
            format!(
                "render {} -> output {} | {}",
                m.render.map(r).unwrap_or_else(|| "?".into()),
                r(m.output),
                m.pipeline
            )
        }
        Some(m) => m.status.clone(),
        None => "not captured".into(),
    }
}

/// `out/<scene>/sheet.png`: header + every variant (labelled shots), side by side.
pub fn scene_sheet(
    result: &SceneResult,
    output: (u32, u32),
    max_tile_w: u32,
    path: &Path,
) -> image::ImageResult<()> {
    let p = Painter::new();
    let info = result.kind.info();
    let n = result.entries.len().max(1) as u32;
    let cols = if n <= 3 { n } else { n.div_ceil(2) };
    let rows = n.div_ceil(cols);
    let tw = output.0.min(max_tile_w);
    let th = (tw as f32 * output.1 as f32 / output.0 as f32).round() as u32;
    let pad = 16;
    let caption_h = (p.line_height(20.0) * 2.0) as u32 + 10;
    let width = cols * tw + (cols + 1) * pad;

    // Header height depends on wrapping, so measure first on a scratch line count.
    let text_w = (width - 2 * pad) as f32;
    let header_lines = 1
        + p.wrap(
            18.0,
            text_w,
            &format!("Feature under test: {}", info.feature),
        )
        .len()
        + p.wrap(
            18.0,
            text_w,
            &format!("Expected under DLSS: {}", result.kind.headline_expected()),
        )
        .len()
        + 1;
    let header_h =
        (p.line_height(30.0) + p.line_height(18.0) * header_lines as f32) as u32 + 2 * pad;
    let height = header_h + rows * (caption_h + th + pad) + pad;

    let mut img = RgbImage::from_pixel(width, height, BG);
    fill(&mut img, 0, 0, width, header_h, PANEL);
    let x = pad as f32;
    let mut y = pad as f32;
    y += p.text(
        &mut img,
        x,
        y,
        30.0,
        WHITE,
        &format!("{}  ({})", info.title, info.name),
    );
    y += p.paragraph(
        &mut img,
        x,
        y,
        text_w,
        18.0,
        GREY,
        &format!("Feature under test: {}", info.feature),
    );
    y += p.paragraph(
        &mut img,
        x,
        y,
        text_w,
        18.0,
        AMBER,
        &format!("Expected under DLSS: {}", result.kind.headline_expected()),
    );
    let adapter = result
        .entries
        .iter()
        .find_map(|e| e.meta.as_ref().map(|m| m.adapter.clone()))
        .unwrap_or_default();
    p.text(
        &mut img,
        x,
        y,
        18.0,
        GREY,
        &format!("Bevy 0.20.0 + dlss_wgpu 6.0.0 (DLSS SDK 310.9.1) | {adapter}"),
    );

    for (i, entry) in result.entries.iter().enumerate() {
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        let x0 = pad + c * (tw + pad);
        let y0 = header_h + pad + r * (caption_h + th + pad);
        let meta = entry.meta.as_ref();
        p.text(
            &mut img,
            x0 as f32,
            y0 as f32,
            20.0,
            WHITE,
            &format!("{}  -  {}", entry.variant, entry.variant.describe()),
        );
        p.text(
            &mut img,
            x0 as f32,
            y0 as f32 + p.line_height(20.0),
            18.0,
            status_color(meta),
            &res_line(meta),
        );
        let ty = y0 + caption_h;
        match load_tile(entry.labelled.as_ref(), tw, th) {
            Some(tile) => imageops::replace(&mut img, &tile, x0 as i64, ty as i64),
            None => {
                fill(&mut img, x0, ty, tw, th, PANEL);
                let msg = meta
                    .map(|m| m.status.clone())
                    .unwrap_or_else(|| "not captured".into());
                p.paragraph(
                    &mut img,
                    x0 as f32 + 20.0,
                    ty as f32 + th as f32 * 0.42,
                    tw as f32 - 40.0,
                    24.0,
                    status_color(meta),
                    &msg,
                );
            }
        }
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    img.save(path)
}

fn column_key(v: &Variant) -> (bool, bool, bool, usize) {
    let mode_idx = v
        .dlss
        .and_then(|m| crate::variant::DlssMode::ALL.iter().position(|x| *x == m))
        .unwrap_or(0);
    (v.dlss.is_some(), v.forward, v.taa, mode_idx)
}

/// `out/index.png`: one row per scene, one column per variant (unlabelled shots).
pub fn index_sheet(
    results: &[SceneResult],
    output: (u32, u32),
    path: &Path,
) -> image::ImageResult<()> {
    let p = Painter::new();
    let mut columns: Vec<Variant> = Vec::new();
    for r in results {
        for e in &r.entries {
            if !columns.contains(&e.variant) {
                columns.push(e.variant);
            }
        }
    }
    columns.sort_by_key(column_key);

    let tw = 420u32;
    let th = (tw as f32 * output.1 as f32 / output.0 as f32).round() as u32;
    let pad = 12u32;
    let head_w = 380u32;
    let col_head_h = 40u32;
    let title_h = 64u32;
    let width = head_w + columns.len() as u32 * (tw + pad) + pad;
    let height = title_h + col_head_h + results.len() as u32 * (th + pad) + pad;
    let mut img = RgbImage::from_pixel(width, height, BG);

    p.text(
        &mut img,
        pad as f32,
        14.0,
        30.0,
        WHITE,
        "bevy_dlss_bench: Bevy features vs DLSS MainPassResolutionOverride",
    );
    for (c, v) in columns.iter().enumerate() {
        let x = head_w + pad + c as u32 * (tw + pad);
        p.text(
            &mut img,
            x as f32,
            (title_h + 8) as f32,
            20.0,
            WHITE,
            &v.name(),
        );
    }
    for (r, result) in results.iter().enumerate() {
        let y = title_h + col_head_h + r as u32 * (th + pad);
        let info = result.kind.info();
        fill(&mut img, 0, y, head_w, th, PANEL);
        let mut ty = y as f32 + 10.0;
        ty += p.paragraph(
            &mut img,
            pad as f32,
            ty,
            (head_w - 2 * pad) as f32,
            22.0,
            WHITE,
            info.title,
        );
        p.paragraph(
            &mut img,
            pad as f32,
            ty + 4.0,
            (head_w - 2 * pad) as f32,
            15.0,
            AMBER,
            result.kind.headline_expected(),
        );
        for (c, v) in columns.iter().enumerate() {
            let x = head_w + pad + c as u32 * (tw + pad);
            let Some(entry) = result.entries.iter().find(|e| e.variant == *v) else {
                continue;
            };
            match load_tile(entry.raw.as_ref(), tw, th) {
                Some(tile) => imageops::replace(&mut img, &tile, x as i64, y as i64),
                None => {
                    fill(&mut img, x, y, tw, th, PANEL);
                    let meta = entry.meta.as_ref();
                    let msg = meta
                        .map(|m| m.status.clone())
                        .unwrap_or_else(|| "not captured".into());
                    p.paragraph(
                        &mut img,
                        x as f32 + 12.0,
                        y as f32 + th as f32 * 0.4,
                        tw as f32 - 24.0,
                        16.0,
                        status_color(meta),
                        &msg,
                    );
                }
            }
        }
    }
    img.save(path)
}
