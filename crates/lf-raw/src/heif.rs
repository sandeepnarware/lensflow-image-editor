//! HEIC (and HEIF) photos, decoded by the computer's own codecs rather than a bundled one:
//! HEVC is patent-encumbered and libheif is LGPL, and every system already has a decoder.
//!
//! - Windows: Windows Imaging Component, through Microsoft's "HEIF Image Extensions" and
//!   "HEVC Video Extensions" (from the Microsoft Store; many PCs have them already).
//! - macOS: ImageIO, through `sips` (part of every macOS).
//! - Linux: libheif's `heif-dec` (or the older `heif-convert`), from the distribution's
//!   `libheif` tools package.
//!
//! When the decoder is missing the error says what to install. EXIF (capture time, camera,
//! GPS) is read from the file itself ([`crate::exif::read_summary`] understands HEIF).

use image::RgbaImage;
use std::path::Path;

/// Why a HEIC could not be opened, when the computer lacks the decoder.
#[cfg(windows)]
pub const MISSING: &str = "To open HEIC photos, install \"HEIF Image Extensions\" and \"HEVC Video Extensions\" from the Microsoft Store.";
#[cfg(target_os = "macos")]
pub const MISSING: &str = "This Mac could not decode the HEIC photo.";
#[cfg(all(unix, not(target_os = "macos")))]
pub const MISSING: &str = "To open HEIC photos, install libheif's tools (the package that provides heif-dec, e.g. libheif-examples).";

/// The photo at `path` as 8-bit RGBA, as the system decodes it.
pub fn decode(path: &Path) -> Result<RgbaImage, String> {
    platform::decode(path)
}

#[cfg(windows)]
mod platform {
    use super::MISSING;
    use image::RgbaImage;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory,
        WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::core::PCWSTR;

    /// WINCODEC_ERR_COMPONENTNOTFOUND: no codec for the file (the extension is missing).
    const NO_CODEC: i32 = 0x8898_2F50_u32 as i32;

    pub fn decode(path: &Path) -> Result<RgbaImage, String> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let text = |e: windows::core::Error| {
            if e.code().0 == NO_CODEC {
                MISSING.to_owned()
            } else {
                format!("Windows could not decode it ({e}).")
            }
        };
        // SAFETY: plain COM calls on this thread; every interface is released when dropped,
        // and the pixel buffer is exactly the size `CopyPixels` is told.
        unsafe {
            // Already initialised (another apartment kind) is fine: WIC works in either.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(text)?;
            let decoder = factory
                .CreateDecoderFromFilename(
                    PCWSTR(wide.as_ptr()),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .map_err(text)?;
            let frame = decoder.GetFrame(0).map_err(text)?;
            let converter = factory.CreateFormatConverter().map_err(text)?;
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppRGBA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(text)?;
            let (mut w, mut h) = (0u32, 0u32);
            converter.GetSize(&mut w, &mut h).map_err(text)?;
            let mut buf = vec![0u8; w as usize * h as usize * 4];
            converter
                .CopyPixels(std::ptr::null(), w * 4, &mut buf)
                .map_err(text)?;
            RgbaImage::from_raw(w, h, buf).ok_or_else(|| "The decoded size is wrong.".to_owned())
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::MISSING;
    use image::RgbaImage;
    use std::path::Path;
    use std::process::Command;

    /// Converts with the first tool that works, into a temporary PNG, and reads it.
    pub fn decode(path: &Path) -> Result<RgbaImage, String> {
        let dir = tempfile_dir()?;
        let out = dir.join("heic.png");
        let tools: Vec<Vec<std::ffi::OsString>> = if cfg!(target_os = "macos") {
            vec![vec![
                "sips".into(),
                "-s".into(),
                "format".into(),
                "png".into(),
                path.into(),
                "--out".into(),
                out.clone().into(),
            ]]
        } else {
            vec![
                vec!["heif-dec".into(), path.into(), out.clone().into()],
                vec!["heif-convert".into(), path.into(), out.clone().into()],
            ]
        };
        let mut last = MISSING.to_owned();
        for tool in tools {
            match Command::new(&tool[0]).args(&tool[1..]).output() {
                Ok(o) if o.status.success() && out.exists() => {
                    let img = image::open(&out).map_err(|e| e.to_string())?.to_rgba8();
                    let _ = std::fs::remove_dir_all(&dir);
                    return Ok(img);
                }
                Ok(o) => last = String::from_utf8_lossy(&o.stderr).trim().to_owned(),
                Err(_) => {}
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        Err(if last.is_empty() {
            MISSING.to_owned()
        } else {
            last
        })
    }

    fn tempfile_dir() -> Result<std::path::PathBuf, String> {
        let dir = std::env::temp_dir().join(format!(
            "lensflow-heic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_that_is_not_a_heic_is_refused_with_a_reason() {
        let dir = std::env::temp_dir().join(format!("lf-heif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("broken.heic");
        std::fs::write(&path, b"\0\0\0\x18ftypheic\0\0\0\0mif1heic").unwrap();
        let e = decode(&path).unwrap_err();
        assert!(!e.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::GENERIC_WRITE;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_ContainerFormatHeif, GUID_WICPixelFormat32bppBGRA,
        IWICImagingFactory, WICBitmapEncoderNoCache,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    };

    /// Writes a 64 × 48 picture (left half red, right half blue) as HEIC with Windows' own
    /// encoder; `None` where this Windows has none.
    fn write_heic(path: &std::path::Path) -> Option<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            let stream = factory.CreateStream().ok()?;
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
            stream
                .InitializeFromFilename(windows::core::PCWSTR(wide.as_ptr()), GENERIC_WRITE.0)
                .ok()?;
            let encoder = factory
                .CreateEncoder(&GUID_ContainerFormatHeif, std::ptr::null())
                .ok()?;
            encoder.Initialize(&stream, WICBitmapEncoderNoCache).ok()?;
            let mut frame = None;
            encoder
                .CreateNewFrame(&mut frame, std::ptr::null_mut())
                .ok()?;
            let frame = frame?;
            frame.Initialize(None).ok()?;
            frame.SetSize(64, 48).ok()?;
            let mut format = GUID_WICPixelFormat32bppBGRA;
            frame.SetPixelFormat(&mut format).ok()?;
            let mut px = Vec::with_capacity(64 * 48 * 4);
            for _ in 0..48 {
                for x in 0..64 {
                    px.extend(if x < 32 {
                        [0, 0, 255, 255]
                    } else {
                        [255, 0, 0, 255]
                    });
                }
            }
            frame.WritePixels(48, 64 * 4, &px).ok()?;
            frame.Commit().ok()?;
            encoder.Commit().ok()?;
        }
        Some(())
    }

    #[test]
    fn a_heic_made_by_windows_opens() {
        let dir = std::env::temp_dir().join(format!("lf-heic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("made.heic");
        if write_heic(&path).is_none() {
            eprintln!("skipped: this Windows has no HEIF encoder");
            return;
        }
        let img = super::decode(&path).expect("decodes");
        assert_eq!(img.dimensions(), (64, 48));
        let left = img.get_pixel(10, 24).0;
        let right = img.get_pixel(54, 24).0;
        assert!(left[0] > 180 && left[2] < 90, "left is red: {left:?}");
        assert!(right[2] > 180 && right[0] < 90, "right is blue: {right:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
