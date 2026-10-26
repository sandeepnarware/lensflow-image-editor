//! The studio's saved looks (`galleries/edit-presets`) and the desktop sync merge.

use crate::{AccountError, Client};
use serde_json::{Value, json};

const PATH: &str = "galleries/edit-presets";

/// A preset as the server holds it (`id`, `name`, `settings`, `created_at`, `updated_at`).
/// `updated_at` is the response's `updated_at` when present (older servers left it out),
/// else `created_at`, else empty. Timestamps are assumed to be ISO-8601 (UTC or with an
/// offset).
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub id: i64,
    pub name: String,
    pub settings: Value,
    pub updated_at: String,
}

fn parse(v: &Value) -> Result<Preset, AccountError> {
    let bad = || AccountError::Server(200, "Unexpected preset response.".into());
    let s = |k: &str| v.get(k).and_then(Value::as_str);
    Ok(Preset {
        id: v.get("id").and_then(Value::as_i64).ok_or_else(bad)?,
        name: s("name").ok_or_else(bad)?.to_string(),
        settings: v.get("settings").cloned().unwrap_or(Value::Null),
        updated_at: s("updated_at")
            .or_else(|| s("created_at"))
            .unwrap_or("")
            .to_string(),
    })
}

fn one(body: &Value) -> Result<Preset, AccountError> {
    parse(body.get("data").unwrap_or(body))
}

pub fn list(client: &Client) -> Result<Vec<Preset>, AccountError> {
    let body = client.get_json(PATH)?;
    body.get("data")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(parse).collect())
        .unwrap_or_else(|| Ok(Vec::new()))
}

pub fn create(client: &Client, name: &str, settings: &Value) -> Result<Preset, AccountError> {
    one(&client.post_json(PATH, &json!({"name": name, "settings": settings}))?)
}

pub fn update(
    client: &Client,
    id: i64,
    name: &str,
    settings: &Value,
) -> Result<Preset, AccountError> {
    let req = client
        .request(reqwest::Method::PATCH, &format!("{PATH}/{id}"))
        .json(&json!({"name": name, "settings": settings}));
    one(&client.send(req)?)
}

pub fn delete(client: &Client, id: i64) -> Result<(), AccountError> {
    client
        .send(client.request(reqwest::Method::DELETE, &format!("{PATH}/{id}")))
        .map(|_| ())
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalPreset {
    pub name: String,
    pub settings: Value,
    /// ISO-8601 (offsets are converted to UTC for comparison).
    pub updated_at: String,
    pub remote_id: Option<i64>,
    pub synced_at: Option<String>,
}

/// What a sync should do. `push` entries with a `remote_id` are updates, without are creates.
/// A conflict appears in all three: `conflicts` (renamed local copy, remote), the renamed
/// copy in `push` (a create), and the remote in `pull` (it takes the original name locally).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    pub push: Vec<LocalPreset>,
    pub pull: Vec<Preset>,
    pub conflicts: Vec<(LocalPreset, Preset)>,
    /// Server ids of presets deleted here (tombstones) to delete there; each tombstone is
    /// purged once its delete succeeds.
    pub delete_remote: Vec<i64>,
    /// Names of local rows to remove for good: tombstones with nothing left to delete on
    /// the server, and synced presets that were deleted on the server.
    pub purge: Vec<String>,
}

pub struct Sync;

