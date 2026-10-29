//! The sidebar's Keywords list (as Lightroom's Keyword List): every keyword of the folder's
//! photos with how many have it, as a tree when keywords are written as paths
//! ("Places > India > Goa" sits under "Places" and "India"). A click shows only the photos
//! with that keyword or one under it (`Filter::keyword`); a second click shows all again.

use super::Library;
use crate::theme;
use eframe::egui::{self, RichText};
use std::collections::BTreeMap;

pub const KEYWORDS: &str = "Keywords";
pub const NONE: &str = "No keywords in this folder.";
/// The separator of a keyword path.
pub const SEP: &str = ">";

/// A keyword's path, each part trimmed ("Places > India" → ["Places", "India"]).
pub fn path(keyword: &str) -> Vec<String> {
    keyword
        .split(SEP)
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect()
}

/// A path written back as a keyword ("Places > India").
pub fn joined(parts: &[String]) -> String {
    parts.join(&format!(" {SEP} "))
}

/// Whether a photo with `keywords` is under `chosen` (that keyword or one below it; case
/// ignored).
pub fn under(keywords: &[String], chosen: &str) -> bool {
    let want: Vec<String> = path(chosen).iter().map(|p| p.to_lowercase()).collect();
    keywords.iter().any(|k| {
        let have: Vec<String> = path(k).iter().map(|p| p.to_lowercase()).collect();
        have.len() >= want.len() && have[..want.len()] == want[..]
    })
}

/// Every node of the tree (each keyword and each of its parents, by path), with how many of
/// the photos (`keywords` per photo) are under it, in path order.
pub fn tree<'a>(photos: impl Iterator<Item = &'a [String]>) -> Vec<(Vec<String>, usize)> {
    let mut counts: BTreeMap<Vec<String>, usize> = BTreeMap::new();
    for keywords in photos {
        let mut nodes: Vec<Vec<String>> = Vec::new();
        for k in keywords {
            let p = path(k);
            for n in 1..=p.len() {
                nodes.push(p[..n].to_vec());
            }
        }
        nodes.sort();
        nodes.dedup();
        for n in nodes {
            *counts.entry(n).or_default() += 1;
        }
    }
    counts.into_iter().collect()
}

impl Library {
    pub(super) fn keywords_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(RichText::new(KEYWORDS).strong());
        ui.separator();
        let nodes = tree(self.photos.iter().map(|p| p.meta.keywords.as_slice()));
        if nodes.is_empty() {
            ui.label(RichText::new(NONE).color(theme::MUTED).small());
            return;
        }
        let chosen = self.filter.keyword.clone();
        let mut click = None;
        for (p, n) in &nodes {
            let name = joined(p);
            let here = chosen.as_deref() == Some(name.as_str());
            ui.horizontal(|ui| {
                ui.add_space(12.0 * (p.len() - 1) as f32);
                let label = format!("{}  {n}", p.last().map(String::as_str).unwrap_or(""));
                let r = ui.selectable_label(here, label);
                let full = name.clone();
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Keyword {full}"))
                });
                if r.clicked() {
                    click = Some(name.clone());
                }
            });
        }
        if let Some(name) = click {
            let mut f = self.filter.clone();
            f.keyword = (f.keyword.as_deref() != Some(name.as_str())).then_some(name);
            self.set_filter(f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_paths_make_a_tree_with_counts() {
        let a = vec!["Places > India > Goa".to_owned(), "beach".to_owned()];
        let b = vec!["Places>India>Delhi".to_owned()];
        let c = vec!["beach".to_owned()];
        let t = tree([a.as_slice(), b.as_slice(), c.as_slice()].into_iter());
        let get = |p: &[&str]| {
            t.iter()
                .find(|(n, _)| n.iter().map(String::as_str).eq(p.iter().copied()))
                .map(|(_, c)| *c)
        };
        assert_eq!(get(&["Places"]), Some(2));
        assert_eq!(get(&["Places", "India"]), Some(2));
        assert_eq!(get(&["Places", "India", "Goa"]), Some(1));
        assert_eq!(get(&["beach"]), Some(2));
        assert!(under(&a, "Places > India"));
        assert!(under(&a, "places"));
        assert!(!under(&c, "Places"));
        assert!(!under(&b, "Places > India > Goa"));
    }
}
