//! Image and video decode.
//!
//! Stills are PNG and JPEG, decoded in-process (M3a). Video is ffmpeg as a child
//! process feeding raw frames over a pipe — see [`video`], which documents why a
//! pipe and not FFI, with the measurements.
//!
//! # Straight in, premultiplied at the boundary
//!
//! Decoders hand back **straight** alpha; the renderer composites in
//! **premultiplied** (ADR 0012). The multiply happens here, once, at the point
//! the pixels enter the system — the same rule as the output boundary, in the
//! other direction. Compositing a straight-alpha image would put a dark fringe
//! around every soft edge, which is the classic version of this bug and is
//! invisible until it is over live video.
//!
//! # No filesystem — for stills, and only for stills
//!
//! Image decoding takes bytes, not paths. The bytes come from the CAS, and
//! keeping that path dependency-light and IO-free is what lets the same still
//! decoder run in the browser for Design mode.
//!
//! **Video cannot honour that and does not pretend to.** It takes a path,
//! spawns a process, and is compiled out entirely on `wasm32` — the browser has
//! `WebCodecs` and would never want this. The split is deliberate: one half of
//! this crate stays portable, the other half is frankly native, and the module
//! boundary is where you can see which is which.

use std::io::Cursor;

#[cfg(not(target_arch = "wasm32"))]
pub mod video;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("unrecognised image format")]
    UnknownFormat,
    #[error("png: {0}")]
    Png(String),
    #[error("jpeg: {0}")]
    Jpeg(String),
    #[error("image is {w}x{h}, which exceeds the {limit}px limit for one texture")]
    TooLarge { w: u32, h: u32, limit: u32 },
    /// ffmpeg or ffprobe is not on PATH, or would not start.
    ///
    /// Named separately from a decode failure because the fix is completely
    /// different: one is "install ffmpeg", the other is "this file is broken".
    #[error("ffmpeg is not available: {0}")]
    NoFfmpeg(String),
    #[error("could not read this file: {0}")]
    Probe(String),
    #[error("this file has no video stream")]
    NoVideoStream,
    /// Refused rather than accommodated (`docs/RISKS.md` R5): a variable-rate
    /// source has no answer to "which frame is at 4.000 s", so every cue against
    /// it would be approximately right.
    #[error(
        "this clip has a variable frame rate ({avg:.3} average against {nominal:.3} nominal), \
         so it cannot be cued to an exact frame"
    )]
    VariableFrameRate { nominal: f64, avg: f64 },
}

/// Decoded pixels, **premultiplied** RGBA8, ready for the renderer.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The largest edge we will hand to a texture.
///
/// The renderer asks for `downlevel_webgl2_defaults`, so 8192 is the tier a
/// browser is guaranteed to grant. Refusing here, with the size in the message,
/// beats a GPU validation error that names nothing an operator can act on.
pub const MAX_EDGE: u32 = 8192;

/// Sniff the format and decode.
///
/// Magic bytes rather than a filename: assets arrive from the CAS named by their
/// hash, so there is no extension to trust — and trusting one would be worse
/// than having none.
pub fn decode_image(bytes: &[u8]) -> Result<Decoded, MediaError> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        decode_png(bytes)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        decode_jpeg(bytes)
    } else {
        Err(MediaError::UnknownFormat)
    }
}

fn check_size(w: u32, h: u32) -> Result<(), MediaError> {
    if w > MAX_EDGE || h > MAX_EDGE {
        return Err(MediaError::TooLarge {
            w,
            h,
            limit: MAX_EDGE,
        });
    }
    Ok(())
}

