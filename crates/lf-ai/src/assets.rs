//! The files the AI tools need, fetched the first time a tool is used: ONNX Runtime itself
//! (Microsoft's official build, from its GitHub release) and each model. Nothing is bundled
//! with the app. Every file is pinned by size and SHA-256 and checked before it is used; a
//! file that does not match is deleted, never run. Downloads go through a temporary name, so
//! a stopped or failed download leaves nothing half-written.
//!
//! The licences: ONNX Runtime MIT; IS-Net (subject) Apache-2.0; SkySeg MIT; SCUNet
//! Apache-2.0; Real-ESRGAN BSD-3-Clause; LaMa Apache-2.0; YuNet MIT;
//! open-closed-eye-0001 Apache-2.0; MediaPipe selfie multiclass Apache-2.0; SFace Apache-2.0;
//! CLIP ViT-B/32 MIT.
//! See `NOTICE`.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::AiError;

/// How a download becomes the file the app uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unpack {
    /// The download is the file.
    File,
    /// A zip archive: keep the member with this file name.
    Zip(&'static str),
    /// A gzipped tar archive: keep the regular file whose name starts with this.
    Tgz(&'static str),
    /// A gzipped tar archive: keep every regular file whose path contains this, flat, in a
    /// folder named as the asset.
    TgzFolder(&'static str),
}

/// One file to fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    /// What the app calls it (and its file name in the AI folder).
    pub file: &'static str,
    pub url: &'static str,
    /// The download's size and SHA-256 (of the archive, for an archive).
    pub size: u64,
    pub sha256: &'static str,
    pub unpack: Unpack,
}

/// The ONNX Runtime version the app loads.
pub const RUNTIME_VERSION: &str = "1.31.1";

/// ONNX Runtime for this computer, if there is an official build for it.
pub fn runtime() -> Option<Asset> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return Some(Asset {
        file: "onnxruntime-1.31.1.dll",
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-win-x64-1.31.1.zip",
        size: 82_929_944,
        sha256: "5869e26bb25314375f2d5ef1877e3ea06b1543d6c710e6e4a1ec2f7177ab4b2c",
        unpack: Unpack::Zip("onnxruntime.dll"),
    });
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return Some(Asset {
        file: "libonnxruntime-1.31.1.so",
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-linux-x64-1.31.1.tgz",
        size: 11_413_655,
        sha256: "c849e36a1ef3e22d4ff217b9d1ce4a3cf5b9467b975610913ae959ad5b16bdc9",
        unpack: Unpack::Tgz("libonnxruntime.so.1"),
    });
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return Some(Asset {
        file: "libonnxruntime-1.31.1.so",
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-linux-aarch64-1.31.1.tgz",
        size: 10_332_381,
        sha256: "5317505e2da99be65f8e2775ba3c2afd6d2961fd78aba1bd8f56159f48820c78",
        unpack: Unpack::Tgz("libonnxruntime.so.1"),
    });
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Some(Asset {
        file: "libonnxruntime-1.31.1.dylib",
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-osx-arm64-1.31.1.tgz",
        size: 36_936_435,
        sha256: "2e1344a1926ee50eb25b38153cb6992c3567515aef0e677a1a520a1798896d6f",
        unpack: Unpack::Tgz("libonnxruntime.1"),
    });
    #[allow(unreachable_code)]
    None
}

/// IS-Net (general use): the main subject, 1024 × 1024.
pub const SUBJECT: Asset = Asset {
    file: "isnet-general-use.onnx",
    url: "https://github.com/danielgatis/rembg/releases/download/v0.0.0/isnet-general-use.onnx",
    size: 178_648_008,
    sha256: "60920e99c45464f2ba57bee2ad08c919a52bbf852739e96947fbb4358c0d964a",
    unpack: Unpack::File,
};

/// SkySeg: the sky, 320 × 320.
pub const SKY: Asset = Asset {
    file: "skyseg.onnx",
    url: "https://huggingface.co/bukuroo/SkySeg-ONNX/resolve/main/skyseg.onnx",
    size: 175_997_079,
    sha256: "ab9c34c64c3d821220a2886a4a06da4642ffa14d5b30e8d5339056a089aa1d39",
    unpack: Unpack::File,
};

