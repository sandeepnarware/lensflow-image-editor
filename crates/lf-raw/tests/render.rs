//! `renderer_for`: a developed RAW uploads through `Renderer::from_frame`, so a device error
//! during its upload is returned to the caller, not only logged.

use half::f16;
use lf_pipeline::test_support::try_shared_gpu;
use lf_pipeline::{FloatFrame, PipelineError};
use lf_raw::develop::Developed;
use lf_raw::exif::ExifSummary;
use lf_raw::open::{Kind, Opened, Picture};
use lf_raw::render::renderer_for;

fn float_photo() -> Opened {
    let (width, height) = (16, 8);
    let frame = FloatFrame {
        width,
        height,
        data: vec![f16::from_f32(0.5); 4 * 16 * 8],
    };
    Opened {
        picture: Picture::Float(Developed {
            frame,
            full_width: width,
            full_height: height,
            reduced: false,
            orientation_applied: false,
        }),
        kind: Kind::Raw,
        exif: ExifSummary::default(),
        full_width: width,
        full_height: height,
        reduced: false,
        jpeg_exif_bytes: None,
    }
}

#[test]
fn a_raw_upload_error_is_returned() {
    let Some(gpu) = try_shared_gpu() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let opened = float_photo();
    let r = renderer_for(&gpu, &opened).unwrap();
    assert_eq!((r.width, r.height), (16, 8));

    lf_pipeline::test_support::fail_next_upload();
    match renderer_for(&gpu, &opened) {
        Err(PipelineError::Gpu(msg)) => assert!(!msg.is_empty()),
        Err(e) => panic!("expected PipelineError::Gpu, got {e}"),
        Ok(_) => panic!("the RAW upload's device error was not returned"),
    }
    assert!(gpu.take_error().is_none(), "captured by the call");
}
