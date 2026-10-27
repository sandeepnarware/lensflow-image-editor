//! The console's grade: `lensflow-image-editor grade` asks for a reference picture and a
//! folder of photos, fits one grade to the folder exactly as "Match a reference…" does, and
//! writes it beside every photo (or as graded JPEG copies with `--out`) — without a window,
//! a catalogue or a sign-in.
//!
//! The fit is the dialog's own code: [`matching::load_reference`] reads the reference, up to
//! [`CAP`] untouched photos are tiled into a contact sheet, and [`steps::run`] measures and
//! calibrates it through the real pipeline. The one difference is where a photo's current
//! edits come from: its sidecar on disk rather than the catalogue, so the command works on a
//! folder the app has never seen.
//!
//! Nothing here touches the photos. A photo's crop, its spots and its named versions stay its
//! own; the grade arrives as a look, exactly as the dialog applies it.

use crate::dialogs::match_reference::samples::CAP;
use crate::dialogs::match_reference::steps::{self, MatchCtx};
use crate::dialogs::match_reference::{self as matching, Reference, final_settings};
use chrono::Utc;
use image::RgbaImage;
use lf_catalog::sidecar::{self, SIDECAR_VERSION, Sidecar};
use lf_grade::sheet::{MEASURE_EDGE, SHEET_COLUMNS, TILE_EDGE, pick_samples, tile};
use lf_grade::{apply_strength, exposure_offset};
use lf_settings::{DevelopSettings, EditSettings};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

/// What `grade --help` prints (and a bad option, to stderr).
pub const USAGE: &str = "Usage: lensflow-image-editor grade [options]

Grades every photo in a folder to match a reference picture, without opening the window.
With no options it asks for the reference and the folder.

  --reference, -r <file>  The picture whose look is matched.
  --folder, -f <dir>      The folder of photos to grade (a bare path works too).
  --out, -o <dir>         Write graded JPEG copies here instead of edit files beside the
                          photos.
  --strength <0-100>      How much of the grade to apply (default 100).
  --exposure              Even out brightness photo by photo; the colour grade stays the same.
  --recursive             Include the folder's subfolders.
  --yes, -y               Do not ask before grading.
  --dry-run               Say what would be graded, and change nothing.
  --help, -h              Print this help and exit.

Photos are never modified: without --out the grade is written to <photo>.lensflow.json,
which the app reads when the folder is added. Exit codes: 0 every photo graded, 1 some were
skipped (each says why), 2 nothing was graded.
";

/// The file extensions [`images_in`] accepts: the kinds the editor can open. Formats it
/// cannot read (HEIC, AVIF) are left out rather than failing one by one.
pub const EXTENSIONS: &[&str] = &[
    // Pictures.
    "jpg", "jpeg", "png", "tif", "tiff", "webp", // Camera raw.
    "3fr", "arw", "cr2", "cr3", "dcr", "dng", "erf", "iiq", "kdc", "mef", "mos", "mrw", "nef",
    "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
];

/// The JPEG quality of `--out` copies.
pub const COPY_QUALITY: u8 = 92;

/// Shown where the dialog shows why the AI was not used.
pub const MEASUREMENTS_ONLY: &str =
    "Fitted from measurements only: the command line does not ask the AI service.";

const ASK_REFERENCE: &str = "Reference picture: ";
const ASK_FOLDER: &str = "Folder of photos: ";
const NEED_REFERENCE: &str = "A reference picture is needed (--reference <file>).";
const NEED_FOLDER: &str = "A folder of photos is needed (--folder <dir>).";
const NOT_A_FILE: &str = "That is not a file.";
const NOT_A_FOLDER: &str = "That is not a folder.";
const NO_PHOTOS: &str = "No photos the editor can open were found there.";
const SAME_FOLDER: &str =
    "--out must be a different folder from the photos, so the originals are never overwritten.";
const SAY_YES: &str = "Nothing was graded. Pass --yes to grade without asking.";

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub reference: Option<PathBuf>,
    pub folder: Option<PathBuf>,
    /// Write graded JPEGs here instead of edit files beside the photos.
    pub out: Option<PathBuf>,
    /// 0..=100, as the dialog's strength.
    pub strength: u8,
    pub exposure: bool,
    pub recursive: bool,
    /// Do not ask before grading.
    pub yes: bool,
    /// Say what would be done and change nothing.
    pub dry_run: bool,
}

impl Default for Args {
    fn default() -> Args {
        Args {
            reference: None,
            folder: None,
            out: None,
            strength: 100,
            exposure: false,
            recursive: false,
            yes: false,
            dry_run: false,
        }
    }
}

/// What `grade …` asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Run(Box<Args>),
    /// `--help`: print [`USAGE`].
    Help,
    /// An option this build does not know (or a missing value): print it and [`USAGE`].
    Bad(String),
}

