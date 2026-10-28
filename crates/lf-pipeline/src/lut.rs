//! Desktop only: 3D LUT looks ([`lf_settings::lut`]). The tables are loaded once (the app
//! reads the catalogue's LUT folder when it opens) into a process-wide registry by name, so
//! every renderer (the preview, thumbnails, export) finds the same one without the settings
//! carrying the table. A renderer uploads the chosen table into a 3D `Rgba16Float` texture
//! when the choice changes; with no LUT (or one not loaded) it keeps a 2×2×2 stand-in that
//! the develop pass never reads.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use half::f16;
use lf_settings::EditSettings;
use lf_settings::lut::{Lut, LutChoice};

use crate::error::PipelineError;
use crate::gpu::Gpu;

static REGISTRY: RwLock<Option<HashMap<String, Arc<Lut>>>> = RwLock::new(None);

/// Makes `lut` available under `name` (replacing one of that name).
pub fn register(name: &str, lut: Lut) {
    let mut reg = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    reg.get_or_insert_with(HashMap::new)
        .insert(name.to_owned(), Arc::new(lut));
}

/// Forgets every table (another catalogue is opening).
pub fn clear() {
    *REGISTRY.write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The table registered under `name`.
pub fn get(name: &str) -> Option<Arc<Lut>> {
    REGISTRY
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .get(name)
        .cloned()
}

/// Every registered name, sorted (case ignored).
pub fn names() -> Vec<String> {
    let mut names: Vec<String> = REGISTRY
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|r| r.keys().cloned().collect())
        .unwrap_or_default();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

pub struct LutTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    /// The table uploaded (compared by identity: a name registered again is a new table).
    shown: Option<Arc<Lut>>,
}

fn texture(gpu: &Gpu, size: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("lut"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: size,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

impl LutTexture {
    /// The stand-in.
    pub fn new(gpu: &Gpu) -> LutTexture {
        let (texture, view) = texture(gpu, 2);
        LutTexture {
            texture,
            view,
            shown: None,
        }
    }

    /// Forget what was uploaded, so the next [`LutTexture::update`] uploads again.
    pub fn invalidate(&mut self) {
        self.shown = None;
    }

    /// Uploads the LUT `s` chooses when it changed; true when one is ready to sample (the
    /// develop pass applies it only then).
    pub fn update(&mut self, gpu: &Gpu, s: &EditSettings) -> Result<bool, PipelineError> {
        let Some(lut) = LutChoice::from_settings(s).and_then(|c| get(&c.name)) else {
            return Ok(false);
        };
        if self.shown.as_ref().is_some_and(|l| Arc::ptr_eq(l, &lut)) {
            return Ok(true);
        }
        let n = lut.size as u32;
        if n > gpu.limits.max_texture_dimension_3d {
            return Err(PipelineError::TooLarge {
                width: n,
                height: n,
                limit: gpu.limits.max_texture_dimension_3d,
            });
        }
        if self.texture.width() != n {
            let (t, v) = texture(gpu, n);
            let old = std::mem::replace(&mut self.texture, t);
            old.destroy();
            self.view = v;
        }
        let data: Vec<f16> = lut
            .data
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 1.0].map(f16::from_f32))
            .collect();
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(n * 8),
                rows_per_image: Some(n),
            },
            wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: n,
            },
        );
        self.shown = Some(lut);
        Ok(true)
    }
}
