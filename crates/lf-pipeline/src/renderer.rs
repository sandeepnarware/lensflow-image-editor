//! The web's `Renderer`: draws a photo with edit settings. Work is cached by what it depends
//! on, as the web does: the wide blurs once, the detail blurs again only when sharpening or
//! texture or the spots change, the spots only when they change, the curve texture only when
//! the curves change.

use std::collections::HashMap;
use std::sync::Arc;

use crate::curve_texture::CurveTexture;
use crate::error::PipelineError;
use crate::gpu::Gpu;
use crate::passes::develop::{self, Frame, Inputs, Uniforms};
use crate::passes::{UniformArena, copy, gaussian, spots};
use crate::texture::{FLOAT_FORMAT, FORMAT, FloatFrame, Source, Target, read_back, read_back_f16};
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

/// What a develop pass draws: the photo as it was, or developed (from the spotted image when
/// `spotted`).
#[derive(Clone, Copy)]
enum Mode {
    Before,
    Develop { spotted: bool },
}

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

/// Where a develop pass draws: a cached target by name, or the caller's canvas.
#[derive(Clone, Copy)]
enum Dest<'a> {
    Cached(&'static str),
    Canvas(&'a Target),
}

/// The cached targets [`Renderer::trim`] drops: outputs, remade on the next call.
const TRIMMED: [&str; 4] = ["output", "output_f16", "thumb", "hist"];

/// 256 bins per channel of the edited output, luma `round(0.2126 R + 0.7152 G + 0.0722 B)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Histogram {
    pub r: [u32; 256],
    pub g: [u32; 256],
    pub b: [u32; 256],
    pub l: [u32; 256],
}

/// Owns its [`Gpu`] handle, so it can move to a render thread (`Renderer: Send`).
pub struct Renderer {
    gpu: Arc<Gpu>,
    /// Working size of the photo (≤ the file's size).
    pub width: u32,
    pub height: u32,
    pub full_width: u32,
    pub full_height: u32,
    /// Working size ÷ the file's full size.
    pub scale: f64,
    source: Target,
    /// The format of the source and every intermediate: `Rgba16Float` from
    /// [`Renderer::from_texture`], `Rgba8Unorm` from [`Renderer::new`].
    chain: wgpu::TextureFormat,
    curve: CurveTexture,
    blur_m: Target,
    blur_l: Target,
    /// Kept, as the web keeps them, though only the constructor draws into them.
    _small: Target,
    _tmp_small: Target,
    targets: HashMap<&'static str, Target>,
    spots_key: SpotsKey,
    detail_key: Option<DetailKey>,
    /// The develop pass's uniforms, rewritten each draw.
    develop_uniforms: UniformArena,
    /// Test hook: provoke a validation error inside the next call's error scope.
    #[cfg(feature = "test-support")]
    inject_error: bool,
}

impl Renderer {
    /// Uploads `image` (the working copy of a `full_width`×`full_height` photo) with mips and
    /// makes the wide blurs from a small copy, as the web's constructor. An 8-bit photo has no
    /// values above 1.0, so its chain stays in `RGBA8` as the web's does: the low-radius
    /// sharpening turns each 1/255 rounding of the web's 8-bit blur into up to 4/255 of
    /// output, so float intermediates (or a half-float source, whose 11-bit mantissa cannot
    /// hold `v / 255` exactly) put it outside the parity gate. A zero width or height (of the
    /// image or of the full size) is [`PipelineError::EmptyImage`].
    pub fn new(
        gpu: Arc<Gpu>,
        image: &RgbaImage,
        full_width: u32,
        full_height: u32,
    ) -> Result<Renderer, PipelineError> {
        let (width, height) = image.dimensions();
        if width == 0 || height == 0 || full_width == 0 || full_height == 0 {
            return Err(PipelineError::EmptyImage);
        }
        let scopes = Scopes::push(&gpu);
        let made = Source::upload(&gpu, image, true)
            .and_then(|source| Renderer::with_source(gpu.clone(), source, full_width, full_height));
        scopes.pop(&gpu)?;
        made
    }