/// Reads the arguments after `grade`. A bare path is the folder; `--` ends the options.
pub fn parse(args: impl IntoIterator<Item = String>) -> Parsed {
    let mut out = Args::default();
    let mut folder: Option<PathBuf> = None;
    let mut rest = false;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if rest {
            if folder.replace(PathBuf::from(arg)).is_some() {
                return Parsed::Bad("only one folder can be graded at a time".to_owned());
            }
            continue;
        }
        let mut value = |flag: &str| match it.next() {
            Some(v) => Ok(v),
            None => Err(Parsed::Bad(format!("{flag} needs a value"))),
        };
        match arg.as_str() {
            "--help" | "-h" => return Parsed::Help,
            "--" => rest = true,
            "--reference" | "-r" => match value("--reference") {
                Ok(v) => out.reference = Some(PathBuf::from(v)),
                Err(e) => return e,
            },
            "--folder" | "-f" => match value("--folder") {
                Ok(v) => {
                    if folder.replace(PathBuf::from(v)).is_some() {
                        return Parsed::Bad("only one folder can be graded at a time".to_owned());
                    }
                }
                Err(e) => return e,
            },
            "--out" | "-o" => match value("--out") {
                Ok(v) => out.out = Some(PathBuf::from(v)),
                Err(e) => return e,
            },
            "--strength" => {
                let v = match value("--strength") {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                match v.trim().parse::<u16>() {
                    Ok(n) if n <= 100 => out.strength = n as u8,
                    _ => {
                        return Parsed::Bad(format!(
                            "--strength takes a whole number from 0 to 100, not {v}"
                        ));
                    }
                }
            }
            "--exposure" => out.exposure = true,
            "--recursive" => out.recursive = true,
            "--yes" | "-y" => out.yes = true,
            "--dry-run" => out.dry_run = true,
            other if other.starts_with('-') => {
                return Parsed::Bad(format!("unknown option {other}"));
            }
            path => {
                if folder.replace(PathBuf::from(path)).is_some() {
                    return Parsed::Bad("only one folder can be graded at a time".to_owned());
                }
            }
        }
    }
    out.folder = folder;
    Parsed::Run(Box::new(out))
}

/// What the command asks and says; tests answer with a script.
pub trait Console {
    /// Asks for a line; `None` when there is nothing more to read.
    fn ask(&mut self, prompt: &str) -> Option<String>;
    /// A line of its own.
    fn say(&mut self, line: &str);
    /// A line that replaces itself while work goes on.
    fn progress(&mut self, text: &str);
    /// Ends a progress line.
    fn finish_progress(&mut self);
}

/// The terminal the command was started from.
pub struct Stdio;

impl Console for Stdio {
    fn ask(&mut self, prompt: &str) -> Option<String> {
        let mut stdout = io::stdout();
        let _ = write!(stdout, "{prompt}");
        let _ = stdout.flush();
        let mut line = String::new();
        match io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim().to_owned()),
        }
    }

    fn say(&mut self, line: &str) {
        println!("{line}");
    }

    fn progress(&mut self, text: &str) {
        let mut stdout = io::stdout();
        // Padded, so a shorter line never leaves the end of the longer one behind.
        let _ = write!(stdout, "\r{text:<70}");
        let _ = stdout.flush();
    }

    fn finish_progress(&mut self) {
        println!();
    }
}

/// Runs the command from the terminal and returns its exit code. On Windows the console is
/// joined (or made) first, and kept open afterwards when this process made it.
pub fn run(args: &Args) -> i32 {
    // A release build opened from Explorer has no console to print into or read from; make
    // one, and keep it open afterwards so the report can be read.
    let made = crate::cli::ensure_console();
    let code = execute(args, &mut Stdio);
    if made {
        let mut console = Stdio;
        let _ = console.ask("\nPress Enter to close this window.");
    }
    code
}

