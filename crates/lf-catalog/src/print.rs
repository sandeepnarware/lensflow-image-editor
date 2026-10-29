//! Printing, as a PDF: the selected photos laid out on pages (one per page, or a grid for a
//! contact sheet), each fitted into its cell, with an optional caption under it. The studio
//! prints the PDF from any viewer (the app opens it), or keeps it to send to a lab.
//!
//! The photos are rendered as JPEGs by the export ([`crate::export::run`], sized for 300 dpi
//! in their cell) and placed in the PDF as they are (`DCTDecode`), so nothing is compressed
//! twice. Captions use the built-in Helvetica (no font is embedded); characters it cannot
//! show are written as `?`. The PDF writer is the small subset a printed page needs.
//!
//! A contact sheet (more than one photo a page) also carries a title at the top of each page,
//! when one is given, and "Page n of N" at the bottom ([`pdf_titled`]).

use std::fmt::Write as _;

/// Points per millimetre (a point is 1/72 inch).
const PT_PER_MM: f64 = 72.0 / 25.4;
/// The resolution photos are rendered at for their cell.
pub const DPI: f64 = 300.0;
/// The longest edge a photo is rendered at, whatever the paper (a poster still prints fine).
pub const MAX_EDGE: u32 = 6000;
/// The caption's size and the room kept under a photo for it, in points.
const CAPTION_PT: f64 = 9.0;
const CAPTION_ROOM: f64 = 14.0;

/// A paper size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paper {
    pub name: &'static str,
    /// Portrait width and height, in millimetres.
    pub mm: (f64, f64),
}

/// The papers offered, the most common first.
pub const PAPERS: [Paper; 7] = [
    Paper {
        name: "A4",
        mm: (210.0, 297.0),
    },
    Paper {
        name: "Letter",
        mm: (215.9, 279.4),
    },
    Paper {
        name: "A3",
        mm: (297.0, 420.0),
    },
    Paper {
        name: "4 × 6 in",
        mm: (101.6, 152.4),
    },
    Paper {
        name: "5 × 7 in",
        mm: (127.0, 177.8),
    },
    Paper {
        name: "8 × 10 in",
        mm: (203.2, 254.0),
    },
    Paper {
        name: "12 × 18 in",
        mm: (304.8, 457.2),
    },
];

/// Which way up a page is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    /// Each page follows its first photo (a landscape photo gets a landscape page).
    #[default]
    Auto,
    Portrait,
    Landscape,
}

/// What is written under each photo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Caption {
    #[default]
    None,
    FileName,
    /// The catalogue's title, else the file name.
    Title,
    /// The file name, the stars and the day taken (a contact sheet's).
    Details,
}

/// How the pages are laid out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrintSpec {
    pub paper: Paper,
    pub orientation: Orientation,
    /// Photos across and down each page.
    pub cols: u32,
    pub rows: u32,
    /// Around the page, and between photos, in millimetres.
    pub margin_mm: f64,
    pub gap_mm: f64,
    pub caption: Caption,
}

impl PrintSpec {
    pub fn per_page(&self) -> usize {
        (self.cols.max(1) * self.rows.max(1)) as usize
    }

    /// The page size in points for a page whose first photo is `landscape`.
    fn page_pt(&self, landscape: bool) -> (f64, f64) {
        let (w, h) = (self.paper.mm.0 * PT_PER_MM, self.paper.mm.1 * PT_PER_MM);
        let turn = match self.orientation {
            Orientation::Auto => landscape,
            Orientation::Portrait => false,
            Orientation::Landscape => true,
        };
        if turn { (h, w) } else { (w, h) }
    }

    /// Each cell's size in points on a page `page` big.
    fn cell_pt(&self, page: (f64, f64)) -> (f64, f64) {
        let m = self.margin_mm * PT_PER_MM;
        let g = self.gap_mm * PT_PER_MM;
        let (cols, rows) = (f64::from(self.cols.max(1)), f64::from(self.rows.max(1)));
        (
            ((page.0 - 2.0 * m - (cols - 1.0) * g) / cols).max(1.0),
            ((page.1 - 2.0 * m - (rows - 1.0) * g) / rows).max(1.0),
        )
    }

    /// The long edge, in pixels, to render photos at: 300 dpi in the larger way a cell can
    /// be turned, at most [`MAX_EDGE`].
    pub fn render_edge(&self) -> u32 {
        let page = self.page_pt(false);
        let cell = self.cell_pt(page).0.max(self.cell_pt(page).1);
        let cell_turned = {
            let p = self.page_pt(true);
            self.cell_pt(p).0.max(self.cell_pt(p).1)
        };
        let inches = cell.max(cell_turned) / 72.0;
        ((inches * DPI).round() as u32).clamp(64, MAX_EDGE)
    }
}

/// One photo to place: its JPEG bytes, pixel size and caption.
#[derive(Clone, Debug)]
pub struct Placed {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub caption: Option<String>,
}