/// SCUNet (real-world, PSNR): blind denoising; the graph and its weights.
pub const DENOISE: Asset = Asset {
    file: "scunet_color_real_psnr.onnx",
    url: "https://huggingface.co/Heliosoph/scunet-onnx/resolve/main/scunet_color_real_psnr.onnx",
    size: 3_798_678,
    sha256: "231be201ab413dbc999d7951caa9844846b93a12a40a41e037d6b5888ed4e88c",
    unpack: Unpack::File,
};
pub const DENOISE_DATA: Asset = Asset {
    file: "scunet_color_real_psnr.onnx.data",
    url: "https://huggingface.co/Heliosoph/scunet-onnx/resolve/main/scunet_color_real_psnr.onnx.data",
    size: 73_138_176,
    sha256: "98825ea1210b641c71e5f052f582c70c49fd44b35387ebe2c034268c17df3feb",
    unpack: Unpack::File,
};

/// Real-ESRGAN general x4v3: 4× super-resolution.
pub const UPSCALE: Asset = Asset {
    file: "realesr-general-x4v3.onnx",
    url: "https://huggingface.co/Heliosoph/realesrgan-onnx/resolve/main/realesr-general-x4v3.onnx",
    size: 4_871_181,
    sha256: "09b757accd747d7e423c1d352b3e8f23e77cc5742d04bae958d4eb8082b76fa4",
    unpack: Unpack::File,
};

/// LaMa (big-lama, fp32): inpainting at 512 × 512.
pub const INPAINT: Asset = Asset {
    file: "lama_fp32.onnx",
    url: "https://huggingface.co/Carve/LaMa-ONNX/resolve/main/lama_fp32.onnx",
    size: 208_044_816,
    sha256: "1faef5301d78db7dda502fe59966957ec4b79dd64e16f03ed96913c7a4eb68d6",
    unpack: Unpack::File,
};

/// YuNet (2023mar): faces, 640 × 640.
pub const FACES: Asset = Asset {
    file: "face_detection_yunet_2023mar.onnx",
    url: "https://huggingface.co/opencv/face_detection_yunet/resolve/main/face_detection_yunet_2023mar.onnx",
    size: 232_589,
    sha256: "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
    unpack: Unpack::File,
};

/// SFace (OpenCV zoo, 2021dec, Apache-2.0): a 128-number description of a face aligned to
/// 112 × 112, for telling people apart. Its input normalisation is part of the graph.
pub const IDENTITY: Asset = Asset {
    file: "face_recognition_sface_2021dec.onnx",
    url: "https://huggingface.co/opencv/face_recognition_sface/resolve/3d7082438a6e4551e840c9b2bb60b71e8da4b524/face_recognition_sface_2021dec.onnx",
    size: 38_696_353,
    sha256: "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
    unpack: Unpack::File,
};

/// CLIP ViT-B/32's image model (OpenAI, MIT; int8-quantised ONNX by Xenova): a photo as 512
/// numbers in the space the keyword list's text descriptions share, at 224 × 224.
pub const CLIP_VISION: Asset = Asset {
    file: "clip-vit-base-patch32-vision-int8.onnx",
    url: "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/6b7c523703ae26e39be4b03774966f8571c7f523/onnx/vision_model_quantized.onnx",
    size: 89_117_001,
    sha256: "583fd1110a514667812fee7d684952aaf82a99b959760c8d7dca7e0ab9839299",
    unpack: Unpack::File,
};

/// MediaPipe's selfie multiclass segmenter (Google, Apache-2.0), 256 × 256: background,
/// hair, body skin, face skin, clothes, accessories. The TFLite model converted to ONNX
/// with its weights unchanged (the conversion's card pins the source TFLite's SHA-256).
pub const PEOPLE: Asset = Asset {
    file: "selfie_multiclass_256x256.onnx",
    url: "https://huggingface.co/senty-au/selfie_multiclass_256x256-ONNX/resolve/6db8421a7150ac20558f2c24675078eb3a1a04d0/onnx/model.onnx",
    size: 16_454_560,
    sha256: "35ec1ecd9ee7f85073c99c00020b7f6751b69506eeacf683bc8665f6117f85b0",
    unpack: Unpack::File,
};

/// Intel's open-closed-eye-0001 (Open Model Zoo, Apache-2.0): whether an eye is closed,
/// 32 × 32.
pub const EYES: Asset = Asset {
    file: "open_closed_eye.onnx",
    url: "https://storage.openvinotoolkit.org/repositories/open_model_zoo/public/2022.1/open-closed-eye-0001/open_closed_eye.onnx",
    size: 46_164,
    sha256: "4daa100034482525a26c9afb9297c16580a531189e66e3d2b2ac7d32becfd593",
    unpack: Unpack::File,
};

