//! The fit, as the web's `MatchReferencePanel.vue` runs it: measure the reference and the
//! contact sheet, ask the AI to read the look (only when the studio may use AI, and only
//! after a fresh entitlement check), calibrate the grade through the real pipeline until the
//! sheet measures like the reference, and optionally let the AI check the result.
//!
//! Nothing is uploaded but the two JPEGs `desktop/grade` takes (`reference` and `sheet`, at
//! most 1024 px and 700 KB; `lf_grade::sheet::jpeg` keeps them there and
//! `lf_account::grade::extract` checks again).

use crate::account::Generation;
use image::RgbaImage;
use lf_account::entitlement::{Decision, Standing};
use lf_account::grade::{self, Mode, Proposal};
use lf_account::{AccountError, Client};
use lf_grade::sheet::{self, AI_EDGE, MEASURE_EDGE};
use lf_grade::{CalibrateOptions, Residual, Stats, calibrate, measure};
use lf_pipeline::Renderer;
use lf_settings::looks::LOOKS;
use lf_settings::{EditSettings, HslBand};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

pub const READING: &str = "Reading the reference…";
pub const ASKING: &str = "Asking the AI…";
pub const CHECKING: &str = "Checking with the AI…";
pub const AI_OFF: &str = "AI is off for your studio: the grade is fitted from measurements only.";
pub const PLAN_LIMIT: &str = "Your studio's AI requests are used up for this month.";
pub const READ_ONLY: &str =
    "Your LensFlow plan has lapsed: editing works, but preset sync and AI are off until you renew.";
pub const USED_UP: &str = "Your studio's AI requests are used up for this month: the grade is fitted from measurements only.";
pub const STOPPED: &str = "The match was stopped.";
pub const NOT_CHECKED: &str =
    "Your studio could not be checked, so the AI was not asked. Try again in a moment.";
pub const SIGNED_OUT: &str = "You were signed out. Sign in again to match a reference.";
/// The calibration's rounds (`calibrate`'s default).
pub const ROUNDS: usize = 4;
/// The JPEG quality the web sends (0.85).
pub const JPEG_QUALITY: f64 = 0.85;

pub fn fitting(round: usize) -> String {
    format!("Fitting the grade (round {round} of {ROUNDS})…")
}

/// The cost shown before anything is sent: `n` requests of what is left this month.
pub fn cost_line(n: u32, remaining: Option<u64>) -> String {
    let remaining = remaining.map_or_else(|| "unlimited".to_owned(), |r| r.to_string());
    format!(
        "Uses {n} of your {remaining} remaining AI requests (one more per step if the AI has to be asked again)."
    )
}

/// Why AI cannot be used under `decision` (the app's own lock and banner copy).
pub fn lock_copy(decision: &Decision) -> String {
    match decision {
        Decision::Lock(_, message) => message.clone(),
        Decision::SignIn => SIGNED_OUT.to_owned(),
        Decision::ChooseStudio => "Choose a studio first.".to_owned(),
        Decision::Allow(e) | Decision::AllowStale(e, _) => {
            if e.standing != Standing::Active {
                READ_ONLY.to_owned()
            } else if decision.ai_allowed() && used_up(decision) {
                USED_UP.to_owned()
            } else {
                AI_OFF.to_owned()
            }
        }
    }
}

/// The AI requests left this month under `decision` (`None`: unlimited or unknown).
pub fn remaining(decision: &Decision) -> Option<u64> {
    match decision {
        Decision::Allow(e) | Decision::AllowStale(e, _) => e.ai.remaining,
        _ => None,
    }
}

fn used_up(decision: &Decision) -> bool {
    remaining(decision) == Some(0)
}

/// AI may be asked under `decision`: allowed, with requests left (the web's `aiUsable`).
pub fn ai_usable(decision: &Decision) -> bool {
    decision.ai_allowed() && !used_up(decision)
}

/// Why the fit runs from measurements alone under `decision`.
fn off_note(decision: &Decision) -> &'static str {
    if decision.ai_allowed() && used_up(decision) {
        USED_UP
    } else {
        AI_OFF
    }
}

/// Renders the contact sheet (the real pipeline in the app; a stand-in in tests).
pub trait SheetRender: Send {
    fn pixels(&mut self, s: &EditSettings, edge: u32) -> Result<RgbaImage, String>;
}

impl SheetRender for Renderer {
    fn pixels(&mut self, s: &EditSettings, edge: u32) -> Result<RgbaImage, String> {
        Renderer::pixels(self, s, edge).map_err(|e| e.to_string())
    }
}

