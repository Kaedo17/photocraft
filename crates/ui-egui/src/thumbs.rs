//! File thumbnails for the in-app file picker's icon views.
//!
//! One decode budget: at most [`BUDGET`] decodes are started per frame (the browser repaints
//! while the budget lasts), and every answer — including "not decodable" — is cached per path in
//! the egui context, so a folder of plain documents never probes twice.
//!
//! The module compiles and is tested on every platform; only the in-app picker's icon views
//! call it, so hosts with their blocking dialog never decode anything.

use std::path::Path;

use photocraft_codecs::{ChannelLayout, SampleType};

/// The longest side a thumbnail is kept at.
pub const MAX_EDGE: u32 = 128;
/// Decodes started per frame.
pub const BUDGET: u32 = 2;

/// A decoded thumbnail: the texture and its pixel size (`None` — also cached — while not decoded
/// yet, and for files with no picture to show).
pub type Thumb = Option<(egui::TextureHandle, [u32; 2])>;

/// Thumbnail of a file (`None` while not decoded yet, and when it has no picture to show).
pub fn get(ctx: &egui::Context, path: &Path) -> Thumb {
    let id = egui::Id::new(("file-thumb", path.to_string_lossy().into_owned()));
    ctx.data(|d| d.get_temp::<Thumb>(id)).flatten()
}

/// Try to decode the next undecoded paths, honouring [`BUDGET`]. Returns the names of the paths
/// it handled, so the caller repaints and re-asks — only while the budget lasts, so a frame
/// never decodes a whole folder.
pub fn load(ctx: &egui::Context, paths: &[std::path::PathBuf]) -> Vec<String> {
    let mut budget = BUDGET;
    let mut done = Vec::new();
    for path in paths {
        if budget == 0 {
            break;
        }
        let id = egui::Id::new(("file-thumb", path.to_string_lossy().into_owned()));
        if ctx.data(|d| d.get_temp::<Thumb>(id)).is_some() {
            continue;
        }
        budget -= 1;
        let thumb = decode(ctx, path);
        ctx.data_mut(|d| d.insert_temp(id, thumb));
        done.push(path.to_string_lossy().into_owned());
    }
    done
}

/// Decode one file to a [`Thumb`] (a texture at most [`MAX_EDGE`] on the long side).
fn decode(ctx: &egui::Context, path: &Path) -> Thumb {
    let thumb = (|| {
        let bytes = std::fs::read(path).ok()?;
        let source = if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pcraft")) {
            // The native format embeds a PNG thumbnail of its own; decode that.
            photocraft_format::read_thumbnail(&bytes).ok().flatten()?
        } else {
            bytes
        };
        let img = photocraft_codecs::decode(&source).ok()?;
        let (w, h) = (img.width(), img.height());
        if w == 0 || h == 0 || w > 16_384 || h > 16_384 {
            return None;
        }
        let rgba = img.convert(ChannelLayout::Rgba, SampleType::U8);
        let step = (w.max(h) as f32 / MAX_EDGE as f32).ceil().max(1.0) as u32;
        let (tw, th) = (w.div_ceil(step), h.div_ceil(step));
        let src = rgba.data();
        let mut px = Vec::with_capacity((tw * th * 4) as usize);
        for y in 0..th {
            for x in 0..tw {
                let i = ((y * step) as usize * w as usize + (x * step) as usize) * 4;
                px.extend_from_slice(&src[i..i + 4]);
            }
        }
        let name = path.to_string_lossy();
        let tex = ctx.load_texture(
            format!("file-thumb-{name}"),
            egui::ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], &px),
            egui::TextureOptions::LINEAR,
        );
        Some((tex, [tw, th]))
    })();
    thumb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        let img = photocraft_codecs::Image::from_u8(w, h, ChannelLayout::Rgba, rgba.to_vec()).unwrap();
        photocraft_codecs::encode(&img, photocraft_codecs::Format::Png, &Default::default()).unwrap()
    }

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("photocraft-thumbs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn decodes_a_real_image_and_scales_down() {
        let dir = dir("scale");
        let path = dir.join("Scene.png");
        std::fs::write(&path, png_bytes(300, 128, &vec![180u8; 300 * 128 * 4])).unwrap();
        let ctx = egui::Context::default();
        assert!(get(&ctx, &path).is_none(), "not decoded until asked");
        assert_eq!(load(&ctx, &[path.clone()]), vec![path.to_string_lossy().into_owned()]);
        let (tex, size) = get(&ctx, &path).expect("decoded");
        assert_eq!(tex.size(), [size[0] as usize, size[1] as usize]);
        assert!(size[0] <= MAX_EDGE && size[1] <= MAX_EDGE, "at most {MAX_EDGE} on the long side: {size:?}");
        assert!(
            (size[0] as f32 / size[1] as f32 * 100.0).round() as i64 - (300.0f32 / 128.0f32 * 100.0).round() as i64 <= 1,
            "kept the picture's aspect: {size:?}"
        );
        // Cached: a second pass does no work.
        assert!(load(&ctx, &[path.clone()]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_without_pictures_cache_as_none() {
        let dir = dir("none");
        let txt = dir.join("notes.txt");
        std::fs::write(&txt, b"not an image").unwrap();
        let ctx = egui::Context::default();
        assert_eq!(load(&ctx, &[txt.clone()]), vec![txt.to_string_lossy().into_owned()], "probed once");
        assert!(get(&ctx, &txt).is_none(), "and stays None (cached)");
        assert!(load(&ctx, &[txt.clone()]).is_empty(), "no second probe");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn budget_spreads_decodes_over_frames() {
        let dir = dir("budget");
        let px = vec![7u8; 4 * 4 * 4];
        let paths: Vec<std::path::PathBuf> = (0..3)
            .map(|i| {
                let p = dir.join(format!("{i}.png"));
                std::fs::write(&p, png_bytes(4, 4, &px)).unwrap();
                p
            })
            .collect();
        let ctx = egui::Context::default();
        let done = load(&ctx, &paths);
        assert_eq!(done.len(), 2, "the [BUDGET] caps one frame");
        assert!(get(&ctx, &paths[0]).is_some());
        assert!(get(&ctx, &paths[1]).is_some());
        assert!(get(&ctx, &paths[2]).is_none(), "waited for the next frame");
        assert_eq!(load(&ctx, &paths), vec![paths[2].to_string_lossy().into_owned()], "the rest next call");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