/// The command itself, asking and printing through `console`.
pub fn execute(args: &Args, console: &mut dyn Console) -> i32 {
    console.say(&format!(
        "lensflow-ImageEditor {} — grade",
        crate::app::VERSION
    ));
    let reference = match resolve(args.reference.as_deref(), console, ASK_REFERENCE, false) {
        Ok(path) => path,
        Err(message) => {
            console.say(message);
            return 2;
        }
    };
    let folder = match resolve(args.folder.as_deref(), console, ASK_FOLDER, true) {
        Ok(path) => path,
        Err(message) => {
            console.say(message);
            return 2;
        }
    };
    let photos = match images_in(&folder, args.recursive) {
        Ok(photos) if photos.is_empty() => {
            console.say(&format!("{NO_PHOTOS} ({})", folder.display()));
            return 1;
        }
        Ok(photos) => photos,
        Err(e) => {
            console.say(&format!(
                "The folder {} could not be read. {e}",
                folder.display()
            ));
            return 2;
        }
    };
    console.say(&format!("Reference   {}", reference.display()));
    console.say(&format!(
        "Folder      {} ({} photos)",
        folder.display(),
        photos.len()
    ));
    if let Some(out) = &args.out {
        if same_folder(out, &folder) {
            console.say(SAME_FOLDER);
            return 2;
        }
        console.say(&format!(
            "Output      graded JPEG copies in {}",
            out.display()
        ));
    } else {
        console.say("Output      edit files beside the photos");
    }
    if args.dry_run {
        console.say(&format!(
            "Dry run: {} photos would be graded at {}% strength{}.",
            photos.len(),
            args.strength,
            if args.exposure {
                ", matching exposure photo by photo"
            } else {
                ""
            }
        ));
        return 0;
    }
    if !args.yes {
        let Some(answer) = console.ask(&confirm_text(photos.len())) else {
            console.say(SAY_YES);
            return 2;
        };
        if !is_yes(&answer) {
            console.say(SAY_YES);
            return 1;
        }
    }
    let gpu = match crate::gpu::headless() {
        Ok(gpu) => gpu,
        Err(e) => {
            console.say(&format!("The graphics device could not be opened. {e}"));
            return 2;
        }
    };
    match grade_folder(&gpu, &photos, &reference, args, console) {
        Ok(report) => {
            report.say(console);
            if report.skipped.is_empty() { 0 } else { 1 }
        }
        Err(message) => {
            console.say(&message);
            2
        }
    }
}

/// "Apply the grade to all 42 photos? [Y/n] "
pub fn confirm_text(n: usize) -> String {
    let noun = if n == 1 { "photo" } else { "photos" };
    format!("Apply the grade to all {n} {noun}? [Y/n] ")
}

/// An answer that means yes (Enter counts).
fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_lowercase().as_str(), "" | "y" | "yes")
}

/// The path from the command line, or the answer to `prompt`; checked to be a file or a
/// folder. Paths pasted with quotes (from Explorer) lose them.
fn resolve(
    given: Option<&Path>,
    console: &mut dyn Console,
    prompt: &str,
    want_dir: bool,
) -> Result<PathBuf, &'static str> {
    let path = match given {
        Some(path) => path.to_path_buf(),
        None => {
            let some = console.ask(prompt);
            let answer = some.as_deref().unwrap_or("").trim();
            let answer = answer.trim_matches(['"', '\'']).trim();
            if answer.is_empty() {
                return Err(if want_dir {
                    NEED_FOLDER
                } else {
                    NEED_REFERENCE
                });
            }
            PathBuf::from(answer)
        }
    };
    let ok = if want_dir {
        path.is_dir()
    } else {
        path.is_file()
    };
    if !ok {
        return Err(if want_dir { NOT_A_FOLDER } else { NOT_A_FILE });
    }
    Ok(path)
}

/// Whether `dir` and `folder` name the same place.
fn same_folder(dir: &Path, folder: &Path) -> bool {
    match (std::fs::canonicalize(dir), std::fs::canonicalize(folder)) {
        (Ok(a), Ok(b)) => a == b,
        _ => dir == folder,
    }
}

