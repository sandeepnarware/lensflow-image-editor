//! The web's `Renderer`: draws a photo with edit settings. Work is cached by what it depends
//! on, as the web does: the wide blurs once, the detail blurs again only when sharpening or
//! texture or the spots change, the spots only when they change, the curve texture only when
//! the curves change.

use std::collections::HashMap;

use crate::curve_texture::CurveTexture;
use crate::error::PipelineError;
use crate::gpu::Gpu;
use crate::passes::develop::{self, Frame, Inputs, Uniforms};
use crate::passes::{copy, gaussian, spots};
use crate::texture::{Source, Target, read_back};
use image::RgbaImage;
use lf_settings::geometry::{Geometry, output_size, output_to_source, turned_size};
use lf_settings::{Crop, EditSettings, Rotate};

/// Long edge of the small copy used for the wide blurs (fixed, so the preview and the export
/// blur alike).
pub const SMALL_EDGE: f64 = 384.0;

/// The spots' JSON (the web's `spotsKey`, `JSON.stringify(s.spots)`; empty when none).
type SpotsKey = String;

/// What the detail blurs were last made from (the web's `detailKey`): the spots' key and the
/// sigmas, so the blurs are rebuilt from the spotted image when the spots change.
type DetailKey = (SpotsKey, Option<f64>, Option<f64>);

/// The part of the output a draw shows, as fractions of the output (zoom and pan).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Default for View {
    fn default() -> View {
        View {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        }
    }
}

/// The web's `DrawOptions`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DrawOptions {
    /// The photo as it was: no geometry, no development.
    pub before: bool,
    /// Draws this crop instead of the settings' (e.g. the whole turned frame while cropping).
    pub crop: Option<Crop>,
    pub view: View,
}

/// 256 bins per channel of the edited output, luma `round(0.2126 R + 0.7152 G + 0.0722 B)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Histogram {
    pub r: [u32; 256],
    pub g: [u32; 256],
    pub b: [u32; 256],
    pub l: [u32; 256],
}

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
    spots_key: SpotsKey,
    detail_key: Option<DetailKey>,
}