/// The lensfun lens database (CC-BY-SA 3.0): its `data/db` XML files from a pinned
/// snapshot, for automatic lens profiles (not AI, but fetched and checked the same way).
pub const LENSFUN: Asset = Asset {
    file: "lensfun-db",
    url: "https://codeload.github.com/lensfun/lensfun/tar.gz/bc5615d6278cb5fb75dd53e74015367453d5591d",
    size: 2_938_798,
    sha256: "b56bff238bd9b8f6e517785de9361a11608f0d2afbfa9e21ce4a33d69d5c2046",
    unpack: Unpack::TgzFolder("/data/db/"),
};

/// Where `asset` lives in `dir` once fetched.
pub fn path(dir: &Path, asset: &Asset) -> PathBuf {
    dir.join(asset.file)
}

/// Whether `asset` is already in `dir` (by name and, for a plain file, size).
pub fn present(dir: &Path, asset: &Asset) -> bool {
    match fs::metadata(path(dir, asset)) {
        Ok(m) if asset.unpack == Unpack::File => m.len() == asset.size,
        Ok(m) if matches!(asset.unpack, Unpack::TgzFolder(_)) => m.is_dir(),
        Ok(m) => m.len() > 0,
        Err(_) => false,
    }
}

/// How much `assets` still need to download, in bytes.
pub fn missing_bytes(dir: &Path, assets: &[Asset]) -> u64 {
    assets
        .iter()
        .filter(|a| !present(dir, a))
        .map(|a| a.size)
        .sum()
}

/// Makes sure `asset` is in `dir`, downloading and checking it if not. `progress(done,
/// total)` in bytes as it downloads; `stop` ends it (nothing is kept).
pub fn ensure(
    dir: &Path,
    asset: &Asset,
    progress: &mut dyn FnMut(u64, u64),
    stop: &AtomicBool,
) -> Result<PathBuf, AiError> {
    let out = path(dir, asset);
    if present(dir, asset) {
        return Ok(out);
    }
    fs::create_dir_all(dir).map_err(|e| AiError::Io(format!("the AI folder ({e})")))?;
    let part = dir.join(format!("{}.part", asset.file));
    let result = download(asset, &part, progress, stop).and_then(|()| unpack(asset, &part, &out));
    let _ = fs::remove_file(&part);
    result.map(|()| out)
}

fn download(
    asset: &Asset,
    to: &Path,
    progress: &mut dyn FnMut(u64, u64),
    stop: &AtomicBool,
) -> Result<(), AiError> {
    log::info!("downloading {} from {}", asset.file, asset.url);
    let client = reqwest::blocking::Client::builder()
        .timeout(None)
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| AiError::Download(e.to_string()))?;
    let mut response = client
        .get(asset.url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| AiError::Download(e.to_string()))?;
    let mut file =
        fs::File::create(to).map_err(|e| AiError::Io(format!("{} ({e})", to.display())))?;
    let mut hash = Sha256::new();
    let mut done = 0u64;
    let mut buf = vec![0u8; 1 << 16];
    progress(0, asset.size);
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err(AiError::Stopped);
        }
        let n = response
            .read(&mut buf)
            .map_err(|e| AiError::Download(e.to_string()))?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > asset.size {
            return Err(AiError::Mismatch(asset.file));
        }
        hash.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| AiError::Io(format!("{} ({e})", to.display())))?;
        progress(done, asset.size);
    }
    file.sync_all()
        .map_err(|e| AiError::Io(format!("{} ({e})", to.display())))?;
    if done != asset.size || hex::encode(hash.finalize()) != asset.sha256 {
        return Err(AiError::Mismatch(asset.file));
    }
    Ok(())
}

