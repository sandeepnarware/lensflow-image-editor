//! The web's `Renderer`: draws a photo with edit settings. Work is cached by what it depends
//! on, as the web does: the wide blurs once, the detail blurs again only when sharpening or
//! texture change, the curve texture only when the curves change.

use std::collections::HashMap;

use crate::curve_texture::CurveTexture;
use crate::error::PipelineError;
use crate::gpu::Gpu;
use crate::passes::develop::{self, Frame, Inputs, Uniforms};
use crate::passes::{copy, gaussian};
use crate::texture::{Source, Target, read_back};
use image::RgbaImage;
use lf_settings::EditSettings;
use lf_settings::geometry::{Geometry, output_size, output_to_source, turned_size};

/// Long edge of the small copy used for the wide blurs (fixed, so the preview and the export
/// blur alike).
pub const SMALL_EDGE: f64 = 384.0;

/// The sigmas the detail blurs were last made with (the web's `detailKey`).
type DetailKey = (Option<f64>, Option<f64>);

pub struct Renderer<'g> {
    gpu: &'g Gpu,
    /// Working size of the photo (≤ the file's size).
    pub width: u32,
    pub height: u32,
    pub full_width: u32,
    pub full_height: u32,
    /// Working size ÷ the file's full size.
    pub scale: f64,
    source: Target,
    curve: CurveTexture,
    blur_m: Target,
    blur_l: Target,
    /// Kept, as the web keeps them, though only the constructor draws into them.
    _small: Target,
    _tmp_small: Target,
    targets: HashMap<&'static str, Target>,
    detail_key: Option<DetailKey>,
}

impl<'g> Renderer<'g> {
    /// Uploads `image` (the working copy of a `full_width`×`full_height` photo) with mips and
    /// makes the wide blurs from a small copy, as the web's constructor.
    pub fn new(
        gpu: &'g Gpu,
        image: &RgbaImage,
        full_width: u32,
        full_height: u32,
    ) -> Result<Renderer<'g>, PipelineError> {
        let (width, height) = image.dimensions();
        let source = Source::upload(gpu, image, true)?;
        let curve = CurveTexture::new(gpu)?;

        let k = SMALL_EDGE / f64::from(width.max(height));
        let small_size = |v: u32| ((f64::from(v) * k.min(1.0)).round() as u32).max(1);
        let (sw, sh) = (small_size(width), small_size(height));
        let small = Target::new(gpu, sw, sh, false)?;
        let tmp_small = Target::new(gpu, sw, sh, false)?;
        let blur_m = Target::new(gpu, sw, sh, false)?;
        let blur_l = Target::new(gpu, sw, sh, false)?;
        copy::run(gpu, &source, &small);
        // Sigmas as fractions of the long edge (1.5% and 4%).
        let long = f64::from(sw.max(sh));
        gaussian::blur(gpu, &small, &blur_m, &tmp_small, (long * 0.015) as f32);
        gaussian::blur(gpu, &small, &blur_l, &tmp_small, (long * 0.04) as f32);

        Ok(Renderer {
            gpu,
            width,
            height,
            full_width,
            full_height,
            scale: f64::from(width) / f64::from(full_width),
            source,
            curve,
            blur_m,
            blur_l,
            _small: small,
            _tmp_small: tmp_small,
            targets: HashMap::new(),
            detail_key: None,
        })
    }

    /// Full-resolution output size for these settings.
    pub fn output_size(&self, s: &EditSettings) -> (u32, u32) {
        output_size(self.full_width, self.full_height, &Geometry::from(s))
    }

    /// The detail blurs (cached) and the curve texture for `s`. Spots are not applied yet:
    /// the photo is developed from the source.
    pub fn prepare(&mut self, s: &EditSettings) -> Result<(), PipelineError> {
        // Blur radii in full-size pixels, scaled to the working size.
        let sharp_sigma = (s.sharpen_radius * self.scale).max(0.35);
        let texture_sigma = (4.0 * self.scale).max(0.6);
        let key = (
            (s.sharpen > 0.0).then_some(sharp_sigma),
            (s.texture != 0.0).then_some(texture_sigma),
        );
        if self.detail_key != Some(key) {
            let (w, h) = (self.width, self.height);
            self.cached("tmp", w, h, false)?;
            if s.sharpen > 0.0 {
                self.cached("sharp", w, h, true)?;
            }
            if s.texture != 0.0 {
                self.cached("texture", w, h, true)?;
            }
            let tmp = &self.targets["tmp"];
            if s.sharpen > 0.0 {
                let to = &self.targets["sharp"];
                gaussian::blur(self.gpu, &self.source, to, tmp, sharp_sigma as f32);
            }
            if s.texture != 0.0 {
                let to = &self.targets["texture"];
                gaussian::blur(self.gpu, &self.source, to, tmp, texture_sigma as f32);
            }
            self.detail_key = Some(key);
        }
        self.curve.update(self.gpu, &s.curve);
        Ok(())
    }

    /// The edited photo as saving renders it (`renderJpeg` up to the JPEG step): the output
    /// size scaled to the working size, the whole output, row 0 = the top of the picture.
    pub fn render(&mut self, s: &EditSettings) -> Result<RgbaImage, PipelineError> {
        self.prepare(s)?;
        let geometry = Geometry::from(s);
        let (ow, oh) = output_size(self.full_width, self.full_height, &geometry);
        let k = f64::from(self.width) / f64::from(self.full_width);
        let w = ((f64::from(ow) * k).round() as u32).max(1);
        let h = ((f64::from(oh) * k).round() as u32).max(1);
        self.cached("output", w, h, false)?;

        let matrix = output_to_source(self.full_width, self.full_height, &geometry, &s.crop);
        let (tw, th) = turned_size(self.full_width, self.full_height, s.rotate);
        let frame = Frame {
            matrix,
            out: [s.crop.w * f64::from(tw), s.crop.h * f64::from(th)],
            view: [0.0, 0.0, 1.0, 1.0],
            develop: true,
            flip: false,
            img_size: [self.width, self.height],
            scale: self.scale,
            target: [w, h],
        };
        let sharp = self.targets.get("sharp");
        let texture = self.targets.get("texture");
        let uniforms = Uniforms::new(s, &frame, sharp.is_some(), texture.is_some());
        let inputs = Inputs {
            image: &self.source,
            blur_m: &self.blur_m,
            blur_l: &self.blur_l,
            blur_s: sharp.unwrap_or(&self.blur_m),
            blur_t: texture.unwrap_or(&self.blur_m),
            curve: &self.curve.target,
        };
        let out = &self.targets["output"];
        develop::run(self.gpu, &inputs, &uniforms, out);
        read_back(self.gpu, out)
    }

    /// The web's `cached()`: the target named `name`, remade when its size changes.
    fn cached(
        &mut self,
        name: &'static str,
        w: u32,
        h: u32,
        mips: bool,
    ) -> Result<&Target, PipelineError> {
        let fresh = !matches!(self.targets.get(name), Some(t) if t.width == w && t.height == h);
        if fresh {
            if let Some(old) = self.targets.remove(name) {
                old.texture.destroy();
            }
            self.targets
                .insert(name, Target::new(self.gpu, w, h, mips)?);
        }
        Ok(&self.targets[name])
    }
}
