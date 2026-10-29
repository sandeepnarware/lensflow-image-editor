//! Geotagging from a GPX track (Lightroom's Map → Tracklog): a phone or GPS logger records
//! where it was over time; each photo is placed where the track was when it was taken.
//!
//! A camera's clock keeps local time with no zone, so the photo's time is read as local and
//! moved by the offset the user gives (the camera's zone, e.g. +05:30) to UTC, as GPX times
//! are. Between two track points no more than [`MAX_GAP`] apart the place is interpolated; a
//! photo up to [`REACH`] before the first or after the last point (or beside a longer gap)
//! takes the nearest point; anything further from the track is left without a place.
//!
//! The reader is a small scanner for `<trkpt lat=".." lon="..">…<time>…</time>` (and `rtept`
//! and `wpt` alike), not a full XML parser: GPX files are simple and the app needs no more.

/// The longest gap between two track points that is interpolated across, in seconds.
pub const MAX_GAP: i64 = 10 * 60;
/// How far from a track point a photo may be and still take it, in seconds.
pub const REACH: i64 = 5 * 60;

/// One track point: when (Unix seconds, UTC) and where.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub t: i64,
    pub lat: f64,
    pub lon: f64,
}

/// The value of `name="…"` (or `'…'`) in a tag's text.
fn attr(tag: &str, name: &str) -> Option<f64> {
    for quote in ['"', '\''] {
        let key = format!("{name}={quote}");
        if let Some(i) = tag.find(&key) {
            let rest = &tag[i + key.len()..];
            let end = rest.find(quote)?;
            return rest[..end].trim().parse().ok();
        }
    }
    None
}

/// The points of a GPX document that carry a time, in time order.
pub fn parse(text: &str) -> Vec<Point> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = ["<trkpt", "<rtept", "<wpt"]
        .iter()
        .filter_map(|t| rest.find(t))
        .min()
    {
        rest = &rest[start..];
        let Some(open_end) = rest.find('>') else {
            break;
        };
        let tag = &rest[..open_end];
        let self_closing = tag.ends_with('/');
        let body_end = if self_closing {
            open_end
        } else {
            ["</trkpt>", "</rtept>", "</wpt>"]
                .iter()
                .filter_map(|t| rest.find(t))
                .min()
                .unwrap_or(rest.len())
        };
        let body = &rest[open_end.min(body_end)..body_end];
        let time = body.find("<time>").and_then(|i| {
            let s = &body[i + 6..];
            let e = s.find("</time>")?;
            chrono::DateTime::parse_from_rfc3339(s[..e].trim()).ok()
        });
        if let (Some(lat), Some(lon), Some(time)) = (attr(tag, "lat"), attr(tag, "lon"), time)
            && (-90.0..=90.0).contains(&lat)
            && (-180.0..=180.0).contains(&lon)
        {
            out.push(Point {
                t: time.timestamp(),
                lat,
                lon,
            });
        }
        rest = &rest[body_end.max(1)..];
    }
    out.sort_by_key(|p| p.t);
    out
}

/// A photo's time (`2024-05-01T12:34:56`, the camera's local time) in UTC seconds, the camera
/// being `offset_minutes` ahead of UTC.
pub fn photo_time(captured: &str, offset_minutes: i32) -> Option<i64> {
    let local =
        chrono::NaiveDateTime::parse_from_str(captured.get(..19)?, "%Y-%m-%dT%H:%M:%S").ok()?;
    Some(local.and_utc().timestamp() - i64::from(offset_minutes) * 60)
}

/// Where the track was at `t` (see the module notes), if near enough.
pub fn locate(track: &[Point], t: i64) -> Option<(f64, f64)> {
    let after = track.partition_point(|p| p.t <= t);
    let before = after.checked_sub(1).map(|i| track[i]);
    let next = track.get(after).copied();
    match (before, next) {
        (Some(a), Some(b)) if b.t - a.t <= MAX_GAP => {
            let k = if b.t == a.t {
                0.0
            } else {
                (t - a.t) as f64 / (b.t - a.t) as f64
            };
            Some((a.lat + (b.lat - a.lat) * k, a.lon + (b.lon - a.lon) * k))
        }
        (a, b) => [a, b]
            .into_iter()
            .flatten()
            .filter(|p| (p.t - t).abs() <= REACH)
            .min_by_key(|p| (p.t - t).abs())
            .map(|p| (p.lat, p.lon)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GPX: &str = r#"<?xml version="1.0"?>
<gpx version="1.1"><trk><trkseg>
  <trkpt lat="15.50" lon="73.80"><ele>5</ele><time>2026-10-01T06:00:00Z</time></trkpt>
  <trkpt lat='15.60' lon='73.90'><time>2026-10-01T06:10:00Z</time></trkpt>
  <trkpt lat="16.00" lon="74.00"><time>2026-10-01T08:00:00Z</time></trkpt>
  <trkpt lat="99" lon="0"><time>2026-10-01T09:00:00Z</time></trkpt>
  <trkpt lat="1" lon="1"/>
</trkseg></trk></gpx>"#;

    #[test]
    fn a_track_is_read_in_time_order() {
        let t = parse(GPX);
        assert_eq!(
            t.len(),
            3,
            "a latitude past 90 and a point with no time are left out"
        );
        assert_eq!((t[0].lat, t[1].lon), (15.5, 73.9));
        assert!(t[0].t < t[1].t);
    }

    #[test]
    fn a_photo_is_placed_along_the_track_by_its_local_time() {
        let t = parse(GPX);
        // 11:35 at UTC+05:30 is 06:05 UTC: half way along the first leg.
        let at = photo_time("2026-10-01T11:35:00", 330).unwrap();
        let (lat, lon) = locate(&t, at).unwrap();
        assert!((lat - 15.55).abs() < 1e-9 && (lon - 73.85).abs() < 1e-9);
        // Across the long gap (06:10 to 08:00): the nearest point, if near enough.
        let near = photo_time("2026-10-01T11:43:00", 330).unwrap();
        assert_eq!(locate(&t, near), Some((15.6, 73.9)));
        let far = photo_time("2026-10-01T12:30:00", 330).unwrap();
        assert_eq!(locate(&t, far), None);
        // Before the track starts, within reach; long after it ends, not.
        let early = photo_time("2026-10-01T11:27:00", 330).unwrap();
        assert_eq!(locate(&t, early), Some((15.5, 73.8)));
        assert_eq!(
            locate(&t, photo_time("2026-10-02T00:00:00", 330).unwrap()),
            None
        );
        assert_eq!(photo_time("not a date", 0), None);
    }
}