/// The checked download at `part` becomes `out` (renamed, or its member taken out).
fn unpack(asset: &Asset, part: &Path, out: &Path) -> Result<(), AiError> {
    let tmp = out.with_extension("unpacking");
    let io = |e: std::io::Error| AiError::Io(format!("{} ({e})", out.display()));
    match asset.unpack {
        Unpack::File => {
            fs::rename(part, out).map_err(io)?;
            return Ok(());
        }
        Unpack::Zip(name) => {
            let file = fs::File::open(part).map_err(io)?;
            let mut zip =
                zip::ZipArchive::new(file).map_err(|e| AiError::Download(e.to_string()))?;
            let index = (0..zip.len())
                .find(|&i| {
                    zip.by_index(i).is_ok_and(|f| {
                        f.is_file() && f.name().is_ok_and(|n| n.rsplit('/').next() == Some(name))
                    })
                })
                .ok_or(AiError::Mismatch(asset.file))?;
            let mut member = zip
                .by_index(index)
                .map_err(|e| AiError::Download(e.to_string()))?;
            let mut dest = fs::File::create(&tmp).map_err(io)?;
            std::io::copy(&mut member, &mut dest).map_err(io)?;
        }
        Unpack::Tgz(prefix) => {
            let file = fs::File::open(part).map_err(io)?;
            let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
            let mut found = false;
            for entry in tar.entries().map_err(io)? {
                let mut entry = entry.map_err(io)?;
                let regular = entry.header().entry_type().is_file();
                let name = entry
                    .path()
                    .ok()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
                if regular && name.is_some_and(|n| n.starts_with(prefix)) {
                    let mut dest = fs::File::create(&tmp).map_err(io)?;
                    std::io::copy(&mut entry, &mut dest).map_err(io)?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(AiError::Mismatch(asset.file));
            }
        }
        Unpack::TgzFolder(within) => {
            let file = fs::File::open(part).map_err(io)?;
            let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
            let _ = fs::remove_dir_all(&tmp);
            fs::create_dir_all(&tmp).map_err(io)?;
            let mut found = 0;
            for entry in tar.entries().map_err(io)? {
                let mut entry = entry.map_err(io)?;
                if !entry.header().entry_type().is_file() {
                    continue;
                }
                let path = entry
                    .path()
                    .map_err(io)?
                    .to_string_lossy()
                    .replace('\\', "/");
                // Flat, by file name only: nothing in the archive can write outside the folder.
                let name = path.rsplit('/').next().unwrap_or_default().to_owned();
                if path.contains(within) && !name.is_empty() && !name.starts_with('.') {
                    let mut dest = fs::File::create(tmp.join(&name)).map_err(io)?;
                    std::io::copy(&mut entry, &mut dest).map_err(io)?;
                    found += 1;
                }
            }
            if found == 0 {
                let _ = fs::remove_dir_all(&tmp);
                return Err(AiError::Mismatch(asset.file));
            }
            let _ = fs::remove_dir_all(out);
        }
    }
    fs::rename(&tmp, out).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_asset_is_pinned() {
        let all = [
            SUBJECT,
            SKY,
            DENOISE,
            DENOISE_DATA,
            UPSCALE,
            INPAINT,
            FACES,
            EYES,
            PEOPLE,
            LENSFUN,
        ]
        .into_iter()
        .chain(runtime());
        for a in all {
            assert_eq!(a.sha256.len(), 64, "{}", a.file);
            assert!(a.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(a.url.starts_with("https://"), "{}", a.file);
            assert!(a.size > 0);
        }
    }

    #[test]
    fn a_present_file_is_not_fetched_again_and_sizes_add_up() {
        let dir = tempfile::tempdir().unwrap();
        let small = Asset {
            file: "x.onnx",
            url: "https://example.invalid/x.onnx",
            size: 3,
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            unpack: Unpack::File,
        };
        assert_eq!(missing_bytes(dir.path(), &[small]), 3);
        fs::write(dir.path().join("x.onnx"), b"abc").unwrap();
        assert!(present(dir.path(), &small));
        let got = ensure(dir.path(), &small, &mut |_, _| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(got, dir.path().join("x.onnx"));
        assert_eq!(missing_bytes(dir.path(), &[small]), 0);
        // A file of the wrong size is not taken as present.
        fs::write(dir.path().join("x.onnx"), b"abcd").unwrap();
        assert!(!present(dir.path(), &small));
    }

    #[test]
    fn a_zip_member_is_taken_out() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("rt.zip.part");
        {
            let mut zip = zip::ZipWriter::new(fs::File::create(&part).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("pkg/lib/other.dll", opts).unwrap();
            zip.write_all(b"no").unwrap();
            zip.start_file("pkg/lib/onnxruntime.dll", opts).unwrap();
            zip.write_all(b"runtime").unwrap();
            zip.finish().unwrap();
        }
        let asset = Asset {
            file: "rt.dll",
            url: "https://example.invalid",
            size: 1,
            sha256: "",
            unpack: Unpack::Zip("onnxruntime.dll"),
        };
        let out = dir.path().join("rt.dll");
        unpack(&asset, &part, &out).unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"runtime");
    }
}