fn decode_png(bytes: &[u8]) -> Result<Decoded, MediaError> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    // Normalise 16-bit down and expand palettes and low bit depths, so the rest
    // of this function only ever sees 8-bit RGB or RGBA.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|e| MediaError::Png(e.to_string()))?;
    let info = reader.info().clone();
    check_size(info.width, info.height)?;

    let mut buf = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|e| MediaError::Png(e.to_string()))?;
    buf.truncate(frame.buffer_size());

    let px = (info.width * info.height) as usize;
    let mut rgba = vec![0u8; px * 4];
    match frame.color_type {
        png::ColorType::Rgba => {
            rgba.copy_from_slice(&buf[..px * 4]);
            premultiply(&mut rgba);
        }
        png::ColorType::Rgb => {
            for i in 0..px {
                rgba[i * 4] = buf[i * 3];
                rgba[i * 4 + 1] = buf[i * 3 + 1];
                rgba[i * 4 + 2] = buf[i * 3 + 2];
                rgba[i * 4 + 3] = 255;
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for i in 0..px {
                let (g, a) = (buf[i * 2], buf[i * 2 + 1]);
                rgba[i * 4] = g;
                rgba[i * 4 + 1] = g;
                rgba[i * 4 + 2] = g;
                rgba[i * 4 + 3] = a;
            }
            premultiply(&mut rgba);
        }
        png::ColorType::Grayscale => {
            for i in 0..px {
                let g = buf[i];
                rgba[i * 4] = g;
                rgba[i * 4 + 1] = g;
                rgba[i * 4 + 2] = g;
                rgba[i * 4 + 3] = 255;
            }
        }
        other => {
            return Err(MediaError::Png(format!(
                "unsupported colour type {other:?}"
            )))
        }
    }

    Ok(Decoded {
        width: info.width,
        height: info.height,
        rgba,
    })
}

fn decode_jpeg(bytes: &[u8]) -> Result<Decoded, MediaError> {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    let pixels = decoder
        .decode()
        .map_err(|e| MediaError::Jpeg(e.to_string()))?;
    let info = decoder
        .info()
        .ok_or_else(|| MediaError::Jpeg("no frame header".into()))?;
    check_size(info.width as u32, info.height as u32)?;

    let px = (info.width as usize) * (info.height as usize);
    let mut rgba = vec![0u8; px * 4];
    match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            for i in 0..px {
                rgba[i * 4] = pixels[i * 3];
                rgba[i * 4 + 1] = pixels[i * 3 + 1];
                rgba[i * 4 + 2] = pixels[i * 3 + 2];
                rgba[i * 4 + 3] = 255;
            }
        }
        jpeg_decoder::PixelFormat::L8 => {
            for i in 0..px {
                let g = pixels[i];
                rgba[i * 4] = g;
                rgba[i * 4 + 1] = g;
                rgba[i * 4 + 2] = g;
                rgba[i * 4 + 3] = 255;
            }
        }
        other => {
            return Err(MediaError::Jpeg(format!(
                "unsupported pixel format {other:?}"
            )))
        }
    }
    // JPEG has no alpha, so there is nothing to premultiply — and a JPEG used
    // as a keyed graphic will key as a solid rectangle, which is the format's
    // limitation rather than ours.
    Ok(Decoded {
        width: info.width as u32,
        height: info.height as u32,
        rgba,
    })
}

/// Straight → premultiplied, in place.
fn premultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 255 {
            continue;
        }
        // Rounded, not truncated: repeated truncation darkens midtones
        // measurably over an image, and the cost is one add.
        px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
        px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
        px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
    }
}

pub fn selftest() -> Result<(), String> {
    // A 2x2 PNG with one half-transparent pixel, checked for the premultiply.
    // Built here rather than read from disk so the selftest needs no fixtures.
    let mut png_bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png_bytes, 2, 2);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        // Opaque white, half-alpha white, transparent white, opaque black.
        w.write_image_data(&[
            255, 255, 255, 255, 255, 255, 255, 128, 255, 255, 255, 0, 0, 0, 0, 255,
        ])
        .map_err(|e| e.to_string())?;
    }
    let d = decode_image(&png_bytes).map_err(|e| e.to_string())?;
    if (d.width, d.height) != (2, 2) {
        return Err(format!("decoded {}x{}, expected 2x2", d.width, d.height));
    }
    if d.rgba[4] != 128 {
        return Err(format!(
            "half-alpha white should premultiply to 128, got {}",
            d.rgba[4]
        ));
    }
    if d.rgba[8] != 0 {
        return Err(format!(
            "transparent white should premultiply to 0, got {}",
            d.rgba[8]
        ));
    }

    // The A/V clock, checked at a length no test clip will ever be. Two hours of
    // 59.94 against 29.97 media must land on exactly half the frame count; a
    // float-seconds conversion drifts here, and being one frame out at the end of
    // a service is precisely the bug that never gets found in a ten-second test.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let c = video::MediaClock::new((60000, 1001), (30000, 1001));
        let two_hours = 431_568u64;
        if c.media_frame(two_hours) != two_hours / 2 {
            return Err(format!(
                "two hours of 2:1 pulldown drifted: got {}, expected {}",
                c.media_frame(two_hours),
                two_hours / 2
            ));
        }
    }
    Ok(())
}
