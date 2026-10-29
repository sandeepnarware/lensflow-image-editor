//! Every character beyond ASCII in the app's string literals can be drawn: egui panics on a
//! glyph none of its fonts has (Compare's hint once held "⇄" and panicked on opening). Comments
//! are skipped; a literal only used in logs is checked too, which costs nothing.

use eframe::egui;

/// The non-ASCII characters inside `"…"` on the lines of `source` that are not comments.
fn literal_chars(source: &str) -> Vec<char> {
    let mut out = Vec::new();
    for line in source.lines() {
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        let mut inside = false;
        let mut escaped = false;
        for c in line.chars() {
            if inside {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    inside = false;
                } else if !c.is_ascii() {
                    out.push(c);
                }
            } else if c == '"' {
                inside = true;
            }
        }
    }
    out
}

fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn every_character_in_the_apps_strings_has_a_glyph() {
    let ctx = egui::Context::default();
    // Fonts are loaded on the first frame.
    let mut frame = ctx.run_ui(egui::RawInput::default(), |_| {});
    frame.textures_delta.clear();
    let font = egui::FontId::proportional(14.0);
    let mut files = Vec::new();
    sources(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let mut missing = std::collections::BTreeMap::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for c in literal_chars(&text) {
            // Variation selectors and joiners shape emoji; they are not drawn alone.
            if matches!(c, '\u{fe0f}' | '\u{200d}' | '\u{feff}') {
                continue;
            }
            if !ctx.fonts_mut(|f| f.has_glyph(&font, c)) {
                missing
                    .entry(c)
                    .or_insert_with(|| file.display().to_string());
            }
        }
    }
    assert!(missing.is_empty(), "no glyph for {missing:?}");
}