/// A fresh entitlement decision, asked for before each AI request (the app queues the same
/// check `refresh_now` does on the account lane and waits for its answer); `None` when it
/// could not be had (the app is quitting, or the match was cancelled meanwhile).
pub type Refresh = Box<dyn FnMut(&AtomicBool) -> Option<Decision> + Send>;

/// What the AI steps need: the studio's client, the decision the cost line was shown under,
/// and the fresh check.
pub struct AiLink {
    pub client: Client,
    pub decision: Decision,
    pub refresh: Refresh,
    /// The account generation the link was made in (a 401 signs that session out).
    pub generation: Generation,
}

impl std::fmt::Debug for AiLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiLink")
            .field("decision", &self.decision)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// Waits for the refreshed decision, giving up when `cancel` is set or the sender is gone.
pub fn wait_for(rx: &Receiver<Decision>, cancel: &AtomicBool) -> Option<Decision> {
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(d) => return Some(d),
            Err(RecvTimeoutError::Timeout) if !cancel.load(Ordering::SeqCst) => {}
            Err(_) => return None,
        }
    }
}

/// Everything one fit needs.
pub struct MatchCtx<'a> {
    /// The reference picture (at most `AI_EDGE`).
    pub reference: &'a RgbaImage,
    /// The contact sheet, unedited.
    pub sheet: &'a RgbaImage,
    /// Renders `sheet` with settings.
    pub renderer: &'a mut dyn SheetRender,
    /// `None`, or a decision without AI: the fit runs from measurements alone.
    pub ai: Option<&'a mut AiLink>,
    /// "Let AI check the result (1 more request)".
    pub review: bool,
    /// Checked between steps (and between rounds).
    pub cancel: &'a AtomicBool,
    pub progress: &'a mut dyn FnMut(&str),
}

/// The fitted grade.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub settings: EditSettings,
    pub residual: Residual,
    pub rounds: usize,
    pub requests_used: u32,
    /// Hue bands the AI flagged as scene content (left alone by the fit).
    pub scene_bands: Vec<HslBand>,
    /// The AI's note on the look.
    pub notes: String,
    /// Why the AI was not used, or not fully (AI off, requests used up, unavailable).
    pub ai_note: Option<String>,
    /// The reference's median luma (for matching exposure per photo).
    pub reference_median: f64,
}

#[derive(Debug)]
pub enum MatchError {
    /// The message to show.
    Failed(String),
    /// The dialog was closed.
    Cancelled,
    /// The server signed this device out (asked in this generation).
    Unauthorized(Generation),
}

impl std::fmt::Display for MatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchError::Failed(m) => f.write_str(m),
            MatchError::Cancelled => f.write_str(STOPPED),
            MatchError::Unauthorized(_) => f.write_str(SIGNED_OUT),
        }
    }
}

/// `partial` (the AI's settings) laid key by key over `onto`, as the web's
/// `{ ...onto, ...clipboardSettings(partial) }`.
pub fn merged(onto: &EditSettings, partial: &Value) -> EditSettings {
    let mut base = onto.to_value();
    if let (Some(m), Value::Object(carried)) = (
        base.as_object_mut(),
        crate::clipboard::clipboard_settings(partial),
    ) {
        m.extend(carried);
    }
    lf_settings::normalise(&base)
}

/// Asks the AI once: a fresh entitlement check first, and nothing is sent unless it still
/// allows AI. The outer error stops the fit; the inner one is the request's own.
fn ask(
    link: &mut AiLink,
    cancel: &AtomicBool,
    mode: Mode,
    reference: &[u8],
    sheet: &[u8],
    settings: Option<&EditSettings>,
) -> Result<Result<Proposal, grade::Failed>, MatchError> {
    let fresh = (link.refresh)(cancel);
    if cancel.load(Ordering::SeqCst) {
        return Err(MatchError::Cancelled);
    }
    let Some(fresh) = fresh else {
        return Err(MatchError::Failed(NOT_CHECKED.to_owned()));
    };
    if !ai_usable(&fresh) {
        return Err(MatchError::Failed(lock_copy(&fresh)));
    }
    link.decision = fresh;
    let examples: Vec<_> = LOOKS.iter().collect();
    Ok(grade::extract_with_cost(
        &link.client,
        mode,
        reference,
        sheet,
        &examples,
        settings,
    ))
}

/// The note for a failed AI call (`PlanLimit` has its own copy).
fn unavailable(e: &AccountError, what: &str) -> String {
    if let AccountError::PlanLimit(_) = e {
        return PLAN_LIMIT.to_owned();
    }
    log::warn!("match: the AI could not {what}: {e}");
    format!("AI unavailable ({e}); matched by measurement only.")
}

