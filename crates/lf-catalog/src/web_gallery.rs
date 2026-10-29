//! Web gallery (Lightroom's Web module, made simple): a folder that is a whole website, ready to
//! upload anywhere or open from disk: `index.html` (a grid of thumbnails; a click shows the
//! photo large, the arrow keys step, Esc closes), `images/` (the photos as exported) and
//! `thumbs/` (small copies). No script or style is fetched from elsewhere; the page works
//! offline.

use std::io;
use std::path::{Path, PathBuf};

/// The thumbnails' long edge, in pixels.
pub const THUMB_EDGE: u32 = 480;

/// One photo on the page: its exported file (under `images/`) and its caption.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub image: PathBuf,
    pub caption: String,
}

/// `text` safe inside HTML text and attributes.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A file name as a URL path segment (spaces and the like percent-encoded).
fn url_name(name: &str) -> String {
    let mut out = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The page for `title` showing `entries` (file names under `images/` and `thumbs/`).
pub fn page(title: &str, entries: &[(String, String)]) -> String {
    let t = escape(title);
    let mut cells = String::new();
    for (i, (name, caption)) in entries.iter().enumerate() {
        let n = url_name(name);
        let c = escape(caption);
        cells.push_str(&format!(
            "<a href=\"images/{n}\" data-i=\"{i}\" title=\"{c}\"><img src=\"thumbs/{n}\" alt=\"{c}\" loading=\"lazy\"></a>\n"
        ));
    }
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{t}</title>
<style>
body {{ margin: 0; background: #111; color: #eee; font: 16px/1.4 system-ui, sans-serif; }}
h1 {{ font-weight: 400; margin: 24px 16px; }}
.grid {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 8px; padding: 0 16px 24px; }}
.grid img {{ width: 100%; aspect-ratio: 3 / 2; object-fit: cover; display: block; background: #222; }}
#view {{ position: fixed; inset: 0; background: rgba(0,0,0,.94); display: none; align-items: center; justify-content: center; flex-direction: column; }}
#view.on {{ display: flex; }}
#view img {{ max-width: 96vw; max-height: 88vh; }}
#view p {{ margin: 12px; color: #bbb; }}
</style>
</head>
<body>
<h1>{t}</h1>
<div class="grid">
{cells}</div>
<div id="view"><img alt=""><p></p></div>
<script>
const links = [...document.querySelectorAll('.grid a')];
const view = document.getElementById('view');
const img = view.querySelector('img');
const cap = view.querySelector('p');
let at = -1;
function show(i) {{
  at = (i + links.length) % links.length;
  img.src = links[at].href;
  cap.textContent = links[at].title;
  view.classList.add('on');
}}
links.forEach((a, i) => a.addEventListener('click', e => {{ e.preventDefault(); show(i); }}));
view.addEventListener('click', () => view.classList.remove('on'));
document.addEventListener('keydown', e => {{
  if (!view.classList.contains('on')) return;
  if (e.key === 'Escape') view.classList.remove('on');
  if (e.key === 'ArrowRight') show(at + 1);
  if (e.key === 'ArrowLeft') show(at - 1);
}});
</script>
</body>
</html>
"#
    )
}

/// Makes the site in `folder` from `entries` (already exported into `folder/images`): the
/// thumbnails and `index.html`. Returns the page's path.
pub fn write(folder: &Path, title: &str, entries: &[Entry]) -> io::Result<PathBuf> {
    let thumbs = folder.join("thumbs");
    std::fs::create_dir_all(&thumbs)?;
    let mut listed = Vec::new();
    for e in entries {
        let Some(name) = e
            .image
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
        else {
            continue;
        };
        let img = image::open(&e.image).map_err(io::Error::other)?;
        let small = img.thumbnail(THUMB_EDGE, THUMB_EDGE).to_rgb8();
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 82)
            .encode_image(&small)
            .map_err(io::Error::other)?;
        std::fs::write(thumbs.join(&name), bytes)?;
        listed.push((name, e.caption.clone()));
    }
    let index = folder.join("index.html");
    std::fs::write(&index, page(title, &listed))?;
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_site_holds_its_page_thumbnails_and_escaped_captions() {
        let dir = tempfile::tempdir().unwrap();
        let images = dir.path().join("images");
        std::fs::create_dir_all(&images).unwrap();
        let big = images.join("Haldi 1.jpg");
        image::RgbImage::from_pixel(1200, 800, image::Rgb([200, 120, 40]))
            .save(&big)
            .unwrap();
        let index = write(
            dir.path(),
            "Asha & Raj",
            &[Entry {
                image: big,
                caption: "<Haldi>".into(),
            }],
        )
        .unwrap();
        let html = std::fs::read_to_string(index).unwrap();
        assert!(html.contains("<title>Asha &amp; Raj</title>"));
        assert!(html.contains("images/Haldi%201.jpg"));
        assert!(html.contains("&lt;Haldi&gt;") && !html.contains("<Haldi>"));
        let thumb = image::open(dir.path().join("thumbs/Haldi 1.jpg")).unwrap();
        assert_eq!(thumb.width(), THUMB_EDGE);
    }
}