/// `text` as a PDF string literal in WinAnsi (characters outside it become `?`).
fn pdf_text(text: &str) -> String {
    let mut out = String::from("(");
    for c in text.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if (' '..='~').contains(&c) => out.push(c),
            _ => out.push('?'),
        }
    }
    out.push(')');
    out
}

/// Helvetica's width of `text` at `size` (an average: close enough to centre a caption).
fn text_width(text: &str, size: f64) -> f64 {
    text.chars().count() as f64 * size * 0.5
}

/// The PDF for `photos` laid out by `spec`.
pub fn pdf(spec: &PrintSpec, photos: &[Placed]) -> Vec<u8> {
    pdf_titled(spec, photos, "")
}

/// The size of a contact sheet's title and page numbers, in points.
const HEADER_PT: f64 = 11.0;
const FOOTER_PT: f64 = 8.0;

/// [`pdf`] with `title` at the top of each contact-sheet page (none when empty) and page
/// numbers at the bottom (contact sheets only; a margin of under 8 mm leaves them out, as
/// there is no room).
pub fn pdf_titled(spec: &PrintSpec, photos: &[Placed], title: &str) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();
    // Object numbers: 1 catalogue, 2 page tree, 3 font, then per page: page, contents, and
    // one image per photo.
    let pages: Vec<&[Placed]> = photos.chunks(spec.per_page()).collect();
    let mut next = 4;
    let mut page_ids = Vec::new();
    let mut plan = Vec::new();
    for page in &pages {
        let page_id = next;
        let contents_id = next + 1;
        let image_ids: Vec<usize> = (0..page.len()).map(|i| next + 2 + i).collect();
        next += 2 + page.len();
        page_ids.push(page_id);
        plan.push((page_id, contents_id, image_ids));
    }
    let object = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, id: usize, body: &[u8]| {
        if offsets.len() < id {
            offsets.resize(id, 0);
        }
        offsets[id - 1] = out.len();
        out.extend(format!("{id} 0 obj\n").as_bytes());
        out.extend(body);
        out.extend(b"\nendobj\n");
    };
    out.extend(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");
    object(
        &mut out,
        &mut offsets,
        1,
        b"<< /Type /Catalog /Pages 2 0 R >>",
    );
    let kids: String = page_ids.iter().map(|id| format!("{id} 0 R ")).collect();
    object(
        &mut out,
        &mut offsets,
        2,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.trim_end(),
            page_ids.len()
        )
        .as_bytes(),
    );
    object(
        &mut out,
        &mut offsets,
        3,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    );
    let margin = spec.margin_mm * PT_PER_MM;
    let gap = spec.gap_mm * PT_PER_MM;
    let sheet = spec.per_page() > 1 && spec.margin_mm >= 8.0;
    let page_count = pages.len();
    for (n, (page, (page_id, contents_id, image_ids))) in pages.iter().zip(&plan).enumerate() {
        let landscape = page[0].width > page[0].height;
        let size = spec.page_pt(landscape);
        let cell = spec.cell_pt(size);
        let room = if spec.caption == Caption::None {
            0.0
        } else {
            CAPTION_ROOM
        };
        let mut stream = String::new();
        let mut xobjects = String::new();
        if sheet {
            let title = title.trim();
            if !title.is_empty() {
                let ty = size.1 - margin / 2.0 - HEADER_PT / 3.0;
                let _ = writeln!(
                    stream,
                    "BT /F1 {HEADER_PT} Tf {margin:.3} {ty:.3} Td {} Tj ET",
                    pdf_text(title)
                );
            }
            let footer = format!("Page {} of {page_count}", n + 1);
            let fx = size.0 - margin - text_width(&footer, FOOTER_PT);
            let fy = margin / 2.0 - FOOTER_PT / 3.0;
            let _ = writeln!(
                stream,
                "BT /F1 {FOOTER_PT} Tf {fx:.3} {fy:.3} Td {} Tj ET",
                pdf_text(&footer)
            );
        }
        for (i, (photo, image_id)) in page.iter().zip(image_ids).enumerate() {
            let (col, row) = (i as u32 % spec.cols.max(1), i as u32 / spec.cols.max(1));
            // PDF's origin is the bottom left: rows count down from the top.
            let x0 = margin + f64::from(col) * (cell.0 + gap);
            let top = size.1 - margin - f64::from(row) * (cell.1 + gap);
            let (avail_w, avail_h) = (cell.0, (cell.1 - room).max(1.0));
            let scale = (avail_w / f64::from(photo.width.max(1)))
                .min(avail_h / f64::from(photo.height.max(1)));
            let (w, h) = (
                f64::from(photo.width) * scale,
                f64::from(photo.height) * scale,
            );
            let x = x0 + (avail_w - w) / 2.0;
            let y = top - (avail_h - h) / 2.0 - h;
            let name = format!("Im{}", i + 1);
            let _ = writeln!(stream, "q {w:.3} 0 0 {h:.3} {x:.3} {y:.3} cm /{name} Do Q");
            let _ = write!(xobjects, "/{name} {image_id} 0 R ");
            if let Some(caption) = photo.caption.as_deref().filter(|_| room > 0.0) {
                let tx = x0 + (cell.0 - text_width(caption, CAPTION_PT)).max(0.0) / 2.0;
                let ty = y - CAPTION_PT - 3.0;
                let _ = writeln!(
                    stream,
                    "BT /F1 {CAPTION_PT} Tf {tx:.3} {ty:.3} Td {} Tj ET",
                    pdf_text(caption)
                );
            }
        }
        object(
            &mut out,
            &mut offsets,
            *page_id,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.3} {:.3}] \
                 /Resources << /Font << /F1 3 0 R >> /XObject << {}>> >> /Contents {} 0 R >>",
                size.0, size.1, xobjects, contents_id
            )
            .as_bytes(),
        );
        let mut body = format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes();
        body.extend(stream.as_bytes());
        body.extend(b"\nendstream");
        object(&mut out, &mut offsets, *contents_id, &body);
        for (photo, image_id) in page.iter().zip(image_ids) {
            let mut body = format!(
                "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB \
                 /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
                photo.width,
                photo.height,
                photo.jpeg.len()
            )
            .into_bytes();
            body.extend(&photo.jpeg);
            body.extend(b"\nendstream");
            object(&mut out, &mut offsets, *image_id, &body);
        }
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for at in &offsets {
        out.extend(format!("{at:010} 00000 n \n").as_bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(w: u32, h: u32) -> Placed {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode_image(&image::RgbImage::new(w, h))
            .unwrap();
        Placed {
            jpeg: bytes,
            width: w,
            height: h,
            caption: Some("IMG_0001 (bride).jpg — été".into()),
        }
    }

    fn spec(cols: u32, rows: u32) -> PrintSpec {
        PrintSpec {
            paper: PAPERS[0],
            orientation: Orientation::Auto,
            cols,
            rows,
            margin_mm: 10.0,
            gap_mm: 5.0,
            caption: Caption::FileName,
        }
    }

    #[test]
    fn a_contact_sheet_has_a_page_per_grid_and_every_photo_once() {
        let photos: Vec<Placed> = (0..5).map(|_| jpeg(30, 20)).collect();
        let doc = pdf(&spec(2, 2), &photos);
        let text = String::from_utf8_lossy(&doc);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.ends_with("%%EOF\n"));
        assert!(text.contains("/Count 2"), "5 photos at 4 a page: 2 pages");
        assert_eq!(text.matches("/Subtype /Image").count(), 5);
        assert_eq!(text.matches(" Do Q").count(), 5);
        // The caption is escaped, and what Helvetica lacks is a question mark.
        assert!(text.contains("(IMG_0001 \\(bride\\).jpg ? ?t?) Tj"));
        // The cross-reference table points at each object.
        let xref = text.find("\nxref\n").unwrap() + 1;
        let first = text[xref..].lines().nth(3).unwrap();
        let at: usize = first[..10].parse().unwrap();
        assert!(doc[at..].starts_with(b"1 0 obj"));
    }

    #[test]
    fn a_contact_sheet_carries_its_title_and_page_numbers() {
        let photos: Vec<Placed> = (0..5).map(|_| jpeg(30, 20)).collect();
        let text =
            String::from_utf8_lossy(&pdf_titled(&spec(2, 2), &photos, "Ana & Raj")).into_owned();
        assert_eq!(text.matches("(Ana & Raj) Tj").count(), 2, "on both pages");
        assert!(text.contains("(Page 1 of 2) Tj") && text.contains("(Page 2 of 2) Tj"));
        // One photo a page is not a contact sheet: nothing added.
        let one =
            String::from_utf8_lossy(&pdf_titled(&spec(1, 1), &photos[..1], "Ana")).into_owned();
        assert!(!one.contains("(Ana) Tj") && !one.contains("Page 1"));
    }

    #[test]
    fn a_landscape_photo_gets_a_landscape_page_when_auto() {
        let doc = pdf(&spec(1, 1), &[jpeg(30, 20)]);
        let text = String::from_utf8_lossy(&doc);
        // A4 turned: 841.890 wide, 595.276 high.
        assert!(text.contains("/MediaBox [0 0 841.890 595.276]"), "{text}");
        let portrait = PrintSpec {
            orientation: Orientation::Portrait,
            ..spec(1, 1)
        };
        let text = String::from_utf8_lossy(&pdf(&portrait, &[jpeg(30, 20)])).into_owned();
        assert!(text.contains("/MediaBox [0 0 595.276 841.890]"));
    }

    #[test]
    fn photos_are_rendered_for_300_dpi_in_their_cell() {
        // One photo on A4 with 10 mm margins: the cell's long side is 277 mm, 10.9 in.
        let edge = spec(1, 1).render_edge();
        assert!((3200..=3300).contains(&edge), "{edge}");
        assert!(spec(3, 3).render_edge() < edge);
    }
}