    /// Takes `source` (the working copy of a `full_width`×`full_height` photo: an
    /// `Rgba16Float` texture with mips, e.g. from [`Source::upload_f16`]) and makes the wide
    /// blurs from a small copy, as the web's constructor. Every intermediate is `Rgba16Float`
    /// too, so values above 1.0 reach the develop pass. Another format, or no mips, is
    /// [`PipelineError::WrongFormat`]; a zero full size is [`PipelineError::EmptyImage`].
    pub fn from_texture(
        gpu: Arc<Gpu>,
        source: Target,
        full_width: u32,
        full_height: u32,
    ) -> Result<Renderer, PipelineError> {
        if source.format != FLOAT_FORMAT || !source.mips {
            let mips = |m: bool| if m { "with mipmaps" } else { "without mipmaps" };
            return Err(PipelineError::WrongFormat {
                expected: format!("{FLOAT_FORMAT:?} {}", mips(true)),
                got: format!("{:?} {}", source.format, mips(source.mips)),
            });
        }
        let scopes = Scopes::push(&gpu);
        let made = Renderer::with_source(gpu.clone(), source, full_width, full_height);
        scopes.pop(&gpu)?;
        made
    }

    /// Uploads `frame` (the working copy of a `full_width`×`full_height` photo, half floats
    /// that may exceed 1.0) with mips and builds the renderer as [`Renderer::from_texture`]
    /// does, all under this call's own error scopes: a device error during the upload (or the
    /// build) is returned as [`PipelineError::Gpu`] / `OutOfMemory`, not only logged. A frame
    /// whose data does not match its size is [`PipelineError::BadFrame`]; a lost device is
    /// [`PipelineError::DeviceLost`].
    pub fn from_frame(
        gpu: Arc<Gpu>,
        frame: &FloatFrame,
        full_width: u32,
        full_height: u32,
    ) -> Result<Renderer, PipelineError> {
        gpu.check()?;
        if full_width == 0 || full_height == 0 {
            return Err(PipelineError::EmptyImage);
        }
        let scopes = Scopes::push(&gpu);
        let made = Source::upload_f16(&gpu, frame, true)
            .and_then(|source| Renderer::with_source(gpu.clone(), source, full_width, full_height));
        #[cfg(feature = "test-support")]
        if crate::test_support::take_upload_failure() {
            crate::test_support::provoke_validation_error(&gpu);
        }
        scopes.pop(&gpu)?;
        made
    }

    /// `new` and `from_texture` past their checks: the intermediates take `source`'s format.
    fn with_source(
        gpu: Arc<Gpu>,
        source: Target,
        full_width: u32,
        full_height: u32,
    ) -> Result<Renderer, PipelineError> {
        let (width, height) = (source.width, source.height);
        if full_width == 0 || full_height == 0 {
            return Err(PipelineError::EmptyImage);
        }
        let chain = source.format;
        let curve = CurveTexture::new(&gpu)?;

        let k = SMALL_EDGE / f64::from(width.max(height));
        let small_size = |v: u32| ((f64::from(v) * k.min(1.0)).round() as u32).max(1);
        let (sw, sh) = (small_size(width), small_size(height));
        let target = |w, h| Target::new_with(&gpu, w, h, false, chain);
        let small = target(sw, sh)?;
        let tmp_small = target(sw, sh)?;
        let blur_m = target(sw, sh)?;
        let blur_l = target(sw, sh)?;
        copy::run(&gpu, &source, &small);
        // Sigmas as fractions of the long edge (1.5% and 4%).
        let long = f64::from(sw.max(sh));
        gaussian::blur(&gpu, &small, &blur_m, &tmp_small, (long * 0.015) as f32);
        gaussian::blur(&gpu, &small, &blur_l, &tmp_small, (long * 0.04) as f32);
        let develop_uniforms = develop::arena(&gpu);

        Ok(Renderer {
            gpu,
            width,
            height,
            full_width,
            full_height,
            scale: f64::from(width) / f64::from(full_width),
            source,
            chain,
            curve,
            blur_m,
            blur_l,
            _small: small,
            _tmp_small: tmp_small,
            targets: HashMap::new(),
            spots_key: String::new(),
            detail_key: None,
            develop_uniforms,
            #[cfg(feature = "test-support")]
            inject_error: false,
        })
    }

