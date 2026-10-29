//! Local AI for the desktop editor: subject and sky masks, denoising, upscaling, object
//! removal, face finding, recognising people and culling scores, on this computer's CPU through ONNX Runtime. Nothing is sent
//! anywhere: the runtime and the models are downloaded once into the AI folder
//! ([`assets`], each pinned by SHA-256) and run from there.
//!
//! Pixels come in and go out as [`Rgb`] (f32 RGB, 0..1, display-referred sRGB: what the
//! photo looks like with its edits). Large photos are worked in overlapping tiles
//! ([`tiles`]) so memory stays bounded.

pub mod assets;
pub mod cull;
pub mod faces;
pub mod identity;
pub mod inpaint;
pub mod keywords;
pub mod objects;
pub mod restore;
pub mod segment;
pub mod tiles;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

use assets::Asset;

/// Why an AI tool could not run: a sentence to show.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AiError {
    #[error("The download failed ({0}). Check the internet connection and try again.")]
    Download(String),
    #[error("{0} did not download correctly, so it was deleted. Try again.")]
    Mismatch(&'static str),
    #[error("Could not write {0}.")]
    Io(String),
    #[error("The AI engine could not start ({0}).")]
    Runtime(String),
    #[error("The AI model failed ({0}).")]
    Model(String),
    #[error("Stopped.")]
    Stopped,
    #[error("The AI tools are not available on this kind of computer yet.")]
    Unsupported,
}

/// An RGB picture: `w × h` pixels, three f32 each (0..1), top row first.
#[derive(Debug, Clone, PartialEq)]
pub struct Rgb {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Rgb {
    pub fn new(w: usize, h: usize) -> Rgb {
        Rgb {
            w,
            h,
            data: vec![0.0; w * h * 3],
        }
    }

    /// From 8-bit RGBA (alpha dropped).
    pub fn from_rgba8(img: &image::RgbaImage) -> Rgb {
        Rgb {
            w: img.width() as usize,
            h: img.height() as usize,
            data: img
                .pixels()
                .flat_map(|p| [p.0[0], p.0[1], p.0[2]].map(|v| f32::from(v) / 255.0))
                .collect(),
        }
    }

    pub fn to_rgba8(&self) -> image::RgbaImage {
        let mut out = image::RgbaImage::new(self.w as u32, self.h as u32);
        for (px, c) in out.pixels_mut().zip(self.data.as_chunks::<3>().0) {
            let v = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
            *px = image::Rgba([v(c[0]), v(c[1]), v(c[2]), 255]);
        }
        out
    }

    pub fn at(&self, x: usize, y: usize) -> [f32; 3] {
        let i = (y * self.w + x) * 3;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    /// The part at `(x, y)` sized `w × h` (inside the picture).
    pub fn crop(&self, x: usize, y: usize, w: usize, h: usize) -> Rgb {
        let mut out = Rgb::new(w, h);
        for row in 0..h {
            let from = ((y + row) * self.w + x) * 3;
            out.data[row * w * 3..(row + 1) * w * 3]
                .copy_from_slice(&self.data[from..from + w * 3]);
        }
        out
    }

    /// Resized to `w × h` (bilinear for upsizing, area averaging for downsizing, as
    /// `image`'s triangle filter does).
    pub fn resized(&self, w: usize, h: usize) -> Rgb {
        if (w, h) == (self.w, self.h) {
            return self.clone();
        }
        let buf: image::ImageBuffer<image::Rgb<f32>, Vec<f32>> =
            image::ImageBuffer::from_raw(self.w as u32, self.h as u32, self.data.clone())
                .expect("sized");
        let out = image::imageops::resize(
            &buf,
            w as u32,
            h as u32,
            image::imageops::FilterType::Triangle,
        );
        Rgb {
            w,
            h,
            data: out.into_raw(),
        }
    }

    /// Planar NCHW floats, as the models take them.
    pub fn planar(&self) -> Vec<f32> {
        let n = self.w * self.h;
        let mut out = vec![0.0; n * 3];
        for (i, c) in self.data.as_chunks::<3>().0.iter().enumerate() {
            out[i] = c[0];
            out[n + i] = c[1];
            out[2 * n + i] = c[2];
        }
        out
    }

    /// From planar NCHW floats.
    pub fn from_planar(w: usize, h: usize, planar: &[f32]) -> Rgb {
        let n = w * h;
        let mut data = vec![0.0; n * 3];
        for i in 0..n {
            data[i * 3] = planar[i];
            data[i * 3 + 1] = planar[n + i];
            data[i * 3 + 2] = planar[2 * n + i];
        }
        Rgb { w, h, data }
    }
}

/// A single-channel picture (a mask: 0..1).
#[derive(Debug, Clone, PartialEq)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Gray {
    pub fn to_luma8(&self) -> image::GrayImage {
        image::GrayImage::from_raw(
            self.w as u32,
            self.h as u32,
            self.data
                .iter()
                .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
                .collect(),
        )
        .expect("sized")
    }