fn jpeg(img: &RgbaImage) -> Result<Vec<u8>, MatchError> {
    sheet::jpeg(img, AI_EDGE, JPEG_QUALITY)
        .map_err(|e| MatchError::Failed(format!("The picture could not be prepared. {e}")))
}

/// The calibration through the sheet renderer. A failed render or a close ends the rounds
/// (the reference's own statistics measure as a match); the caller reports either after.
struct Fit<'a> {
    renderer: &'a mut dyn SheetRender,
    target: &'a Stats,
    anchors: &'a [f64],
    scene_bands: &'a [HslBand],
    edge: u32,
    cancel: &'a AtomicBool,
    error: &'a mut Option<String>,
}

impl Fit<'_> {
    /// `rounds`: `None` for the full fit (each round announced), `Some(1)` after a review.
    fn run(
        &mut self,
        start: &EditSettings,
        rounds: Option<usize>,
        progress: &mut dyn FnMut(&str),
    ) -> lf_grade::Calibration {
        let mut round = 0;
        let (target, anchors, scene_bands, edge, cancel) = (
            self.target,
            self.anchors,
            self.scene_bands,
            self.edge,
            self.cancel,
        );
        let renderer = &mut *self.renderer;
        let error = &mut *self.error;
        let mut render = |s: &EditSettings| -> Stats {
            if error.is_some() || cancel.load(Ordering::SeqCst) {
                return target.clone();
            }
            round += 1;
            if rounds.is_none() {
                progress(&fitting(round));
            }
            match renderer.pixels(&without_effects(s), edge) {
                Ok(img) => measure(img.as_raw(), 1),
                Err(e) => {
                    *error = Some(e);
                    target.clone()
                }
            }
        };
        calibrate(CalibrateOptions {
            start,
            reference: target,
            anchors,
            render: &mut render,
            scene_bands,
            rounds,
        })
    }
}

/// Vignette and grain cannot be measured on a mosaic: left out of the measurement renders.
fn without_effects(s: &EditSettings) -> EditSettings {
    let mut out = s.clone();
    out.vignette = 0.0;
    out.grain = 0.0;
    out
}