    /// The device this renderer draws on.
    pub fn gpu(&self) -> &Arc<Gpu> {
        &self.gpu
    }

    /// The device was lost: every draw now fails with [`PipelineError::DeviceLost`].
    pub fn is_lost(&self) -> bool {
        self.gpu.is_lost()
    }

    /// Drops the output-sized cached targets (`render`'s, `pixels`'s and `histogram`'s) to
    /// free memory, e.g. when the photo leaves the screen; they are remade on the next call.
    pub fn trim(&mut self) {
        for name in TRIMMED {
            if let Some(t) = self.targets.remove(name) {
                t.texture.destroy();
            }
        }
    }

    /// Whether the cached target `name` exists (for tests of [`Renderer::trim`]).
    #[doc(hidden)]
    pub fn has_cached(&self, name: &str) -> bool {
        self.targets.contains_key(name)
    }

    /// Full-resolution output size for these settings.
    pub fn output_size(&self, s: &EditSettings) -> (u32, u32) {
        output_size(self.full_width, self.full_height, &Geometry::from(s))
    }

    /// The spots, the detail blurs (each cached) and the curve texture for `s`; returns the
    /// image to develop from: the spotted target when `s` has spots, else the source.
    /// Runs under the call's own error scopes, as `draw` does.
    pub fn prepare(&mut self, s: &EditSettings) -> Result<&Target, PipelineError> {
        let spotted = self.scoped(|r| r.prepare_caches(s))?;
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
            self.cached("spots", w, h, true, self.chain)?;
            if spots_key != self.spots_key {
                let target = &self.targets["spots"];
                spots::run(&self.gpu, &self.source, target, &s.spots, w, h);
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
            self.cached("tmp", w, h, false, self.chain)?;
            if s.sharpen > 0.0 {
                self.cached("sharp", w, h, true, self.chain)?;
            }
            if s.texture != 0.0 {
                self.cached("texture", w, h, true, self.chain)?;
            }
            let image = self.image(spotted);
            let tmp = &self.targets["tmp"];
            if s.sharpen > 0.0 {
                let to = &self.targets["sharp"];
                gaussian::blur(&self.gpu, image, to, tmp, sharp_sigma as f32);
            }
            if s.texture != 0.0 {
                let to = &self.targets["texture"];
                gaussian::blur(&self.gpu, image, to, tmp, texture_sigma as f32);
            }
            self.detail_key = Some(key);
        }
        self.curve.update(&self.gpu, &s.curve);
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
    /// A lost device is [`PipelineError::DeviceLost`]; an error the device reports during the
    /// call is [`PipelineError::Gpu`] (as for `draw`, `pixels` and `histogram`).
    pub fn render(&mut self, s: &EditSettings) -> Result<RgbaImage, PipelineError> {
        self.scoped(|r| r.render_inner(s))
    }

    fn render_inner(&mut self, s: &EditSettings) -> Result<RgbaImage, PipelineError> {
        let spotted = self.prepare_caches(s)?;
        let geometry = Geometry::from(s);
        let (ow, oh) = output_size(self.full_width, self.full_height, &geometry);
        let k = f64::from(self.width) / f64::from(self.full_width);
        let w = ((f64::from(ow) * k).round() as u32).max(1);
        let h = ((f64::from(oh) * k).round() as u32).max(1);
        self.cached("output", w, h, false, FORMAT)?;
        let mode = Mode::Develop { spotted };
        let dest = Dest::Cached("output");
        self.develop_into(s, &geometry, &s.crop, &View::default(), mode, dest);
        read_back(&self.gpu, &self.targets["output"])
    }

    /// [`Renderer::render`] into an `Rgba16Float` target and read back as half floats, for a
    /// 16-bit export: the develop shader's final clamp to 0..1 still applies, but the value is
    /// not quantised to 8 bits. Row 0 = the top of the picture.
    pub fn render_f16(&mut self, s: &EditSettings) -> Result<FloatFrame, PipelineError> {
        self.scoped(|r| r.render_f16_inner(s))
    }

    fn render_f16_inner(&mut self, s: &EditSettings) -> Result<FloatFrame, PipelineError> {
        let spotted = self.prepare_caches(s)?;
        let geometry = Geometry::from(s);
        let (ow, oh) = output_size(self.full_width, self.full_height, &geometry);
        let k = f64::from(self.width) / f64::from(self.full_width);
        let w = ((f64::from(ow) * k).round() as u32).max(1);
        let h = ((f64::from(oh) * k).round() as u32).max(1);
        self.cached("output_f16", w, h, false, FLOAT_FORMAT)?;
        let mode = Mode::Develop { spotted };
        let dest = Dest::Cached("output_f16");
        self.develop_into(s, &geometry, &s.crop, &View::default(), mode, dest);
        read_back_f16(&self.gpu, &self.targets["output_f16"])
    }

    /// Runs `f` under this thread's own validation and out-of-memory error scopes, so the
    /// call reports exactly the device errors it caused, even when other threads (or the UI)
    /// share the device. A lost device is [`PipelineError::DeviceLost`]; a captured error is
    /// [`PipelineError::Gpu`] or [`PipelineError::OutOfMemory`] (taking precedence over
    /// `f`'s own result), logged, and every cache is dropped so the next call starts fresh.
    fn scoped<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, PipelineError>,
    ) -> Result<T, PipelineError> {
        if self.gpu.is_lost() {
            return Err(PipelineError::DeviceLost);
        }
        let scopes = Scopes::push(&self.gpu);
        let result = f(self);
        #[cfg(feature = "test-support")]
        if std::mem::take(&mut self.inject_error) {
            crate::test_support::provoke_validation_error(&self.gpu);
        }
        match scopes.pop(&self.gpu) {
            Ok(()) => result,
            Err(e) => {
                self.reset_caches();
                Err(e)
            }
        }
    }

