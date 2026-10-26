//! `Target::view` and `Target::texture` are readable through accessors, so the app can hand a
//! canvas to egui (`register_native_texture`) without reaching into fields.

use lf_pipeline::test_support::try_shared_gpu;
use lf_pipeline::texture::{FORMAT, Target};

#[test]
fn target_view_accessor_exists() {
    let Some(gpu) = try_shared_gpu() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let t = Target::new(&gpu, 8, 4, false).unwrap();
    let view: &wgpu::TextureView = t.view();
    let texture: &wgpu::Texture = t.texture();
    assert!(view == &t.view);
    assert_eq!(texture.format(), FORMAT);
    assert_eq!((texture.width(), texture.height()), (8, 4));
}