/// Runs the fit (on the `Open` lane: the sheet renderer needs the device).
pub fn run(ctx: MatchCtx<'_>) -> Result<Outcome, MatchError> {
    let MatchCtx {
        reference,
        sheet,
        renderer,
        ai,
        review,
        cancel,
        progress,
    } = ctx;
    let stopped = || cancel.load(Ordering::SeqCst);
    let failed = |e: String| MatchError::Failed(e);

    // 1. What the reference and the untouched sheet measure.
    progress(READING);
    if stopped() {
        return Err(MatchError::Cancelled);
    }
    let target = measure(sheet::scaled(reference, MEASURE_EDGE).as_raw(), 1);
    // The anchors and every calibration render are measured at 512, as the web's.
    let defaults = EditSettings::default_settings();
    let anchors = measure(
        renderer
            .pixels(&defaults, MEASURE_EDGE)
            .map_err(failed)?
            .as_raw(),
        1,
    )
    .percentiles;

    // 2. The AI reads the look (one request), only when the studio may use AI.
    let mut start = defaults.clone();
    let mut scene_bands = Vec::new();
    let mut notes = String::new();
    let mut ai_note = None;
    let mut requests_used = 0;
    let off = ai.as_ref().map_or(AI_OFF, |l| off_note(&l.decision));
    let mut link = ai.filter(|l| ai_usable(&l.decision));
    let mut reference_jpeg = None;
    match link.as_deref_mut() {
        None => ai_note = Some(off.to_owned()),
        Some(link) => {
            let ref_jpeg = jpeg(reference)?;
            let sheet_jpeg = jpeg(sheet)?;
            if stopped() {
                return Err(MatchError::Cancelled);
            }
            progress(ASKING);
            match ask(link, cancel, Mode::Extract, &ref_jpeg, &sheet_jpeg, None)? {
                Ok(p) => {
                    requests_used += p.requests_used;
                    start = merged(&defaults, &p.settings);
                    scene_bands = p.scene_hues;
                    notes = p.notes;
                    reference_jpeg = Some(ref_jpeg);
                }
                Err(failed) => match failed.error {
                    AccountError::Unauthorized => {
                        return Err(MatchError::Unauthorized(link.generation));
                    }
                    e => {
                        requests_used += failed.requests_used;
                        ai_note = Some(unavailable(&e, "read the look"));
                    }
                },
            }
        }
    }
    if stopped() {
        return Err(MatchError::Cancelled);
    }

    // 3. Calibrate through the real pipeline. A failed render or a close ends the rounds
    // (the reference's own statistics measure as a match) and is reported after.
    let mut render_error = None;
    let mut fit = Fit {
        renderer: &mut *renderer,
        target: &target,
        anchors: &anchors,
        scene_bands: &scene_bands,
        edge: MEASURE_EDGE,
        cancel,
        error: &mut render_error,
    };
    let mut result = fit.run(&start, None, progress);

    // 4. The optional review: the model checks the rendered result (one more request), then
    // one more round.
    if let (Some(link), Some(ref_jpeg), true) = (link, &reference_jpeg, review) {
        if stopped() {
            return Err(MatchError::Cancelled);
        }
        progress(CHECKING);
        let rendered = fit
            .renderer
            .pixels(&result.settings, AI_EDGE)
            .map_err(failed)?;
        let rendered = jpeg(&rendered)?;
        let asked = match ask(
            link,
            cancel,
            Mode::Review,
            ref_jpeg,
            &rendered,
            Some(&result.settings),
        ) {
            // A refused check keeps the first reading; the note says why it was not checked.
            Err(MatchError::Failed(message)) => {
                ai_note = Some(message);
                None
            }
            other => Some(other?),
        };
        match asked {
            None => {}
            Some(Ok(p)) => {
                requests_used += p.requests_used;
                if !p.notes.is_empty() {
                    notes = p.notes;
                }
                let next = merged(&result.settings, &p.settings);
                result = fit.run(&next, Some(1), progress);
            }
            Some(Err(failed)) => match failed.error {
                AccountError::Unauthorized => {
                    return Err(MatchError::Unauthorized(link.generation));
                }
                // The first reading stands; the review is a refinement.
                e => {
                    requests_used += failed.requests_used;
                    if matches!(e, AccountError::PlanLimit(_)) {
                        ai_note = Some(PLAN_LIMIT.to_owned());
                    } else {
                        log::warn!("match: the AI could not check the result: {e}");
                    }
                }
            },
        }
    }
    if stopped() {
        return Err(MatchError::Cancelled);
    }
    if let Some(e) = render_error {
        return Err(MatchError::Failed(format!(
            "The contact sheet could not be drawn. {e}"
        )));
    }
    Ok(Outcome {
        settings: result.settings,
        residual: result.residual,
        rounds: result.rounds,
        requests_used,
        scene_bands,
        notes,
        ai_note,
        reference_median: target.median,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use httpmock::prelude::*;
    use lf_account::Instance;
    use lf_account::entitlement::Entitlement;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    /// Hands the sheet back whatever the settings (only the requests matter here).
    struct Unchanged(RgbaImage);

    impl SheetRender for Unchanged {
        fn pixels(&mut self, _: &EditSettings, edge: u32) -> Result<RgbaImage, String> {
            Ok(sheet::scaled(&self.0, edge))
        }
    }

    pub(crate) fn client(server: &MockServer) -> Client {
        let url = reqwest::Url::parse(&server.url("/api")).unwrap();
        Client::new(Instance::custom(url).unwrap(), "1.0.0")
            .with_token("t")
            .with_studio("frames")
    }

    pub(crate) fn allowed() -> Decision {
        Decision::Allow(Entitlement::example(Standing::Active, true))
    }

    pub(crate) fn read_only() -> Decision {
        Decision::Allow(Entitlement::example(Standing::ReadOnly, true))
    }

    /// A link whose fresh check answers `fresh`, counting the checks.
    pub(crate) fn link(
        server: &MockServer,
        shown: Decision,
        fresh: Decision,
    ) -> (AiLink, Arc<AtomicUsize>) {
        let checks = Arc::new(AtomicUsize::new(0));
        let counter = checks.clone();
        let link = AiLink {
            client: client(server),
            decision: shown,
            refresh: Box::new(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                Some(fresh.clone())
            }),
            generation: 4,
        };
        (link, checks)
    }

    /// Noise, so the JPEGs are not trivially small.
    pub(crate) fn noisy(w: u32, h: u32, seed: u32) -> RgbaImage {
        let mut s = seed.wrapping_mul(2_654_435_761).max(1);
        RgbaImage::from_fn(w, h, |x, y| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let n = (s & 0x3f) as u8;
            image::Rgba([
                (x * 255 / w) as u8 / 2 + n,
                (y * 255 / h) as u8 / 2 + n,
                120 + n / 2,
                255,
            ])
        })
    }

    fn fit(
        reference: &RgbaImage,
        sheet: &RgbaImage,
        ai: Option<&mut AiLink>,
        review: bool,
        cancel: &AtomicBool,
    ) -> (Result<Outcome, MatchError>, Vec<String>) {
        let mut renderer = Unchanged(sheet.clone());
        let mut seen = Vec::new();
        let result = run(MatchCtx {
            reference,
            sheet,
            renderer: &mut renderer,
            ai,
            review,
            cancel,
            progress: &mut |t: &str| seen.push(t.to_owned()),
        });
        (result, seen)
    }

    /// The JPEG parts of a multipart body (part name, bytes).
    fn jpeg_parts(body: &[u8]) -> Vec<(String, Vec<u8>)> {
        let line_end = body.iter().position(|&b| b == b'\r').unwrap();
        let boundary = body[..line_end].to_vec();
        let mut parts = Vec::new();
        let mut rest = body;
        while let Some(at) = find(rest, &boundary) {
            rest = &rest[at + boundary.len()..];
            let Some(head_end) = find(rest, b"\r\n\r\n") else {
                break;
            };
            let head = String::from_utf8_lossy(&rest[..head_end]).into_owned();
            let data = &rest[head_end + 4..];
            let end = find(data, &boundary).unwrap_or(data.len());
            if head.contains("image/jpeg") {
                let name = head
                    .split("name=\"")
                    .nth(1)
                    .and_then(|s| s.split('"').next())
                    .unwrap_or("")
                    .to_owned();
                // The data ends with "\r\n" before the next boundary.
                parts.push((name, data[..end.saturating_sub(2)].to_vec()));
            }
        }
        parts
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn ai_path_sends_two_jpegs_under_700kb_and_uses_proposal() {
        let server = MockServer::start();
        let body = Arc::new(Mutex::new(Vec::new()));
        let captured = body.clone();
        let grade = server.mock(|w, t| {
            // GradeRequest.php: mode, reference, sheet (JPEG files), examples (JSON text).
            w.method(POST)
                .path("/api/desktop/grade")
                .header_includes("Content-Type", "multipart/form-data")
                .body_includes("name=\"mode\"\r\n\r\nextract")
                .body_includes("name=\"reference\"; filename=\"reference.jpg\"")
                .body_includes("name=\"sheet\"; filename=\"sheet.jpg\"")
                .body_includes("name=\"examples\"")
                .body_excludes("name=\"settings\"")
                .is_true(move |req| {
                    *captured.lock().unwrap() = req.body_vec();
                    true
                });
            t.status(200).json_body(json!({"data": {
                "settings": {"v": 2, "contrast": 25, "vignette": -20},
                "notes": "Warm and soft.",
                "scene_hues": ["green"],
                "confidence": {},
                "requests_used": 1,
            }}));
        });
        let (mut link, checks) = link(&server, allowed(), allowed());
        let reference = noisy(2400, 1600, 1);
        let sheet = noisy(1024, 1536, 2);
        let cancel = AtomicBool::new(false);
        let (result, seen) = fit(&reference, &sheet, Some(&mut link), false, &cancel);
        let outcome = result.unwrap();
        grade.assert_calls(1);
        assert_eq!(
            checks.load(Ordering::SeqCst),
            1,
            "one check before the call"
        );

        let body = body.lock().unwrap().clone();
        let parts = jpeg_parts(&body);
        let names: Vec<&str> = parts.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["reference", "sheet"], "exactly the two pictures");
        for (name, bytes) in &parts {
            assert!(bytes.len() <= 700 * 1024, "{name}: {} bytes", bytes.len());
            assert!(bytes.starts_with(&[0xFF, 0xD8, 0xFF]), "{name} is a JPEG");
            let img = image::load_from_memory(bytes).unwrap();
            assert!(
                img.width().max(img.height()) <= 1024,
                "{name} at most 1024 px"
            );
        }
        let text = String::from_utf8_lossy(&body);
        assert_eq!(
            text.matches("\"settings\":").count(),
            12,
            "the 12 looks as examples"
        );

        assert_eq!(outcome.requests_used, 1);
        assert_eq!(outcome.notes, "Warm and soft.");
        assert_eq!(outcome.scene_bands, [HslBand::Green]);
        assert_eq!(outcome.ai_note, None);
        // What only the model judges survives the calibration.
        assert_eq!(outcome.settings.contrast, 25.0);
        assert_eq!(outcome.settings.vignette, -20.0);
        assert_eq!(seen[0], READING);
        assert!(seen.contains(&ASKING.to_owned()));
        assert!(seen.contains(&fitting(1)));
    }

    #[test]
    fn the_review_sends_the_settings_and_counts_a_second_request() {
        let server = MockServer::start();
        let extract = server.mock(|w, t| {
            w.method(POST)
                .path("/api/desktop/grade")
                .body_includes("name=\"mode\"\r\n\r\nextract");
            t.status(200).json_body(json!({"data": {
                "settings": {"v": 2, "contrast": 10}, "notes": "First.", "requests_used": 1,
            }}));
        });
        let review = server.mock(|w, t| {
            w.method(POST)
                .path("/api/desktop/grade")
                .body_includes("name=\"mode\"\r\n\r\nreview")
                .body_includes("name=\"settings\"");
            t.status(200).json_body(json!({"data": {
                "settings": {"v": 2, "contrast": 30}, "notes": "Checked.", "requests_used": 1,
            }}));
        });
        let (mut link, checks) = link(&server, allowed(), allowed());
        let cancel = AtomicBool::new(false);
        let (result, seen) = fit(
            &noisy(300, 200, 3),
            &noisy(512, 256, 4),
            Some(&mut link),
            true,
            &cancel,
        );
        let outcome = result.unwrap();
        extract.assert_calls(1);
        review.assert_calls(1);
        assert_eq!(checks.load(Ordering::SeqCst), 2, "a check before each call");
        assert_eq!(outcome.requests_used, 2);
        assert_eq!(outcome.notes, "Checked.");
        assert_eq!(outcome.settings.contrast, 30.0);
        assert!(seen.contains(&CHECKING.to_owned()));
    }

    #[test]
    fn no_request_is_sent_when_ai_is_not_allowed() {
        let server = MockServer::start();
        let grade = server.mock(|w, t| {
            w.method(POST).path("/api/desktop/grade");
            t.status(200).json_body(json!({"data": {"settings": {}}}));
        });
        let (mut link, checks) = link(&server, read_only(), allowed());
        let cancel = AtomicBool::new(false);
        let (result, seen) = fit(
            &noisy(300, 200, 5),
            &noisy(512, 256, 6),
            Some(&mut link),
            true,
            &cancel,
        );
        let outcome = result.unwrap();
        grade.assert_calls(0);
        assert_eq!(checks.load(Ordering::SeqCst), 0, "not even a check");
        assert_eq!(outcome.ai_note.as_deref(), Some(AI_OFF));
        assert_eq!(outcome.requests_used, 0);
        assert!(!seen.contains(&ASKING.to_owned()));

        // Without a link at all, the same.
        let (result, _) = fit(
            &noisy(300, 200, 5),
            &noisy(512, 256, 6),
            None,
            true,
            &cancel,
        );
        assert_eq!(result.unwrap().ai_note.as_deref(), Some(AI_OFF));
        grade.assert_calls(0);
    }

    #[test]
    fn a_decision_that_flips_to_locked_before_the_call_sends_nothing() {
        let server = MockServer::start();
        let grade = server.mock(|w, t| {
            w.method(POST).path("/api/desktop/grade");
            t.status(200).json_body(json!({"data": {"settings": {}}}));
        });
        let cancel = AtomicBool::new(false);
        // The cost line was shown under an active plan; the fresh check says read-only.
        let (mut ai, checks) = link(&server, allowed(), read_only());
        let (result, _) = fit(
            &noisy(300, 200, 7),
            &noisy(512, 256, 8),
            Some(&mut ai),
            false,
            &cancel,
        );
        match result {
            Err(MatchError::Failed(m)) => assert_eq!(m, READ_ONLY),
            other => panic!("expected the lock copy, got {other:?}"),
        }
        assert_eq!(checks.load(Ordering::SeqCst), 1);
        grade.assert_calls(0);

        // Paused meanwhile: the lock screen's own words.
        let paused = Decision::Lock(
            lf_account::LockReason::Paused,
            "Your studio is paused. Resume your LensFlow plan to use lensflow-ImageEditor.".into(),
        );
        let (mut ai, _) = link(&server, allowed(), paused);
        let (result, _) = fit(
            &noisy(300, 200, 7),
            &noisy(512, 256, 8),
            Some(&mut ai),
            false,
            &cancel,
        );
        assert!(
            matches!(&result, Err(MatchError::Failed(m)) if m.starts_with("Your studio is paused.")),
            "{result:?}"
        );
        grade.assert_calls(0);

        // The check could not be had at all: nothing is sent either.
        let mut ai = AiLink {
            client: client(&server),
            decision: allowed(),
            refresh: Box::new(|_| None),
            generation: 4,
        };
        let (result, _) = fit(
            &noisy(300, 200, 7),
            &noisy(512, 256, 8),
            Some(&mut ai),
            false,
            &cancel,
        );
        assert!(matches!(&result, Err(MatchError::Failed(m)) if m == NOT_CHECKED));
        grade.assert_calls(0);
    }

    #[test]
    fn plan_limit_shows_copy() {
        let server = MockServer::start();
        let grade = server.mock(|w, t| {
            w.method(POST).path("/api/desktop/grade");
            t.status(422)
                .json_body(json!({"code": "plan_limit", "message": "Limit reached."}));
        });
        let (mut link, _) = link(&server, allowed(), allowed());
        let cancel = AtomicBool::new(false);
        let (result, _) = fit(
            &noisy(300, 200, 9),
            &noisy(512, 256, 10),
            Some(&mut link),
            true,
            &cancel,
        );
        let outcome = result.unwrap();
        grade.assert_calls(1);
        assert_eq!(outcome.ai_note.as_deref(), Some(PLAN_LIMIT));
        assert_eq!(outcome.requests_used, 0);
    }

    #[test]
    fn a_revoked_token_stops_the_fit_as_unauthorized() {
        let server = MockServer::start();
        server.mock(|w, t| {
            w.method(POST).path("/api/desktop/grade");
            t.status(401)
                .json_body(json!({"message": "Unauthenticated."}));
        });
        let (mut link, _) = link(&server, allowed(), allowed());
        let cancel = AtomicBool::new(false);
        let (result, _) = fit(
            &noisy(300, 200, 11),
            &noisy(512, 256, 12),
            Some(&mut link),
            false,
            &cancel,
        );
        assert!(matches!(result, Err(MatchError::Unauthorized(4))));
    }

    #[test]
    fn cancelling_stops_at_the_next_step_and_sends_nothing() {
        let server = MockServer::start();
        let grade = server.mock(|w, t| {
            w.method(POST).path("/api/desktop/grade");
            t.status(200).json_body(json!({"data": {"settings": {}}}));
        });
        let (mut link, checks) = link(&server, allowed(), allowed());
        let cancel = AtomicBool::new(false);
        let reference = noisy(300, 200, 13);
        let sheet = noisy(512, 256, 14);
        let mut renderer = Unchanged(sheet.clone());
        let result = run(MatchCtx {
            reference: &reference,
            sheet: &sheet,
            renderer: &mut renderer,
            ai: Some(&mut link),
            review: true,
            cancel: &cancel,
            // The dialog is closed while the reference is read.
            progress: &mut |_| cancel.store(true, Ordering::SeqCst),
        });
        assert!(matches!(result, Err(MatchError::Cancelled)));
        assert_eq!(checks.load(Ordering::SeqCst), 0);
        grade.assert_calls(0);
    }

    #[test]
    fn a_refused_check_before_the_review_keeps_the_first_reading() {
        let server = MockServer::start();
        let extract = server.mock(|w, t| {
            w.method(POST)
                .path("/api/desktop/grade")
                .body_includes("name=\"mode\"\r\n\r\nextract");
            t.status(200).json_body(json!({"data": {
                "settings": {"v": 2, "contrast": 15}, "notes": "First.", "requests_used": 1,
            }}));
        });
        let review = server.mock(|w, t| {
            w.method(POST)
                .path("/api/desktop/grade")
                .body_includes("name=\"mode\"\r\n\r\nreview");
            t.status(200).json_body(json!({"data": {"settings": {}}}));
        });
        // Allowed for the reading, read-only by the time of the review.
        let answers = Arc::new(Mutex::new(vec![read_only(), allowed()]));
        let mut ai = AiLink {
            client: client(&server),
            decision: allowed(),
            refresh: Box::new(move |_| answers.lock().unwrap().pop()),
            generation: 4,
        };
        let cancel = AtomicBool::new(false);
        let (result, _) = fit(
            &noisy(300, 200, 21),
            &noisy(512, 256, 22),
            Some(&mut ai),
            true,
            &cancel,
        );
        let outcome = result.unwrap();
        extract.assert_calls(1);
        review.assert_calls(0);
        assert_eq!(outcome.requests_used, 1);
        assert_eq!(outcome.notes, "First.");
        assert_eq!(outcome.settings.contrast, 15.0);
        assert_eq!(outcome.ai_note.as_deref(), Some(READ_ONLY));
    }

    #[test]
    fn a_failed_call_still_counts_the_requests_it_spent() {
        let server = MockServer::start();
        let grade = server.mock(|w, t| {
            w.method(POST).path("/api/desktop/grade");
            t.status(422).json_body(json!({
                "message": "The AI could not read the look.",
                "errors": {"reference": ["The AI could not read the look."]},
                "requests_used": 2,
            }));
        });
        let (mut ai, _) = link(&server, allowed(), allowed());
        let cancel = AtomicBool::new(false);
        let (result, _) = fit(
            &noisy(300, 200, 23),
            &noisy(512, 256, 24),
            Some(&mut ai),
            true,
            &cancel,
        );
        let outcome = result.unwrap();
        grade.assert_calls(1);
        assert_eq!(outcome.requests_used, 2);
        assert!(
            outcome
                .ai_note
                .as_deref()
                .is_some_and(|n| n.starts_with("AI unavailable")),
            "{:?}",
            outcome.ai_note
        );
    }

    #[test]
    fn used_up_requests_turn_the_ai_off() {
        let server = MockServer::start();
        let grade = server.mock(|w, t| {
            w.method(POST).path("/api/desktop/grade");
            t.status(200).json_body(json!({"data": {"settings": {}}}));
        });
        let mut none_left = Entitlement::example(Standing::Active, true);
        none_left.ai.remaining = Some(0);
        let none_left = Decision::Allow(none_left);
        assert!(!ai_usable(&none_left));
        assert!(ai_usable(&allowed()));
        assert_eq!(lock_copy(&none_left), USED_UP);
        let cancel = AtomicBool::new(false);

        // Shown with none left: nothing is asked.
        let (mut ai, checks) = link(&server, none_left.clone(), allowed());
        let (result, _) = fit(
            &noisy(300, 200, 25),
            &noisy(512, 256, 26),
            Some(&mut ai),
            true,
            &cancel,
        );
        assert_eq!(result.unwrap().ai_note.as_deref(), Some(USED_UP));
        assert_eq!(checks.load(Ordering::SeqCst), 0);

        // Used up by the time of the call: stopped with the used-up copy.
        let (mut ai, _) = link(&server, allowed(), none_left);
        let (result, _) = fit(
            &noisy(300, 200, 25),
            &noisy(512, 256, 26),
            Some(&mut ai),
            true,
            &cancel,
        );
        assert!(matches!(&result, Err(MatchError::Failed(m)) if m == USED_UP));
        grade.assert_calls(0);
    }

    #[test]
    fn the_cost_line_and_lock_copy() {
        assert_eq!(
            cost_line(2, Some(9)),
            "Uses 2 of your 9 remaining AI requests (one more per step if the AI has to be asked again)."
        );
        assert_eq!(
            cost_line(1, None),
            "Uses 1 of your unlimited remaining AI requests (one more per step if the AI has to be asked again)."
        );
        assert_eq!(lock_copy(&read_only()), READ_ONLY);
        let mut no_ai = Entitlement::example(Standing::Active, true);
        no_ai.ai.available = false;
        assert_eq!(lock_copy(&Decision::Allow(no_ai)), AI_OFF);
    }

    /// A warm, brighter reference against a neutral sheet: the calibration (measurements
    /// only, through the real pipeline) brings the sheet's tones closer.
    #[test]
    fn calibrate_only_path_reduces_residual_on_synthetic_sheet() {
        let Some(gpu) = lf_pipeline::test_support::try_shared_gpu() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let sheet = RgbaImage::from_fn(512, 256, |x, y| {
            let v = (30 + (x * 160 / 512) + (y % 16)) as u8;
            image::Rgba([v, v, v, 255])
        });
        let reference = RgbaImage::from_fn(400, 300, |x, y| {
            let v = 60.0 + (x as f32 * 170.0 / 400.0) + (y % 16) as f32;
            image::Rgba([
                (v * 1.1 + 12.0).min(255.0) as u8,
                v.min(255.0) as u8,
                (v * 0.78) as u8,
                255,
            ])
        });
        let mut renderer = Renderer::new(gpu, &sheet, 512, 256).unwrap();
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let outcome = run(MatchCtx {
            reference: &reference,
            sheet: &sheet,
            renderer: &mut renderer,
            ai: None,
            review: true,
            cancel: &cancel,
            progress: &mut |t: &str| seen.push(t.to_owned()),
        })
        .unwrap();
        assert_eq!(outcome.ai_note.as_deref(), Some(AI_OFF));
        assert_eq!(outcome.requests_used, 0);
        assert!(seen.contains(&fitting(1)));

        let target = measure(sheet::scaled(&reference, MEASURE_EDGE).as_raw(), 1);
        let luma = |s: &Stats| {
            target
                .percentiles
                .iter()
                .zip(&s.percentiles)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f64::max)
        };
        let chroma = |s: &Stats| {
            (0..3)
                .map(|i| (target.mid_gain[i] - s.mid_gain[i]).abs())
                .fold(0.0, f64::max)
        };
        let mut render = |s: &EditSettings| {
            measure(
                SheetRender::pixels(&mut renderer, s, 512).unwrap().as_raw(),
                1,
            )
        };
        let before = render(&EditSettings::default_settings());
        let after = render(&outcome.settings);
        assert!(
            luma(&after) < luma(&before),
            "luma residual {} -> {}",
            luma(&before),
            luma(&after)
        );
        assert!(
            chroma(&after) < chroma(&before),
            "white balance residual {} -> {}",
            chroma(&before),
            chroma(&after)
        );
        assert!(outcome.residual.luma < luma(&before));
    }
}