    /// Forget every cache (after a device error nothing in them can be trusted).
    fn reset_caches(&mut self) {
        for (_, t) in self.targets.drain() {
            t.texture.destroy();
        }
        self.spots_key.clear();
        self.detail_key = None;
        self.curve.invalidate();
    }

    /// Test hook: the next call provokes a validation error inside its own error scope.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_validation_error(&mut self) {
        self.inject_error = true;
    }

    /// Draws into `canvas` (sized by the caller, in any colour format the device renders to:
    /// `Rgba8Unorm`, `Bgra8Unorm`, `Rgba16Float`): the photo with `s`, or as it was (`before`),
    /// or with the crop override, showing `opts.view`. Targets are top-down, so no Y flip.
    pub fn draw(
        &mut self,
        s: &EditSettings,
        opts: &DrawOptions,
        canvas: &Target,
    ) -> Result<(), PipelineError> {
        self.scoped(|r| r.draw_inner(s, opts, canvas))
    }

    fn draw_inner(
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
        let mode = if opts.before {
            Mode::Before
        } else {
            Mode::Develop {
                spotted: self.prepare_caches(s)?,
            }
        };
        self.develop_into(s, &geometry, &crop, &opts.view, mode, Dest::Canvas(canvas));
        Ok(())
    }

    /// The whole output with `s` at `edge` long edge (the web's `pixels`), top row first.
    pub fn pixels(&mut self, s: &EditSettings, edge: u32) -> Result<RgbaImage, PipelineError> {
        self.scoped(|r| r.pixels_into("thumb", s, edge))
    }

    /// `pixels` into the cached target `name`.
    fn pixels_into(
        &mut self,
        name: &'static str,
        s: &EditSettings,
        edge: u32,
    ) -> Result<RgbaImage, PipelineError> {
        let spotted = self.prepare_caches(s)?;
        let geometry = Geometry::from(s);
        let (ow, oh) = output_size(self.full_width, self.full_height, &geometry);
        let k = f64::from(edge) / f64::from(ow.max(oh));
        let w = ((f64::from(ow) * k).round() as u32).max(1);
        let h = ((f64::from(oh) * k).round() as u32).max(1);
        self.cached(name, w, h, false, FORMAT)?;
        let mode = Mode::Develop { spotted };
        let dest = Dest::Cached(name);
        self.develop_into(s, &geometry, &s.crop, &View::default(), mode, dest);
        read_back(&self.gpu, &self.targets[name])
    }

    /// The develop pass for `s` into `target` (the caches already prepared): the output of
    /// `geometry` cropped to `crop`, showing `view`. Shared by `render`, `draw` and `pixels`.
    fn develop_into(
        &mut self,
        s: &EditSettings,
        geometry: &Geometry,
        crop: &Crop,
        view: &View,
        mode: Mode,
        dest: Dest,
    ) {
        let target = match dest {
            Dest::Cached(name) => &self.targets[name],
            Dest::Canvas(t) => t,
        };
        let (spotted, develop) = match mode {
            Mode::Before => (false, false),
            Mode::Develop { spotted } => (spotted, true),
        };
        let (tw, th) = turned_size(self.full_width, self.full_height, geometry.rotate);
        let frame = Frame {
            matrix: output_to_source(self.full_width, self.full_height, geometry, crop),
            out: [crop.w * f64::from(tw), crop.h * f64::from(th)],
            view: [view.x, view.y, view.w, view.h],
            develop,
            flip: false,
            img_size: [self.width, self.height],
            scale: self.scale,
            target: [target.width, target.height],
        };
        let (sharp, texture) = if develop {
            (self.targets.get("sharp"), self.targets.get("texture"))
        } else {
            (None, None)
        };
        let uniforms = Uniforms::new(s, &frame, sharp.is_some(), texture.is_some());
        let inputs = Inputs {
            // `self.image(spotted)`, by field so the arena below can be borrowed mutably.
            image: if spotted {
                &self.targets["spots"]
            } else {
                &self.source
            },
            blur_m: &self.blur_m,
            blur_l: &self.blur_l,
            blur_s: sharp.unwrap_or(&self.blur_m),
            blur_t: texture.unwrap_or(&self.blur_m),
            curve: &self.curve.target,
        };
        develop::run(
            &self.gpu,
            &inputs,
            &uniforms,
            &mut self.develop_uniforms,
            target,
        );
    }

    /// Brightness histogram of the edited output, from a 200 px render (in its own target,
    /// so it does not evict `pixels`'s thumbnail).
    pub fn histogram(&mut self, s: &EditSettings) -> Result<Histogram, PipelineError> {
        let img = self.scoped(|r| r.pixels_into("hist", s, 200))?;
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

    /// The web's `cached()`: the target named `name` (in `format`), remade when its size
    /// changes.
    fn cached(
        &mut self,
        name: &'static str,
        w: u32,
        h: u32,
        mips: bool,
        format: wgpu::TextureFormat,
    ) -> Result<&Target, PipelineError> {
        let fresh = !matches!(self.targets.get(name), Some(t) if t.width == w && t.height == h);
        if fresh {
            if let Some(old) = self.targets.remove(name) {
                old.texture.destroy();
            }
            self.targets
                .insert(name, Target::new_with(&self.gpu, w, h, mips, format)?);
        }
        Ok(&self.targets[name])
    }
}

/// A validation and an out-of-memory error scope pushed on this thread (wgpu's error scopes
/// are per thread), popped together.
struct Scopes {
    oom: wgpu::ErrorScopeGuard,
    validation: wgpu::ErrorScopeGuard,
}

impl Scopes {
    fn push(gpu: &Gpu) -> Scopes {
        let oom = gpu.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        Scopes { oom, validation }
    }

    /// Pops both (innermost first); a captured error is logged and returned.
    fn pop(self, gpu: &Gpu) -> Result<(), PipelineError> {
        let validation = pollster::block_on(self.validation.pop());
        let oom = pollster::block_on(self.oom.pop());
        if gpu.is_lost() {
            return Err(PipelineError::DeviceLost);
        }
        if let Some(e) = validation {
            log::error!("graphics device error during a render: {e}");
            return Err(PipelineError::Gpu(e.to_string()));
        }
        if let Some(e) = oom {
            log::error!("graphics device out of memory during a render: {e}");
            return Err(PipelineError::OutOfMemory);
        }
        Ok(())
    }
}