impl<'g> Renderer<'g> {
    /// Uploads `image` (the working copy of a `full_width`×`full_height` photo) with mips and
    /// makes the wide blurs from a small copy, as the web's constructor. A zero width or height
    /// (of the image or of the full size) is [`PipelineError::EmptyImage`].
    pub fn new(
        gpu: &'g Gpu,
        image: &RgbaImage,
        full_width: u32,
        full_height: u32,
    ) -> Result<Renderer<'g>, PipelineError> {
        let (width, height) = image.dimensions();
        if width == 0 || height == 0 || full_width == 0 || full_height == 0 {
            return Err(PipelineError::EmptyImage);
        }
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
            spots_key: String::new(),
            detail_key: None,
        })
    }

    /// Full-resolution output size for these settings.
    pub fn output_size(&self, s: &EditSettings) -> (u32, u32) {
        output_size(self.full_width, self.full_height, &Geometry::from(s))
    }

    /// The spots, the detail blurs (each cached) and the curve texture for `s`; returns the
    /// image to develop from: the spotted target when `s` has spots, else the source.
    pub fn prepare(&mut self, s: &EditSettings) -> Result<&Target, PipelineError> {
        let spotted = self.prepare_caches(s)?;
        Ok(self.image(spotted))
    }

    /// `prepare` without the borrow: true when the photo is developed from the spotted target.
    fn prepare_caches(&mut self, s: &EditSettings) -> Result<bool, PipelineError> {
        let (w, h) = (self.width, self.height);
        let spots_key = if s.spots.is_empty() {
            String::new()
        } else {
            serde_json::to_string(&s.spots).expect("spots serialise")
        };
        if !s.spots.is_empty() {
            self.cached("spots", w, h, true)?;
            if spots_key != self.spots_key {
                let target = &self.targets["spots"];
                spots::run(self.gpu, &self.source, target, &s.spots, w, h);
                self.spots_key = spots_key.clone();
                self.detail_key = None;
            }
        } else if !self.spots_key.is_empty() {
            self.spots_key.clear();
            self.detail_key = None;
        }
        let spotted = !s.spots.is_empty();

        // Blur radii in full-size pixels, scaled to the working size.
        let sharp_sigma = (s.sharpen_radius * self.scale).max(0.35);
        let texture_sigma = (4.0 * self.scale).max(0.6);
        let key = (
            spots_key,
            (s.sharpen > 0.0).then_some(sharp_sigma),
            (s.texture != 0.0).then_some(texture_sigma),
        );
        if self.detail_key.as_ref() != Some(&key) {
            self.cached("tmp", w, h, false)?;
            if s.sharpen > 0.0 {
                self.cached("sharp", w, h, true)?;
            }
            if s.texture != 0.0 {
                self.cached("texture", w, h, true)?;
            }
            let image = self.image(spotted);
            let tmp = &self.targets["tmp"];
            if s.sharpen > 0.0 {
                let to = &self.targets["sharp"];
                gaussian::blur(self.gpu, image, to, tmp, sharp_sigma as f32);
            }
            if s.texture != 0.0 {
                let to = &self.targets["texture"];
                gaussian::blur(self.gpu, image, to, tmp, texture_sigma as f32);
            }
            self.detail_key = Some(key);
        }
        self.curve.update(self.gpu, &s.curve);
        Ok(spotted)
    }

    /// The image to develop from: the spotted target, or the source.
    fn image(&self, spotted: bool) -> &Target {
        if spotted {
            &self.targets["spots"]
        } else {
            &self.source
        }
    }

    /// The edited photo as saving renders it (`renderJpeg` up to the JPEG step): the output
    /// size scaled to the working size, the whole output, row 0 = the top of the picture.
    pub fn render(&mut self, s: &EditSettings) -> Result<RgbaImage, PipelineError> {
        let spotted = self.prepare_caches(s)?;
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
            image: self.image(spotted),
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

    /// Draws into `canvas` (sized by the caller): the photo with `s`, or as it was (`before`),
    /// or with the crop override, showing `opts.view`. Targets are top-down, so no Y flip.
    pub fn draw(
        &mut self,
        s: &EditSettings,
        opts: &DrawOptions,
        canvas: &Target,
    ) -> Result<(), PipelineError> {
        let (geometry, crop) = if opts.before {
            let whole = Crop {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            };
            let g = Geometry {
                rotate: Rotate::R0,
                flip_h: false,
                flip_v: false,
                angle: 0.0,
                crop: whole,
            };
            (g, whole)
        } else {
            (Geometry::from(s), opts.crop.unwrap_or(s.crop))
        };
        let spotted = if opts.before {
            false
        } else {
            self.prepare_caches(s)?
        };
        let matrix = output_to_source(self.full_width, self.full_height, &geometry, &crop);
        let (tw, th) = turned_size(self.full_width, self.full_height, geometry.rotate);
        let frame = Frame {
            matrix,
            out: [crop.w * f64::from(tw), crop.h * f64::from(th)],
            view: [opts.view.x, opts.view.y, opts.view.w, opts.view.h],
            develop: !opts.before,
            flip: false,
            img_size: [self.width, self.height],
            scale: self.scale,
            target: [canvas.width, canvas.height],
        };
        let (sharp, texture) = if opts.before {
            (None, None)
        } else {
            (self.targets.get("sharp"), self.targets.get("texture"))
        };
        let uniforms = Uniforms::new(s, &frame, sharp.is_some(), texture.is_some());
        let inputs = Inputs {
            image: self.image(spotted),
            blur_m: &self.blur_m,
            blur_l: &self.blur_l,
            blur_s: sharp.unwrap_or(&self.blur_m),
            blur_t: texture.unwrap_or(&self.blur_m),
            curve: &self.curve.target,
        };
        develop::run(self.gpu, &inputs, &uniforms, canvas);
        Ok(())
    }

    /// The whole output with `s` at `edge` long edge (the web's `pixels`), top row first.
    pub fn pixels(&mut self, s: &EditSettings, edge: u32) -> Result<RgbaImage, PipelineError> {
        let spotted = self.prepare_caches(s)?;
        let geometry = Geometry::from(s);
        let (ow, oh) = output_size(self.full_width, self.full_height, &geometry);
        let k = f64::from(edge) / f64::from(ow.max(oh));
        let w = ((f64::from(ow) * k).round() as u32).max(1);
        let h = ((f64::from(oh) * k).round() as u32).max(1);
        self.cached("thumb", w, h, false)?;
        let (tw, th) = turned_size(self.full_width, self.full_height, s.rotate);
        let frame = Frame {
            matrix: output_to_source(self.full_width, self.full_height, &geometry, &s.crop),
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
            image: self.image(spotted),
            blur_m: &self.blur_m,
            blur_l: &self.blur_l,
            blur_s: sharp.unwrap_or(&self.blur_m),
            blur_t: texture.unwrap_or(&self.blur_m),
            curve: &self.curve.target,
        };
        let out = &self.targets["thumb"];
        develop::run(self.gpu, &inputs, &uniforms, out);
        read_back(self.gpu, out)
    }

    /// Brightness histogram of the edited output, from a 200 px render.
    pub fn histogram(&mut self, s: &EditSettings) -> Result<Histogram, PipelineError> {
        let img = self.pixels(s, 200)?;
        let mut h = Histogram {
            r: [0; 256],
            g: [0; 256],
            b: [0; 256],
            l: [0; 256],
        };
        for p in img.pixels() {
            let [r, g, b, _] = p.0;
            h.r[r as usize] += 1;
            h.g[g as usize] += 1;
            h.b[b as usize] += 1;
            let l = (0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)).round();
            h.l[l as usize] += 1;
        }
        Ok(h)
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
