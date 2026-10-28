//! Culling in the library: which of a folder's photos the grid shows and in what order (the
//! filter bar), and what a flag, star or label key does to the photos it is pressed on.
//!
//! Pure functions of the folder's rows, so the grid, the filmstrip and the arrow keys all
//! follow the same list. The filter is the window's, not the folder's, and is not saved: a
//! filter left on from last week would look like lost photos. The status bar says
//! "12 of 340 photos" while one is on.

use lf_catalog::PhotoRow;
use lf_catalog::cull::{Flag, Label, Mark};

/// Which flags the grid shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlagFilter {
    #[default]
    All,
    /// Picked and unflagged: everything but the rejects.
    NotRejected,
    Picked,
    Unflagged,
    Rejected,
}

impl FlagFilter {
    pub const ALL: [FlagFilter; 5] = [
        FlagFilter::All,
        FlagFilter::NotRejected,
        FlagFilter::Picked,
        FlagFilter::Unflagged,
        FlagFilter::Rejected,
    ];

    pub fn name(self) -> &'static str {
        match self {
            FlagFilter::All => "Any flag",
            FlagFilter::NotRejected => "Hide rejected",
            FlagFilter::Picked => "Picked",
            FlagFilter::Unflagged => "Unflagged",
            FlagFilter::Rejected => "Rejected",
        }
    }

    fn keeps(self, flag: Flag) -> bool {
        match self {
            FlagFilter::All => true,
            FlagFilter::NotRejected => flag != Flag::Rejected,
            FlagFilter::Picked => flag == Flag::Picked,
            FlagFilter::Unflagged => flag == Flag::Unflagged,
            FlagFilter::Rejected => flag == Flag::Rejected,
        }
    }
}

/// Which colour labels the grid shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LabelFilter {
    #[default]
    Any,
    /// Photos with no label (or one this version cannot read).
    None,
    Only(Label),
}

impl LabelFilter {
    /// Any, no label, then each label.
    pub fn all() -> impl Iterator<Item = LabelFilter> {
        [LabelFilter::Any, LabelFilter::None]
            .into_iter()
            .chain(Label::ALL.into_iter().map(LabelFilter::Only))
    }

    pub fn name(self) -> &'static str {
        match self {
            LabelFilter::Any => "Any label",
            LabelFilter::None => "No label",
            LabelFilter::Only(label) => label.name(),
        }
    }

    fn keeps(self, label: Option<Label>) -> bool {
        match self {
            LabelFilter::Any => true,
            LabelFilter::None => label.is_none(),
            LabelFilter::Only(want) => label == Some(want),
        }
    }
}

/// The grid's order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    /// Capture time, then path; undated photos last (the catalogue's own order).
    #[default]
    CaptureTime,
    FileName,
    /// Most stars first; equal ratings keep capture order.
    Rating,
}

impl Sort {
    pub const ALL: [Sort; 3] = [Sort::CaptureTime, Sort::FileName, Sort::Rating];

    pub fn name(self) -> &'static str {
        match self {
            Sort::CaptureTime => "Capture time",
            Sort::FileName => "File name",
            Sort::Rating => "Rating",
        }
    }
}

/// What the filter bar asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub flag: FlagFilter,
    /// At least this many stars (0: any).
    pub min_rating: u8,
    pub label: LabelFilter,
    pub sort: Sort,
    /// Words that must each appear in the file name, the camera or lens, or the title,
    /// caption, creator or a keyword (case ignored).
    pub text: String,
}

impl Filter {
    /// Some photos may be hidden (the sort alone hides none).
    pub fn narrows(&self) -> bool {
        self.flag != FlagFilter::All
            || self.min_rating > 0
            || self.label != LabelFilter::Any
            || !self.text.trim().is_empty()
    }

    fn keeps(&self, row: &PhotoRow) -> bool {
        self.flag.keeps(row.flag)
            && row.rating >= self.min_rating
            && self.label.keeps(row.label)
            && matches_text(row, &self.text)
    }

    /// The ids the grid shows, in its order. `rows` come in capture order.
    pub fn arrange(&self, rows: &[PhotoRow]) -> Vec<i64> {
        let mut kept: Vec<&PhotoRow> = rows.iter().filter(|r| self.keeps(r)).collect();
        match self.sort {
            Sort::CaptureTime => {}
            Sort::FileName => kept.sort_by_cached_key(|r| {
                r.path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default()
            }),
            // Stable: equal ratings stay in capture order.
            Sort::Rating => kept.sort_by_key(|r| std::cmp::Reverse(r.rating)),
        }
        kept.into_iter().map(|r| r.id).collect()
    }
}

/// Whether every word of `text` is somewhere in `row`: its file name, camera, lens, or its
/// title, caption, creator and keywords. No words keeps every row.
pub fn matches_text(row: &PhotoRow, text: &str) -> bool {
    if text.trim().is_empty() {
        return true;
    }
    let m = &row.meta;
    let name = row
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    let fields: Vec<String> = [
        &name,
        &row.camera,
        &row.lens,
        &m.title,
        &m.caption,
        &m.creator,
    ]
    .into_iter()
    .flatten()
    .chain(&m.keywords)
    .map(|s| s.to_lowercase())
    .collect();
    text.split_whitespace()
        .map(str::to_lowercase)
        .all(|word| fields.iter().any(|f| f.contains(&word)))
}

/// The mark a key or menu item sets on `targets`. A colour label already on every target is
/// taken off instead (Lightroom's toggle); flags and stars are always set as asked.
pub fn resolve(asked: Mark, targets: &[&PhotoRow]) -> Mark {
    match asked {
        Mark::Label(Some(label))
            if !targets.is_empty() && targets.iter().all(|r| r.label == Some(label)) =>
        {
            Mark::Label(None)
        }
        other => other,
    }
}