impl Sync {
    /// Plans a sync of this computer's presets (`local`, and the tombstones of those deleted
    /// here, `deleted`) with the server's (`remote`).
    ///
    /// Changed since the last sync: the sync writes the server's own `updated_at` string to
    /// both `updated_at` and `synced_at`, so a local preset changed here is one whose
    /// `updated_at` is no longer equal to its `synced_at` (no comparison across this
    /// computer's clock and the server's). A remote preset changed since is one whose
    /// `updated_at` is later than `synced_at` (both the server's clock).
    ///
    /// - A local preset is matched with a remote one by its `remote_id`, else by name (a
    ///   rename on either side follows the id); an unlinked local preset never matches a
    ///   remote one another local preset is linked to. Only one side changed: it wins (push,
    ///   or pull). Both changed (or a time that cannot be read, or never synced under this
    ///   name): a conflict, both kept.
    /// - An unlinked local preset whose name a remote preset linked to another local one now
    ///   has (a rename on the server onto it): a conflict; it is pushed as `<name> (desktop)`
    ///   and its old name purged here, so the pull can take the name.
    /// - A local preset not on the server: unchanged since it was synced, it was deleted on
    ///   the server, so it is purged here; changed here (or never synced), it is pushed as new.
    /// - A tombstone whose `remote_id` is still on the server: deleted there
    ///   (`delete_remote`), even if a teammate changed it since: the delete wins. Any other
    ///   tombstone: purged.
    /// - A remote preset no local preset or tombstone matches: pulled.
    pub fn merge(local: &[LocalPreset], deleted: &[LocalPreset], remote: &[Preset]) -> Plan {
        let mut plan = Plan::default();
        let linked = |id: i64| local.iter().any(|o| o.remote_id == Some(id));
        let find = |l: &LocalPreset| match l.remote_id {
            Some(id) => remote.iter().find(|r| r.id == id),
            None => remote.iter().find(|r| r.name == l.name && !linked(r.id)),
        };
        for l in local {
            let Some(r) = find(l) else {
                let unchanged = l.synced_at.as_deref() == Some(l.updated_at.as_str());
                if let Some(r) = remote
                    .iter()
                    .find(|r| r.name == l.name && l.remote_id.is_none())
                {
                    // Renamed on the server onto this unlinked preset's name: keep this one
                    // under another name, and free the name for the pull.
                    let copy = LocalPreset {
                        name: conflict_name(&l.name, local, remote),
                        remote_id: None,
                        synced_at: None,
                        ..l.clone()
                    };
                    plan.purge.push(l.name.clone());
                    plan.push.push(copy.clone());
                    plan.conflicts.push((copy, r.clone()));
                } else if l.remote_id.is_some() && unchanged {
                    // Synced, unchanged here since, and gone from the server: deleted on the web.
                    plan.purge.push(l.name.clone());
                } else {
                    plan.push.push(LocalPreset {
                        remote_id: None,
                        ..l.clone()
                    });
                }
                continue;
            };
            if r.name == l.name && settings_equal(&l.settings, &r.settings) {
                if l.remote_id != Some(r.id) {
                    // The same preset, never linked: the pull records its server id.
                    plan.pull.push(r.clone());
                }
                continue;
            }
            let synced = l.synced_at.as_deref();
            let (lc, rc) = match (l.remote_id, synced, synced.and_then(parse_utc)) {
                // Never synced under this name, or a time we cannot read: both count as changed.
                (None, _, _) | (_, None, _) | (_, _, None) => (true, true),
                (Some(_), Some(at), Some(s)) => {
                    let local_changed = l.updated_at != at;
                    match parse_utc(&r.updated_at) {
                        Some(rt) => (local_changed, rt > s),
                        None => (true, true),
                    }
                }
            };
            match (lc, rc) {
                (true, false) => plan.push.push(LocalPreset {
                    remote_id: Some(r.id),
                    ..l.clone()
                }),
                (false, true) => plan.pull.push(r.clone()),
                (false, false) => {}
                (true, true) => {
                    let copy = LocalPreset {
                        name: conflict_name(&l.name, local, remote),
                        remote_id: None,
                        synced_at: None,
                        ..l.clone()
                    };
                    plan.push.push(copy.clone());
                    plan.pull.push(r.clone());
                    plan.conflicts.push((copy, r.clone()));
                }
            }
        }
        for d in deleted {
            match d.remote_id {
                Some(id) if remote.iter().any(|r| r.id == id) => plan.delete_remote.push(id),
                _ => plan.purge.push(d.name.clone()),
            }
        }
        for r in remote {
            let known = local
                .iter()
                .any(|l| l.remote_id == Some(r.id) || (l.remote_id.is_none() && l.name == r.name));
            let tombstoned = deleted.iter().any(|d| d.remote_id == Some(r.id));
            if !known && !tombstoned {
                plan.pull.push(r.clone());
            }
        }
        plan
    }
}

/// `"<name> (desktop)"`, or `(desktop 2)`, `(desktop 3)`, ... when that name is taken.
fn conflict_name(name: &str, local: &[LocalPreset], remote: &[Preset]) -> String {
    let taken = |n: &str| local.iter().any(|l| l.name == n) || remote.iter().any(|r| r.name == n);
    let first = format!("{name} (desktop)");
    if !taken(&first) {
        return first;
    }
    (2..)
        .map(|i| format!("{name} (desktop {i})"))
        .find(|n| !taken(n))
        .expect("unbounded range")
}

/// Settings equality that ignores the integer/float spelling of numbers (`1` equals `1.0`).
pub fn settings_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-9,
            _ => x == y,
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| settings_equal(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| settings_equal(v, w)))
        }
        _ => a == b,
    }
}

/// Parses an ISO-8601 timestamp into `YYYY-MM-DDTHH:MM:SSZ` (UTC, whole seconds) so
/// timestamps compare lexicographically; `None` when unparseable.
fn parse_utc(s: &str) -> Option<String> {
    let (date, rest) = s.split_once(['T', ' '])?;
    let mut d = date.split('-');
    let (y, mo, dd): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let split = rest.find(['Z', 'z', '+', '-']).unwrap_or(rest.len());
    let (time, zone) = rest.split_at(split);
    let mut t = time.split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let mi: i64 = t.next()?.parse().ok()?;
    let sec: i64 = t
        .next()
        .map(|x| x.split('.').next().unwrap_or("0"))
        .unwrap_or("0")
        .parse()
        .ok()?;
    let offset = match zone {
        "" | "Z" | "z" => 0,
        z => {
            let sign = if z.starts_with('-') { -1 } else { 1 };
            let z = z[1..].replace(':', "");
            let oh: i64 = z.get(..2)?.parse().ok()?;
            let om: i64 = z
                .get(2..)
                .filter(|m| !m.is_empty())
                .map_or(Some(0), |m| m.parse().ok())?;
            sign * (oh * 3600 + om * 60)
        }
    };
    let total = days_from_civil(y, mo, dd) * 86_400 + h * 3600 + mi * 60 + sec - offset;
    let (days, rem) = (total.div_euclid(86_400), total.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    ))
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::parse_utc;

    #[test]
    fn offsets_become_utc_z() {
        assert_eq!(
            parse_utc("2026-10-25T05:30:00+05:30").unwrap(),
            "2026-10-25T00:00:00Z"
        );
        assert_eq!(
            parse_utc("2026-10-25T00:00:00.123456Z").unwrap(),
            "2026-10-25T00:00:00Z"
        );
        assert_eq!(
            parse_utc("2026-01-01T00:30:00+01:00").unwrap(),
            "2025-12-31T23:30:00Z"
        );
    }
}
