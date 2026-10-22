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
/// The longest side a source image may have before it is not decoded at all.
pub const MAX_SOURCE_EDGE: u32 = 16_384;
/// Decodes started per frame.
pub const BUDGET: u32 = 2;

/// A decoded thumbnail: the texture and its pixel size (`None` — also cached — while not decoded
/// yet, and for files with no picture to show).
pub type Thumb = Option<(egui::TextureHandle, [u32; 2])>;

/// The cache key of a file's thumbnail: its path and the file's size and modification time, so a
/// save over that path (or a file that appears where an undecodable one was) is decoded afresh
/// instead of showing the old picture — or the old "no picture" — for the rest of the session.
fn key(path: &Path) -> egui::Id {
    let stamp = std::fs::metadata(path).ok().map(|m| (m.len(), m.modified().ok()));
    egui::Id::new(("file-thumb", path.to_string_lossy().into_owned(), stamp))
}

/// Thumbnail of a file (`None` while not decoded yet, and when it has no picture to show).
pub fn get(ctx: &egui::Context, path: &Path) -> Thumb {
    let id = key(path);
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
        let id = key(path);
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
    (|| {
        let bytes = std::fs::read(path).ok()?;
        let source = if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pcraft")) {
            // The native format embeds a PNG thumbnail of its own; decode that.
            photocraft_format::read_thumbnail(&bytes).ok().flatten()?
        } else {
            bytes
        };
        // Bound the decode itself to the size a thumbnail can use: the default limits allow a
        // 2^30-pixel image, which would allocate gigabytes before the size check below could
        // reject it.
        let mut opts = photocraft_codecs::DecodeOptions::default();
        opts.limits.max_width = MAX_SOURCE_EDGE;
        opts.limits.max_height = MAX_SOURCE_EDGE;
        opts.limits.max_pixels = u64::from(MAX_SOURCE_EDGE) * u64::from(MAX_SOURCE_EDGE);
        let img = photocraft_codecs::decode_with(&source, &opts).ok()?;
        let (w, h) = (img.width(), img.height());
        if w == 0 || h == 0 || w > MAX_SOURCE_EDGE || h > MAX_SOURCE_EDGE {
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
    })()
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
        assert_eq!(load(&ctx, std::slice::from_ref(&path)), vec![path.to_string_lossy().into_owned()]);
        let (tex, size) = get(&ctx, &path).expect("decoded");
        assert_eq!(tex.size(), [size[0] as usize, size[1] as usize]);
        assert!(size[0] <= MAX_EDGE && size[1] <= MAX_EDGE, "at most {MAX_EDGE} on the long side: {size:?}");
        assert!(
            (size[0] as f32 / size[1] as f32 * 100.0).round() as i64 - (300.0f32 / 128.0f32 * 100.0).round() as i64 <= 1,
            "kept the picture's aspect: {size:?}"
        );
        // Cached: a second pass does no work.
        assert!(load(&ctx, std::slice::from_ref(&path)).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_without_pictures_cache_as_none() {
        let dir = dir("none");
        let txt = dir.join("notes.txt");
        std::fs::write(&txt, b"not an image").unwrap();
        let ctx = egui::Context::default();
        assert_eq!(load(&ctx, std::slice::from_ref(&txt)), vec![txt.to_string_lossy().into_owned()], "probed once");
        assert!(get(&ctx, &txt).is_none(), "and stays None (cached)");
        assert!(load(&ctx, std::slice::from_ref(&txt)).is_empty(), "no second probe");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_rewritten_in_place_is_decoded_again() {
        let dir = dir("rewrite");
        let path = dir.join("Scene.png");
        std::fs::write(&path, b"not an image").unwrap();
        let ctx = egui::Context::default();
        assert_eq!(load(&ctx, std::slice::from_ref(&path)).len(), 1, "probed once");
        assert!(get(&ctx, &path).is_none(), "an undecodable file caches as None");
        // The user saves an image over it from the in-app browser: the old answer must not stick.
        std::fs::write(&path, png_bytes(64, 300, &vec![200u8; 64 * 300 * 4])).unwrap();
        assert_eq!(load(&ctx, std::slice::from_ref(&path)).len(), 1, "the new file is decoded");
        let (_, size) = get(&ctx, &path).expect("the overwritten file shows a picture");
        // The same nearest-neighbour downscale `decode` applies: ceil(long side / MAX_EDGE).
        let step = 300u32.div_ceil(MAX_EDGE);
        assert_eq!(size, [64u32.div_ceil(step), 300u32.div_ceil(step)], "the new picture, scaled down: {size:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_image_over_the_size_limit_is_not_decoded() {
        let dir = dir("oversized");
        let path = dir.join("huge.png");
        let w = MAX_SOURCE_EDGE + 1;
        std::fs::write(&path, png_bytes(w, 1, &vec![0u8; w as usize * 4])).unwrap();
        let ctx = egui::Context::default();
        assert_eq!(load(&ctx, std::slice::from_ref(&path)).len(), 1, "probed once");
        assert!(get(&ctx, &path).is_none(), "an image over the limit has no thumbnail");
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
