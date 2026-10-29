//! The colour readout under the histogram: the edited photo's red, green and blue (as
//! percentages, as Lightroom shows them) at the point under the pointer, while it is over the
//! preview. The values come from a small picture of the edit ([`EDGE`] pixels on the long
//! side) read back from the device on the histogram's schedule, only while the pointer is
//! over the photo, so a readout costs nothing otherwise.

use image::RgbaImage;

/// The readout picture's long edge.
pub const EDGE: u32 = 800;

/// The colour of `img` at `(u, v)` (fractions of the photo), or `None` outside it.
pub fn value_at(img: &RgbaImage, (u, v): (f64, f64)) -> Option<[u8; 3]> {
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) || img.width() == 0 {
        return None;
    }
    let x = ((u * f64::from(img.width())) as u32).min(img.width() - 1);
    let y = ((v * f64::from(img.height())) as u32).min(img.height() - 1);
    let p = img.get_pixel(x, y).0;
    Some([p[0], p[1], p[2]])
}

/// `R 52.2  G 48.0  B 40.0 %`.
pub fn text(rgb: [u8; 3]) -> String {
    let pct = |v: u8| f64::from(v) / 255.0 * 100.0;
    format!(
        "R {:.1}  G {:.1}  B {:.1} %",
        pct(rgb[0]),
        pct(rgb[1]),
        pct(rgb[2])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_value_under_a_point_reads_as_percentages() {
        let mut img = RgbaImage::from_pixel(10, 5, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(9, 4, image::Rgba([255, 128, 51, 255]));
        assert_eq!(value_at(&img, (1.0, 1.0)), Some([255, 128, 51]));
        assert_eq!(value_at(&img, (0.0, 0.0)), Some([0, 0, 0]));
        assert_eq!(value_at(&img, (1.2, 0.5)), None, "outside the photo");
        assert_eq!(text([255, 128, 51]), "R 100.0  G 50.2  B 20.0 %");
    }
}