/// Writes `mark` into the rows held in memory, so the grid shows it before the catalogue
/// answers.
pub fn apply(row: &mut PhotoRow, mark: Mark) {
    match mark {
        Mark::Flag(flag) => row.flag = flag,
        Mark::Rating(stars) => row.rating = stars,
        Mark::Label(label) => row.label = label,
    }
}

/// "12 of 340 photos" while a filter hides some; otherwise the plain count.
pub fn shown_text(shown: usize, total: usize) -> Option<String> {
    (shown != total).then(|| {
        let noun = if total == 1 { "photo" } else { "photos" };
        format!(
            "{} of {} {noun}",
            crate::catalog_ext::count(shown),
            crate::catalog_ext::count(total)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn row(id: i64, name: &str, flag: Flag, rating: u8, label: Option<Label>) -> PhotoRow {
        PhotoRow {
            id,
            path: PathBuf::from(format!("/shoot/{name}")),
            flag,
            rating,
            label,
            ..PhotoRow::default()
        }
    }

    fn rows() -> Vec<PhotoRow> {
        vec![
            row(1, "c.jpg", Flag::Picked, 3, Some(Label::Red)),
            row(2, "A.jpg", Flag::Rejected, 0, None),
            row(3, "b.jpg", Flag::Unflagged, 5, Some(Label::Green)),
            row(4, "d.jpg", Flag::Unflagged, 3, None),
        ]
    }

    #[test]
    fn no_filter_keeps_capture_order() {
        let f = Filter::default();
        assert!(!f.narrows());
        assert_eq!(f.arrange(&rows()), [1, 2, 3, 4]);
    }

    #[test]
    fn flags_filter() {
        let ids = |flag| {
            Filter {
                flag,
                ..Filter::default()
            }
            .arrange(&rows())
        };
        assert_eq!(ids(FlagFilter::NotRejected), [1, 3, 4]);
        assert_eq!(ids(FlagFilter::Picked), [1]);
        assert_eq!(ids(FlagFilter::Unflagged), [3, 4]);
        assert_eq!(ids(FlagFilter::Rejected), [2]);
    }

    #[test]
    fn stars_and_labels_filter_together() {
        let f = Filter {
            min_rating: 3,
            ..Filter::default()
        };
        assert!(f.narrows());
        assert_eq!(f.arrange(&rows()), [1, 3, 4]);
        let f = Filter {
            min_rating: 3,
            label: LabelFilter::None,
            ..Filter::default()
        };
        assert_eq!(f.arrange(&rows()), [4]);
        let f = Filter {
            label: LabelFilter::Only(Label::Green),
            ..Filter::default()
        };
        assert_eq!(f.arrange(&rows()), [3]);
    }

    #[test]
    fn sorts() {
        let by = |sort| {
            Filter {
                sort,
                ..Filter::default()
            }
            .arrange(&rows())
        };
        // File names ignore case.
        assert_eq!(by(Sort::FileName), [2, 3, 1, 4]);
        // Most stars first; the two 3-star photos stay in capture order.
        assert_eq!(by(Sort::Rating), [3, 1, 4, 2]);
        assert!(
            !Filter {
                sort: Sort::Rating,
                ..Filter::default()
            }
            .narrows(),
            "a sort hides nothing"
        );
    }

    #[test]
    fn a_label_on_every_target_toggles_off() {
        let all = rows();
        let red = &all[0];
        let none = &all[1];
        let asked = Mark::Label(Some(Label::Red));
        assert_eq!(resolve(asked, &[red]), Mark::Label(None));
        assert_eq!(resolve(asked, &[red, none]), asked, "not all red: set red");
        assert_eq!(resolve(asked, &[]), asked);
        // Stars and flags are set, never toggled.
        let picked = Mark::Flag(Flag::Picked);
        assert_eq!(resolve(picked, &[red]), picked);
        assert_eq!(resolve(Mark::Rating(3), &[red]), Mark::Rating(3));
    }

    #[test]
    fn apply_changes_only_its_own_mark() {
        let mut r = rows().remove(0);
        apply(&mut r, Mark::Rating(1));
        apply(&mut r, Mark::Flag(Flag::Rejected));
        assert_eq!(
            (r.flag, r.rating, r.label),
            (Flag::Rejected, 1, Some(Label::Red))
        );
        apply(&mut r, Mark::Label(None));
        assert_eq!(r.label, None);
    }

    #[test]
    fn search_words_must_each_match_somewhere() {
        let mut r = row(1, "IMG_0042.jpg", Flag::Unflagged, 0, None);
        r.camera = Some("Nikon Z6".into());
        r.meta.title = Some("Sangeet".into());
        r.meta.keywords = vec!["Udaipur".into()];
        assert!(matches_text(&r, ""));
        assert!(matches_text(&r, "img_0042 nikon"));
        assert!(matches_text(&r, "SANGEET udaipur"));
        assert!(!matches_text(&r, "sangeet goa"));
        let f = Filter {
            text: " udaipur ".into(),
            ..Filter::default()
        };
        assert!(f.narrows());
        assert_eq!(f.arrange(&[r]), [1]);
    }

    #[test]
    fn the_status_bar_says_how_many_are_shown() {
        assert_eq!(shown_text(12, 340).as_deref(), Some("12 of 340 photos"));
        assert_eq!(shown_text(1_200, 1_200), None);
        assert_eq!(shown_text(0, 1).as_deref(), Some("0 of 1 photo"));
    }
}