/// Every photo the editor can open in `folder`, in name order; with `recursive`, its
/// subfolders too (name order at each level). Hidden files and the app's own sidecars are
/// left out.
pub fn images_in(folder: &Path, recursive: bool) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(folder, recursive, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(folder: &Path, recursive: bool, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if recursive {
                walk(&path, recursive, out)?;
            }
        } else if kind.is_file() && is_photo(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// A file's extension is one the editor opens.
fn is_photo(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}

/// One photo to grade: its file, the edits it carries now, and whether it has any.
#[derive(Debug, Clone)]
pub struct Photo {
    pub path: PathBuf,
    pub current: EditSettings,
    pub edited: bool,
}

impl Photo {
    /// The photo with what its sidecar says. A sidecar that cannot be read is an error: the
    /// command never writes over one, since it may hold edits someone wants back.
    pub fn read(path: PathBuf) -> Result<Photo, String> {
        let side = sidecar::read(&sidecar::path_for(&path))
            .map_err(|e| sidecar::damaged_message(&path, &e))?;
        let (current, edited) = match side {
            Some(side) => {
                let edited = side.settings != EditSettings::default_settings();
                (side.settings, edited)
            }
            None => (EditSettings::default_settings(), false),
        };
        Ok(Photo {
            path,
            current,
            edited,
        })
    }
}

/// How a run went.
#[derive(Debug, Clone)]
pub struct Report {
    pub total: usize,
    pub graded: Vec<PathBuf>,
    pub skipped: Vec<(PathBuf, String)>,
    /// The fitted grade, after the strength.
    pub grade: EditSettings,
    /// How many photos shaped the grade.
    pub samples: usize,
    pub rounds: usize,
    pub seconds: f64,
    /// Where graded copies went, when `--out` asked for copies rather than edit files.
    pub out: Option<PathBuf>,
}

impl Default for Report {
    fn default() -> Report {
        Report {
            total: 0,
            graded: Vec::new(),
            skipped: Vec::new(),
            // `EditSettings` has no `Default` (its defaults are `default_settings`).
            grade: EditSettings::default_settings(),
            samples: 0,
            rounds: 0,
            seconds: 0.0,
            out: None,
        }
    }
}

impl Report {
    /// The closing lines: what was written, what was not, and in how long.
    pub fn say(&self, console: &mut dyn Console) {
        console.finish_progress();
        for (path, why) in &self.skipped {
            console.say(&format!("  skipped {}: {why}", path.display()));
        }
        if self.graded.is_empty() {
            console.say("No photo was graded.");
        } else if self.graded.len() == self.total {
            console.say(&format!(
                "Graded all {} photos in {:.1} s.",
                self.total, self.seconds
            ));
        } else {
            console.say(&format!(
                "Graded {} of {} photos in {:.1} s.",
                self.graded.len(),
                self.total,
                self.seconds
            ));
        }
        console.say(&format!(
            "The grade was fitted to {} photos in {} rounds; {}",
            self.samples, self.rounds, MEASUREMENTS_ONLY
        ));
        console.say(&summary(&self.grade));
        if !self.graded.is_empty() {
            match &self.out {
                Some(out) => console.say(&format!(
                    "The graded copies are in {}; the photos themselves are unchanged.",
                    out.display()
                )),
                None => console.say(
                    "Add the folder in lensflow-ImageEditor to see the grade; the photos \
                     themselves are unchanged.",
                ),
            }
        }
    }
}

/// The grade's main adjustments in a sentence: "exposure +0.35 EV, contrast +12, …".
pub fn summary(grade: &EditSettings) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut push = |name: &str, value: f64, unit: &str| {
        if value.abs() >= 0.005 {
            parts.push(format!("{name} {}{unit}", number(value)));
        }
    };
    push("exposure", grade.exposure, " EV");
    push("contrast", grade.contrast, "");
    push("highlights", grade.highlights, "");
    push("shadows", grade.shadows, "");
    push("whites", grade.whites, "");
    push("blacks", grade.blacks, "");
    push("temperature", grade.temperature, "");
    push("tint", grade.tint, "");
    push("vibrance", grade.vibrance, "");
    push("saturation", grade.saturation, "");
    push("clarity", grade.clarity, "");
    push("texture", grade.texture, "");
    push("dehaze", grade.dehaze, "");
    push("sharpen", grade.sharpen, "");
    push("vignette", grade.vignette, "");
    push("grain", grade.grain, "");
    let moved = [
        &grade.curve.rgb,
        &grade.curve.r,
        &grade.curve.g,
        &grade.curve.b,
    ]
    .iter()
    .any(|points| points.iter().any(|[x, y]| (y - x).abs() > 0.002));
    if moved {
        parts.push("a curve".to_owned());
    }
    let bands = grade
        .hsl
        .0
        .iter()
        .any(|band| band.h.abs() >= 0.05 || band.s.abs() >= 0.05 || band.l.abs() >= 0.05);
    if bands {
        parts.push("HSL bands".to_owned());
    }
    let wheels = [
        &grade.grading.shadows,
        &grade.grading.midtones,
        &grade.grading.highlights,
    ];
    if wheels.iter().any(|wheel| wheel.sat.abs() >= 0.05) {
        parts.push("colour grading".to_owned());
    }
    if grade.bw {
        parts.push("black and white".to_owned());
    }
    if parts.is_empty() {
        return "The grade is neutral: the photos would look as they were taken.".to_owned();
    }
    format!("The grade: {}.", parts.join(", "))
}

/// "+0.35", "+12", "-3.5": as few decimals as the number needs (two at most).
fn number(value: f64) -> String {
    let text = format!("{value:+.2}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    match trimmed {
        "-0" | "+0" => "+0".to_owned(),
        other => other.to_owned(),
    }
}

/// What one fit produced.
struct Fitted {
    grade: EditSettings,
    samples: usize,
    rounds: usize,
    /// The reference's median luma, for matching exposure photo by photo.
    reference_median: f64,
}

/// Fits the grade to `paths` and writes it to each: an edit file beside the photo, or a
/// graded JPEG in `args.out`. The photos themselves are never written.
pub fn grade_folder(
    gpu: &Arc<lf_pipeline::Gpu>,
    paths: &[PathBuf],
    reference: &Path,
    args: &Args,
    console: &mut dyn Console,
) -> Result<Report, String> {
    let started = Instant::now();
    let mut photos = Vec::with_capacity(paths.len());
    let mut skipped = Vec::new();
    for path in paths {
        match Photo::read(path.clone()) {
            Ok(photo) => photos.push(photo),
            // A sidecar that cannot be read is left alone and the photo left out.
            Err(why) => skipped.push((path.clone(), why)),
        }
    }
    if photos.is_empty() {
        return Err(skipped
            .first()
            .map(|(_, why)| why.clone())
            .unwrap_or_else(|| NO_PHOTOS.to_owned()));
    }
    let fitted = fit(gpu, &photos, reference, args.strength, console)?;
    let open = |path: &Path, edge: u32| matching::open_default(gpu, None, path, edge);
    let mut report = Report {
        total: paths.len(),
        skipped,
        grade: fitted.grade.clone(),
        samples: fitted.samples,
        rounds: fitted.rounds,
        out: args.out.clone(),
        ..Report::default()
    };
    let mut medians: HashMap<usize, f64> = HashMap::new();
    let mut taken: HashMap<String, usize> = HashMap::new();
    for (i, photo) in photos.iter().enumerate() {
        if let Some(text) = progress_text(i + 1, photos.len()) {
            console.progress(&text);
        }
        let offset = if args.exposure {
            let median = match medians.get(&i) {
                Some(m) => *m,
                None => match open(&photo.path, TILE_EDGE) {
                    Ok(bitmap) => {
                        let m = matching::samples::median(&bitmap);
                        medians.insert(i, m);
                        m
                    }
                    Err(e) => {
                        report.skipped.push((photo.path.clone(), e));
                        continue;
                    }
                },
            };
            Some(exposure_offset(fitted.reference_median, median))
        } else {
            None
        };
        let settings = final_settings(&photo.current, &fitted.grade, offset);
        let written = match &args.out {
            Some(out) => render_copy(gpu, &photo.path, &settings, out, &mut taken),
            None => write_edit(&photo.path, &settings),
        };
        match written {
            Ok(()) => report.graded.push(photo.path.clone()),
            Err(e) => report.skipped.push((photo.path.clone(), e)),
        }
    }
    report.seconds = started.elapsed().as_secs_f64();
    Ok(report)
}

/// A progress line every 25 photos; small folders go straight to the result.
fn progress_text(done: usize, total: usize) -> Option<String> {
    (done.is_multiple_of(25) || done == total)
        .then(|| format!("Writing the grade… {done} of {total}"))
}

/// Measures the folder against the reference and calibrates one grade for all of it.
fn fit(
    gpu: &Arc<lf_pipeline::Gpu>,
    photos: &[Photo],
    reference: &Path,
    strength: u8,
    console: &mut dyn Console,
) -> Result<Fitted, String> {
    let open = |path: &Path, edge: u32| matching::open_default(gpu, None, path, edge);
    // The grade is fitted to the untouched photos, as the dialog does.
    let picked = pick_samples(photos, |p| p.edited, CAP).0;
    let reference = matching::load_reference(&Reference::File(reference.to_path_buf()), &open)?;
    let mut bitmaps: Vec<RgbaImage> = Vec::with_capacity(picked.len());
    for (i, photo) in picked.iter().enumerate() {
        console.progress(&format!(
            "Reading your photos… {} of {}",
            i + 1,
            picked.len()
        ));
        match open(&photo.path, MEASURE_EDGE) {
            Ok(bitmap) => bitmaps.push(bitmap),
            Err(e) => console.say(&format!("  skipped {}: {e}", photo.path.display())),
        }
    }
    if bitmaps.is_empty() {
        return Err(matching::samples::NONE_READ.to_owned());
    }
    let samples = bitmaps.len();
    let sheet = tile(&bitmaps, TILE_EDGE, SHEET_COLUMNS);
    let mut renderer =
        lf_pipeline::Renderer::new(gpu.clone(), &sheet, sheet.width(), sheet.height())
            .map_err(|e| format!("The contact sheet could not be drawn. {e}"))?;
    let outcome = steps::run(MatchCtx {
        reference: &reference,
        sheet: &sheet,
        renderer: &mut renderer,
        // No account, so no AI: the grade comes from measurements alone.
        ai: None,
        review: false,
        cancel: &AtomicBool::new(false),
        progress: &mut |text: &str| console.progress(text),
    })
    .map_err(|e| e.to_string())?;
    Ok(Fitted {
        grade: apply_strength(&outcome.settings, f64::from(strength)),
        samples,
        rounds: outcome.rounds,
        reference_median: outcome.reference_median,
    })
}

/// Writes `settings` beside the photo, keeping its named versions, its unknown keys and its
/// creation time; the edit file becomes the newer of it and the catalogue's copy.
pub fn write_edit(path: &Path, settings: &EditSettings) -> Result<(), String> {
    let target = sidecar::path_for(path);
    let mut side = match sidecar::read(&target).map_err(|e| sidecar::damaged_message(path, &e))? {
        Some(side) => side,
        None => Sidecar {
            v: SIDECAR_VERSION,
            settings: settings.clone(),
            versions: Vec::new(),
            updated_at: Utc::now(),
            extra: serde_json::Map::new(),
        },
    };
    side.settings = settings.clone();
    side.updated_at = Utc::now();
    sidecar::write(&target, &side).map_err(|e| e.to_string())
}

/// The JPEG `path` is written as in `out`: its stem, `.jpg`, and `-2`, `-3`… when two sources
/// share a stem.
pub fn copy_name(path: &Path, out: &Path, taken: &mut HashMap<String, usize>) -> PathBuf {
    let stem = path
        .file_stem()
        .map_or_else(|| "photo".to_owned(), |s| s.to_string_lossy().into_owned());
    let n = taken.entry(stem.clone()).or_insert(0);
    *n += 1;
    let name = if *n == 1 {
        format!("{stem}.jpg")
    } else {
        format!("{stem}-{n}.jpg")
    };
    out.join(name)
}

/// Renders `path` with `settings` into a graded JPEG in `out`.
pub fn render_copy(
    gpu: &Arc<lf_pipeline::Gpu>,
    path: &Path,
    settings: &EditSettings,
    out: &Path,
    taken: &mut HashMap<String, usize>,
) -> Result<(), String> {
    let dest = copy_name(path, out, taken);
    let develop = DevelopSettings::from_settings(settings);
    let opened = lf_raw::open::open(
        path,
        lf_raw::demosaic::Quality::Best,
        &develop,
        gpu.max_edge(),
    )
    .map_err(|e| e.to_string())?;
    let mut renderer = lf_raw::render::renderer_for(gpu, &opened).map_err(|e| e.to_string())?;
    let image = renderer.render(settings).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    write_jpeg(&dest, image)
}

/// Encodes `image` as a JPEG at [`COPY_QUALITY`], through a temp file so a half-written copy
/// never appears.
fn write_jpeg(dest: &Path, image: RgbaImage) -> Result<(), String> {
    let tmp = dest.with_extension("jpg.part");
    let write = || -> Result<(), String> {
        let file = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
            io::BufWriter::new(file),
            COPY_QUALITY,
        );
        let rgb = image::DynamicImage::ImageRgba8(image).to_rgb8();
        rgb.write_with_encoder(encoder)
            .map_err(|e| format!("{}: {e}", tmp.display()))
    };
    write()?;
    std::fs::rename(&tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", dest.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_settings::{Crop, Spot, SpotMode};

    fn parse_strs(args: &[&str]) -> Parsed {
        parse(args.iter().map(|a| (*a).to_owned()))
    }

    fn run_of(args: &[&str]) -> Args {
        match parse_strs(args) {
            Parsed::Run(args) => *args,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn options_are_read_and_defaulted() {
        let none = run_of(&[]);
        assert_eq!(none, Args::default());
        assert_eq!(none.strength, 100);

        let all = run_of(&[
            "--reference",
            "ref.jpg",
            "--folder",
            "photos",
            "--out",
            "graded",
            "--strength",
            "60",
            "--exposure",
            "--recursive",
            "--yes",
        ]);
        assert_eq!(all.reference, Some(PathBuf::from("ref.jpg")));
        assert_eq!(all.folder, Some(PathBuf::from("photos")));
        assert_eq!(all.out, Some(PathBuf::from("graded")));
        assert_eq!(all.strength, 60);
        assert!(all.exposure && all.recursive && all.yes);
        assert!(!all.dry_run);

        // Short flags, and a bare path is the folder.
        let short = run_of(&["-r", "ref.jpg", "-f", "photos", "-o", "out", "-y"]);
        assert_eq!(short.reference, Some(PathBuf::from("ref.jpg")));
        assert_eq!(short.folder, Some(PathBuf::from("photos")));
        assert_eq!(short.out, Some(PathBuf::from("out")));
        assert!(short.yes);
        assert_eq!(
            run_of(&["D:/shots/day1"]).folder,
            Some(PathBuf::from("D:/shots/day1"))
        );
        // `--` ends the options, so a folder may begin with a dash.
        assert_eq!(run_of(&["--", "-odd"]).folder, Some(PathBuf::from("-odd")));
        // A strength at either end, and the dry run.
        assert_eq!(run_of(&["--strength", "0"]).strength, 0);
        assert!(run_of(&["--dry-run"]).dry_run);
    }

    #[test]
    fn bad_options_are_refused() {
        assert_eq!(parse_strs(&["--help"]), Parsed::Help);
        assert_eq!(parse_strs(&["-h"]), Parsed::Help);
        assert!(matches!(parse_strs(&["--nope"]), Parsed::Bad(m) if m.contains("--nope")));
        assert!(matches!(parse_strs(&["--reference"]), Parsed::Bad(m) if m.contains("value")));
        assert!(
            matches!(parse_strs(&["--strength", "101"]), Parsed::Bad(m) if m.contains("0 to 100"))
        );
        assert!(matches!(parse_strs(&["--strength", "x"]), Parsed::Bad(_)));
        assert!(matches!(parse_strs(&["a", "b"]), Parsed::Bad(m) if m.contains("one folder")));
        assert!(matches!(
            parse_strs(&["--folder", "a", "b"]),
            Parsed::Bad(_)
        ));
    }

    #[test]
    fn only_the_kinds_the_editor_opens_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let write = |name: &str| {
            std::fs::write(root.join(name), b"x").unwrap();
            root.join(name)
        };
        let jpg = write("b.jpg");
        let png = write("a.PNG");
        let raw = write("c.cr2");
        write("notes.txt");
        write("d.heic");
        write("photo.jpg.lensflow.json");
        write(".hidden.jpg");
        let nested = root.join("sub");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("e.jpeg"), b"x").unwrap();

        let flat = images_in(root, false).unwrap();
        assert_eq!(flat, vec![png.clone(), jpg.clone(), raw.clone()]);
        let deep = images_in(root, true).unwrap();
        assert_eq!(deep.len(), 4, "{deep:?}");
        assert!(deep.contains(&nested.join("e.jpeg")));
        // A folder that is not there is an error, not an empty list.
        assert!(images_in(&root.join("nope"), false).is_err());
    }

    /// Answers with a script, and keeps everything said.
    #[derive(Default)]
    struct Script {
        answers: Vec<String>,
        said: Vec<String>,
    }

    impl Console for Script {
        fn ask(&mut self, prompt: &str) -> Option<String> {
            self.said.push(prompt.to_owned());
            if self.answers.is_empty() {
                None
            } else {
                Some(self.answers.remove(0))
            }
        }

        fn say(&mut self, line: &str) {
            self.said.push(line.to_owned());
        }

        fn progress(&mut self, text: &str) {
            self.said.push(text.to_owned());
        }

        fn finish_progress(&mut self) {}
    }

    impl Script {
        fn with(answers: &[&str]) -> Script {
            Script {
                answers: answers.iter().map(|a| (*a).to_owned()).collect(),
                said: Vec::new(),
            }
        }

        fn all(&self) -> String {
            self.said.join("\n")
        }
    }

    fn photos(n: usize) -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.keep();
        for i in 0..n {
            std::fs::write(path.join(format!("p{i}.jpg")), b"x").unwrap();
        }
        path
    }

    #[test]
    fn the_command_asks_for_what_is_missing() {
        let folder = photos(3);
        let reference = folder.join("p0.jpg");
        let mut console = Script::with(&[reference.to_str().unwrap(), folder.to_str().unwrap()]);
        // --dry-run stops before any device work.
        let code = execute(
            &Args {
                dry_run: true,
                ..Args::default()
            },
            &mut console,
        );
        assert_eq!(code, 0, "{}", console.all());
        assert!(console.all().contains("3 photos"), "{}", console.all());
        assert!(console.all().contains("Dry run"), "{}", console.all());

        // Nothing more to read: each half says what it needs, and nothing is graded.
        let mut empty = Script::default();
        assert_eq!(execute(&Args::default(), &mut empty), 2);
        assert!(empty.all().contains(NEED_REFERENCE), "{}", empty.all());

        let mut half = Script::with(&[reference.to_str().unwrap()]);
        assert_eq!(execute(&Args::default(), &mut half), 2);
        assert!(half.all().contains(NEED_FOLDER), "{}", half.all());

        // A path that is not what it says, and a folder with no photos.
        let mut wrong = Script::with(&[folder.to_str().unwrap(), "nope"]);
        assert_eq!(execute(&Args::default(), &mut wrong), 2);
        assert!(wrong.all().contains(NOT_A_FILE), "{}", wrong.all());

        let empty_dir = tempfile::tempdir().unwrap();
        let mut none = Script::with(&[
            reference.to_str().unwrap(),
            empty_dir.path().to_str().unwrap(),
        ]);
        assert_eq!(execute(&Args::default(), &mut none), 1);
        assert!(none.all().contains(NO_PHOTOS), "{}", none.all());

        // Quotes pasted from Explorer are dropped.
        let quoted = format!("\"{}\"", folder.display());
        let mut console = Script::with(&[reference.to_str().unwrap(), &quoted]);
        assert_eq!(
            execute(
                &Args {
                    dry_run: true,
                    ..Args::default()
                },
                &mut console
            ),
            0,
            "{}",
            console.all()
        );

        // --out may not be the photos' own folder.
        let mut console = Script::with(&[reference.to_str().unwrap(), folder.to_str().unwrap()]);
        let code = execute(
            &Args {
                out: Some(folder.clone()),
                yes: true,
                ..Args::default()
            },
            &mut console,
        );
        assert_eq!(code, 2);
        assert!(console.all().contains(SAME_FOLDER), "{}", console.all());
    }

    #[test]
    fn a_no_answer_grades_nothing() {
        let folder = photos(2);
        let reference = folder.join("p0.jpg");
        let mut console =
            Script::with(&[reference.to_str().unwrap(), folder.to_str().unwrap(), "n"]);
        assert_eq!(execute(&Args::default(), &mut console), 1);
        assert!(
            console.all().contains("Nothing was graded"),
            "{}",
            console.all()
        );
        assert!(
            !folder.join("p0.jpg.lensflow.json").exists(),
            "a no writes nothing"
        );
        // Enter alone means yes.
        assert!(is_yes(""));
        assert!(is_yes("Y") && is_yes("yes"));
        assert!(!is_yes("n"));
        assert_eq!(confirm_text(1), "Apply the grade to all 1 photo? [Y/n] ");
        assert_eq!(confirm_text(2), "Apply the grade to all 2 photos? [Y/n] ");
    }

    #[test]
    fn an_edit_file_is_written_and_kept_whole() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("a.jpg");
        std::fs::write(&photo, b"x").unwrap();
        let mut grade = EditSettings::default_settings();
        grade.exposure = 0.4;
        grade.contrast = 12.0;
        grade.crop = Crop {
            x: 0.1,
            y: 0.2,
            w: 0.5,
            h: 0.6,
        };
        grade.spots = vec![Spot {
            x: 0.3,
            y: 0.4,
            sx: 0.5,
            sy: 0.6,
            r: 0.02,
            mode: SpotMode::Heal,
        }];

        write_edit(&photo, &grade).unwrap();
        let side = sidecar::read(&sidecar::path_for(&photo)).unwrap().unwrap();
        assert_eq!(side.settings, grade);
        assert_eq!(side.v, SIDECAR_VERSION);
        assert!(side.versions.is_empty());

        // A second write keeps the versions and the keys this build does not know, and lands
        // newer than the first.
        let mut value = side.to_value();
        value["futureKey"] = serde_json::json!({"a": 1});
        let text = serde_json::to_string_pretty(&value).unwrap();
        std::fs::write(sidecar::path_for(&photo), text).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut next = grade.clone();
        next.exposure = -0.25;
        write_edit(&photo, &next).unwrap();
        let after = sidecar::read(&sidecar::path_for(&photo)).unwrap().unwrap();
        assert_eq!(after.settings.exposure, -0.25);
        assert!(after.updated_at > side.updated_at);
        assert_eq!(after.extra.get("futureKey"), value.get("futureKey"));
        assert_eq!(
            after.settings.crop, grade.crop,
            "the photo's own crop stays its own"
        );
        assert_eq!(after.settings.spots, grade.spots);

        // A damaged edit file is never replaced.
        std::fs::write(sidecar::path_for(&photo), "{ not json").unwrap();
        let error = write_edit(&photo, &grade).unwrap_err();
        assert!(error.contains("could not be read"), "{error}");
        assert!(error.contains("left untouched"), "{error}");
        assert_eq!(
            std::fs::read_to_string(sidecar::path_for(&photo)).unwrap(),
            "{ not json"
        );

        // `Photo::read` refuses it too, so the whole photo is left out.
        assert!(Photo::read(photo.clone()).is_err());
    }

    #[test]
    fn copy_names_never_collide() {
        let out = Path::new("graded");
        let mut taken = HashMap::new();
        assert_eq!(
            copy_name(Path::new("a/b/day1.jpg"), out, &mut taken),
            out.join("day1.jpg")
        );
        assert_eq!(
            copy_name(Path::new("c/day1.png"), out, &mut taken),
            out.join("day1-2.jpg")
        );
        assert_eq!(
            copy_name(Path::new("d/day2.cr3"), out, &mut taken),
            out.join("day2.jpg")
        );
    }

    #[test]
    fn the_summary_names_what_moved() {
        let mut grade = EditSettings::default_settings();
        assert!(summary(&grade).contains("neutral"));
        grade.exposure = 0.35;
        grade.contrast = 12.0;
        grade.bw = true;
        let text = summary(&grade);
        assert!(text.contains("exposure +0.35 EV"), "{text}");
        assert!(text.contains("contrast +12"), "{text}");
        assert!(text.contains("black and white"), "{text}");

        let mut curve = EditSettings::default_settings();
        curve.curve.rgb = vec![[0.0, 0.0], [0.5, 0.62], [1.0, 1.0]];
        assert!(summary(&curve).contains("a curve"));

        let mut bands = EditSettings::default_settings();
        bands.hsl[lf_settings::HslBand::Blue].s = 20.0;
        bands.grading.midtones = lf_settings::GradeWheel {
            hue: 30.0,
            sat: 40.0,
        };
        bands.vignette = -12.0;
        let text = summary(&bands);
        assert!(text.contains("HSL bands"), "{text}");
        assert!(text.contains("colour grading"), "{text}");
        assert!(text.contains("vignette -12"), "{text}");
        assert_eq!(number(1.0), "+1");
        assert_eq!(number(-0.25), "-0.25");
        assert_eq!(number(0.0), "+0");
    }
}
