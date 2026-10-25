use crate::correct::{CorrectOptions, Residual, correct};
use crate::measure::Stats;
use crate::util::normalise_settings;
use lf_settings::{EditSettings, HslBand};

/// The calibration loop for matching a reference: starting from the AI's proposal (or
/// defaults), render the contact sheet, measure it, correct the settings towards the
/// reference, and repeat until the measurable parts agree or the rounds run out. The renderer
/// is injected (`render`), so the loop is the same on the GPU and in tests.
///
/// What only the model can judge survives every round: vignette, grain, contrast,
/// clarity/texture/dehaze, black and white, the midtone wheel and the balance. A
/// black-and-white start also keeps saturation untouched.
pub struct CalibrateOptions<'a> {
    pub start: &'a EditSettings,
    pub reference: &'a Stats,
    /// The sheet's percentile lumas with default settings: the master curve's x positions.
    pub anchors: &'a [f64],
    /// Renders the sheet with the settings and measures it (effects like vignette and grain
    /// zeroed by the caller).
    pub render: &'a mut dyn FnMut(&EditSettings) -> Stats,
    pub scene_bands: &'a [HslBand],
    /// Default 4.
    pub rounds: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Calibration {
    pub settings: EditSettings,
    pub rounds: usize,
    pub residual: Residual,
}

pub const LUMA_THRESHOLD: f64 = 0.01;
pub const CHROMA_THRESHOLD: f64 = 0.005;

pub fn calibrate(opts: CalibrateOptions<'_>) -> Calibration {
    let CalibrateOptions {
        start,
        reference,
        anchors,
        render,
        scene_bands,
        rounds,
    } = opts;
    let max_rounds = rounds.unwrap_or(4).max(1);
    let mut current = normalise_settings(start);
    let mut residual = Residual {
        luma: f64::INFINITY,
        chroma: f64::INFINITY,
    };
    let correct_opts = CorrectOptions {
        scene_bands: scene_bands.to_vec(),
        ..CorrectOptions::default()
    };

    for round in 1..=max_rounds {
        let rendered = render(&current);
        let step = correct(&current, &rendered, reference, anchors, &correct_opts);
        residual = step.residual;
        if residual.luma < LUMA_THRESHOLD && residual.chroma < CHROMA_THRESHOLD {
            return Calibration {
                settings: current,
                rounds: round,
                residual,
            };
        }
        current = preserve(start, &step.settings);
    }

    Calibration {
        settings: current,
        rounds: max_rounds,
        residual,
    }
}

/// The start's model-judged values laid back over a corrected round.
pub fn preserve(start: &EditSettings, corrected: &EditSettings) -> EditSettings {
    let mut out = corrected.clone();
    out.vignette = start.vignette;
    out.vignette_midpoint = start.vignette_midpoint;
    out.grain = start.grain;
    out.grain_size = start.grain_size;
    out.contrast = start.contrast;
    out.clarity = start.clarity;
    out.texture = start.texture;
    out.dehaze = start.dehaze;
    out.bw = start.bw;
    out.grading.midtones = start.grading.midtones;
    out.grading.balance = start.grading.balance;
    if start.bw {
        out.saturation = start.saturation;
    }
    out
}