    pub fn from_luma8(img: &image::GrayImage) -> Gray {
        Gray {
            w: img.width() as usize,
            h: img.height() as usize,
            data: img.as_raw().iter().map(|&v| f32::from(v) / 255.0).collect(),
        }
    }

    /// Resized to `w × h` (triangle filter).
    pub fn resized(&self, w: usize, h: usize) -> Gray {
        if (w, h) == (self.w, self.h) {
            return self.clone();
        }
        let buf: image::ImageBuffer<image::Luma<f32>, Vec<f32>> =
            image::ImageBuffer::from_raw(self.w as u32, self.h as u32, self.data.clone())
                .expect("sized");
        let out = image::imageops::resize(
            &buf,
            w as u32,
            h as u32,
            image::imageops::FilterType::Triangle,
        );
        Gray {
            w,
            h,
            data: out.into_raw(),
        }
    }
}

/// The ONNX Runtime environment, made once per process from the downloaded library.
static RUNTIME: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();

/// The AI folder and what runs from it.
#[derive(Debug, Clone)]
pub struct Engine {
    dir: PathBuf,
}

impl Engine {
    /// Files live in `dir` (the app's cache folder `ai/`).
    pub fn new(dir: impl Into<PathBuf>) -> Engine {
        Engine { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// What a tool needing `models` still has to download, in bytes (the runtime included).
    pub fn to_download(&self, models: &[Asset]) -> u64 {
        let mut all: Vec<Asset> = models.to_vec();
        all.extend(assets::runtime());
        assets::missing_bytes(&self.dir, &all)
    }

    /// Fetches the runtime and `models` if needed (`progress(done, total)` in bytes over all
    /// of them) and starts the runtime.
    pub fn prepare(
        &self,
        models: &[Asset],
        progress: &mut dyn FnMut(u64, u64),
        stop: &AtomicBool,
    ) -> Result<(), AiError> {
        let runtime = assets::runtime().ok_or(AiError::Unsupported)?;
        let mut all = vec![runtime];
        all.extend_from_slice(models);
        let total = assets::missing_bytes(&self.dir, &all);
        let mut before = 0u64;
        for asset in &all {
            if assets::present(&self.dir, asset) {
                continue;
            }
            assets::ensure(
                &self.dir,
                asset,
                &mut |done, _| progress(before + done, total),
                stop,
            )?;
            before += asset.size;
        }
        self.start(&runtime)
    }

    fn start(&self, runtime: &Asset) -> Result<(), AiError> {
        let lib = assets::path(&self.dir, runtime);
        RUNTIME
            .get_or_init(|| {
                log::info!("starting ONNX Runtime from {}", lib.display());
                ort::init_from(&lib)
                    .map_err(|e| e.to_string())?
                    .with_name("lensflow")
                    .build()
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map(|_| ())
            .map_err(|e| AiError::Runtime(e.clone()))
    }

    /// A session for `model` (fetched and started by [`Engine::prepare`] first).
    pub fn session(&self, model: &Asset) -> Result<ort::session::Session, AiError> {
        let env = match RUNTIME.get() {
            Some(Ok(env)) => env,
            Some(Err(e)) => return Err(AiError::Runtime(e.clone())),
            None => return Err(AiError::Runtime("not started".into())),
        };
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        ort::session::Session::builder(env)
            .and_then(|b| b.with_intra_threads(threads).map_err(Into::into))
            .and_then(|mut b| b.commit_from_file(assets::path(&self.dir, model)))
            .map_err(|e| AiError::Model(e.to_string()))
    }
}

/// Runs `session` on one NCHW f32 tensor named `input` and returns the first output's shape
/// and values.
pub(crate) fn run_one(
    session: &mut ort::session::Session,
    input: &str,
    shape: [usize; 4],
    data: Vec<f32>,
) -> Result<(Vec<usize>, Vec<f32>), AiError> {
    let model = |e: ort::Error| AiError::Model(e.to_string());
    let tensor = ort::value::Tensor::from_array((shape, data)).map_err(model)?;
    let outputs = session.run(ort::inputs![input => tensor]).map_err(model)?;
    let (shape, values) = outputs[0].try_extract_tensor::<f32>().map_err(model)?;
    Ok((
        shape.iter().map(|&d| d.max(0) as usize).collect(),
        values.to_vec(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_round_trips() {
        let mut img = Rgb::new(3, 2);
        for (i, v) in img.data.iter_mut().enumerate() {
            *v = i as f32;
        }
        let planar = img.planar();
        assert_eq!(&planar[..6], &[0.0, 3.0, 6.0, 9.0, 12.0, 15.0]);
        assert_eq!(Rgb::from_planar(3, 2, &planar), img);
        let part = img.crop(1, 1, 2, 1);
        assert_eq!(part.data, vec![12.0, 13.0, 14.0, 15.0, 16.0, 17.0]);
    }
}
