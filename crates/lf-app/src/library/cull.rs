//! Culling in the library: which of a folder's photos the grid shows and in what order (the
//! filter bar), and what a flag, star or label key does to the photos it is pressed on.
//!
//! Pure functions of the folder's rows, so the grid, the filmstrip and the arrow keys all
//! follow the same list. The filter is the window's, not the folder's, and is not saved: a
//! filter left on from last week would look like lost photos. The status bar says
//! "12 of 340 photos" while one is on.

use lf_catalog::PhotoRow;
use lf_catalog::cull::{Flag, Label, Mark};
use lf_catalog::scores::Scores;

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

/// What assisted culling's scores must say ([`lf_catalog::scores`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReviewFilter {
    #[default]
    Any,
    /// Blurry or eyes closed.
    Issues,
    Blurry,
    EyesClosed,
    /// Scored, and nothing wrong.
    Clean,
    /// Not scored yet (or the file changed since).
    Unscored,
}

impl ReviewFilter {
    pub const ALL: [ReviewFilter; 6] = [
        ReviewFilter::Any,
        ReviewFilter::Issues,
        ReviewFilter::Blurry,
        ReviewFilter::EyesClosed,
        ReviewFilter::Clean,
        ReviewFilter::Unscored,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ReviewFilter::Any => "Any score",
            ReviewFilter::Issues => "Possible rejects",
            ReviewFilter::Blurry => "Blurry",
            ReviewFilter::EyesClosed => "Eyes closed",
            ReviewFilter::Clean => "Sharp, eyes open",
            ReviewFilter::Unscored => "Not scored",
        }
    }

    fn keeps(self, scores: Option<&Scores>) -> bool {
        match (self, scores) {
            (ReviewFilter::Any, _) => true,
            (ReviewFilter::Unscored, s) => s.is_none(),
            (_, None) => false,
            (ReviewFilter::Issues, Some(s)) => s.issue(),
            (ReviewFilter::Blurry, Some(s)) => s.blurry(),
            (ReviewFilter::EyesClosed, Some(s)) => s.eyes_closed(),
            (ReviewFilter::Clean, Some(s)) => !s.issue(),
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
    /// Sharpest first (assisted culling's score); unscored photos last, in capture order.
    Sharpness,
}

impl Sort {
    pub const ALL: [Sort; 4] = [
        Sort::CaptureTime,
        Sort::FileName,
        Sort::Rating,
        Sort::Sharpness,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Sort::CaptureTime => "Capture time",
            Sort::FileName => "File name",
            Sort::Rating => "Rating",
            Sort::Sharpness => "Sharpness",
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
    /// Assisted culling's verdicts.
    pub review: ReviewFilter,
    pub sort: Sort,
    /// Words that must each appear in the file name, the camera or lens, or the title,
    /// caption, creator or a keyword (case ignored).
    pub text: String,
    /// Only photos from this camera, or through this lens (as the EXIF names them).
    pub camera: Option<String>,
    pub lens: Option<String>,
    /// Only photos with this keyword or one under it (the sidebar's Keywords list).
    pub keyword: Option<String>,
    /// Only these photos (Find similar's answer).
    pub only: Option<Vec<i64>>,
}

impl Filter {
    /// Some photos may be hidden (the sort alone hides none).
    pub fn narrows(&self) -> bool {
        self.flag != FlagFilter::All
            || self.min_rating > 0
            || self.label != LabelFilter::Any
            || self.review != ReviewFilter::Any
            || !self.text.trim().is_empty()
            || self.camera.is_some()
            || self.lens.is_some()
            || self.keyword.is_some()
            || self.only.is_some()
    }

    fn keeps(&self, row: &PhotoRow) -> bool {
        self.flag.keeps(row.flag)
            && row.rating >= self.min_rating
            && self.label.keeps(row.label)
            && self.review.keeps(row.scores.as_ref())
            && matches_text(row, &self.text)
            && (self.camera.is_none() || row.camera == self.camera)
            && (self.lens.is_none() || row.lens == self.lens)
            && self
                .keyword
                .as_ref()
                .is_none_or(|k| crate::library::keywords_ui::under(&row.meta.keywords, k))
            && self.only.as_ref().is_none_or(|ids| ids.contains(&row.id))
    }

    /// The ids the grid shows, in its order. `rows` come in capture order. A collapsed
    /// stack shows only its top (or, when the filter hides the top, its first photo shown).
    pub fn arrange(&self, rows: &[PhotoRow]) -> Vec<i64> {
        let mut kept: Vec<&PhotoRow> = rows.iter().filter(|r| self.keeps(r)).collect();
        let tops: std::collections::HashSet<i64> = kept
            .iter()
            .filter_map(|r| r.stack.filter(|s| s.top == r.id).map(|s| s.id))
            .collect();
        let mut seen = std::collections::HashSet::new();
        kept.retain(|r| match r.stack {
            Some(s) if s.collapsed => {
                if tops.contains(&s.id) {
                    r.id == s.top
                } else {
                    seen.insert(s.id)
                }
            }
            _ => true,
        });
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
            // Stable, scored first: equal (and unscored) photos stay in capture order.
            Sort::Sharpness => kept.sort_by(|a, b| {
                let key = |r: &PhotoRow| r.scores.map(|s| s.sharpness);
                match (key(a), key(b)) {
                    (Some(x), Some(y)) => y.total_cmp(&x),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            }),
        }
        kept.into_iter().map(|r| r.id).collect()
    }
}

/// Whether every word of `text` is somewhere in `row` ([`lf_catalog::meta::row_matches`]).
pub fn matches_text(row: &PhotoRow, text: &str) -> bool {
    lf_catalog::meta::row_matches(row, text)
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

    #[test]
    fn the_camera_and_lens_filters_keep_their_own() {
        let mut a = row(1, "a.nef", Flag::Unflagged, 0, None);
        a.camera = Some("Nikon Z 8".into());
        a.lens = Some("85mm".into());
        let mut b = row(2, "b.cr3", Flag::Unflagged, 0, None);
        b.camera = Some("Canon R5".into());
        b.lens = Some("85mm".into());
        let rows = vec![a, b];
        let nikon = Filter {
            camera: Some("Nikon Z 8".into()),
            ..Filter::default()
        };
        assert!(nikon.narrows());
        assert_eq!(nikon.arrange(&rows), vec![1]);
        let lens = Filter {
            lens: Some("85mm".into()),
            ..Filter::default()
        };
        assert_eq!(lens.arrange(&rows), vec![1, 2]);
        assert_eq!(
            crate::library::cull_ui::distinct(rows.iter().map(|r| r.camera.as_ref())),
            vec!["Canon R5".to_owned(), "Nikon Z 8".to_owned()]
        );
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
    fn culling_scores_filter_and_sort() {
        let mut rows = rows();
        let scored = |sharpness, eyes_closed| {
            Some(Scores {
                sharpness,
                faces: 1,
                eyes_closed,
                eye_sharpness: None,
            })
        };
        rows[0].scores = scored(0.9, 0);
        rows[1].scores = scored(0.1, 0);
        rows[2].scores = scored(0.7, 1);
        let by = |review| {
            Filter {
                review,
                ..Filter::default()
            }
            .arrange(&rows)
        };
        assert_eq!(by(ReviewFilter::Issues), [2, 3]);
        assert_eq!(by(ReviewFilter::Blurry), [2]);
        assert_eq!(by(ReviewFilter::EyesClosed), [3]);
        assert_eq!(by(ReviewFilter::Clean), [1]);
        assert_eq!(by(ReviewFilter::Unscored), [4]);
        assert!(
            Filter {
                review: ReviewFilter::Issues,
                ..Filter::default()
            }
            .narrows()
        );
        // Sharpest first, the unscored photo last.
        let sorted = Filter {
            sort: Sort::Sharpness,
            ..Filter::default()
        }
        .arrange(&rows);
        assert_eq!(sorted, [1, 3, 2, 4]);
    }

    #[test]
    fn a_collapsed_stack_shows_its_top() {
        use lf_catalog::stacks::Stack;
        let mut rows = rows();
        let stack = |collapsed| {
            Some(Stack {
                id: 7,
                top: 3,
                size: 3,
                collapsed,
            })
        };
        for r in &mut rows[1..] {
            r.stack = stack(true);
        }
        assert_eq!(Filter::default().arrange(&rows), [1, 3]);
        // Open: all of them.
        for r in &mut rows[1..] {
            r.stack = stack(false);
        }
        assert_eq!(Filter::default().arrange(&rows), [1, 2, 3, 4]);
        // The filter hides the top: the first photo shown stands for the stack.
        for r in &mut rows[1..] {
            r.stack = stack(true);
        }
        let not_five = Filter {
            min_rating: 0,
            flag: FlagFilter::NotRejected,
            ..Filter::default()
        };
        assert_eq!(not_five.arrange(&rows), [1, 3]);
        let rejected = Filter {
            flag: FlagFilter::Rejected,
            ..Filter::default()
        };
        assert_eq!(rejected.arrange(&rows), [2]);
        let three_stars = Filter {
            min_rating: 3,
            label: LabelFilter::None,
            ..Filter::default()
        };
        assert_eq!(three_stars.arrange(&rows), [4]);
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
