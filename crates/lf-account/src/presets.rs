//! The studio's saved looks (`galleries/edit-presets`) and the desktop sync merge.

use crate::{AccountError, Client};
use serde_json::{Value, json};

const PATH: &str = "galleries/edit-presets";

/// A preset as the server holds it. The server's resource has `id`, `name`, `settings` and
/// `created_at` but no `updated_at`; `updated_at` is the response's `updated_at` when present,
/// else `created_at`, else empty. Timestamps are assumed to be ISO-8601 (UTC or with an offset).
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
}

pub struct Sync;

impl Sync {
    pub fn merge(local: &[LocalPreset], remote: &[Preset]) -> Plan {
        let mut plan = Plan::default();
        for l in local {
            let Some(r) = remote.iter().find(|r| r.name == l.name) else {
                // Not on the server: new here (or deleted there, then re-created).
                plan.push.push(LocalPreset {
                    remote_id: None,
                    ..l.clone()
                });
                continue;
            };
            if l.settings == r.settings {
                continue;
            }
            let (lt, rt) = (utc(&l.updated_at), utc(&r.updated_at));
            let (lc, rc) = match (l.remote_id, l.synced_at.as_deref()) {
                // Never synced under this name: both count as changed.
                (None, _) | (_, None) => (true, true),
                (Some(_), Some(s)) => {
                    let s = utc(s);
                    (lt > s, rt > s)
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
                        name: format!("{} (desktop)", l.name),
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
        for r in remote {
            if !local.iter().any(|l| l.name == r.name) {
                plan.pull.push(r.clone());
            }
        }
        plan
    }
}

/// Normalises an ISO-8601 timestamp to `YYYY-MM-DDTHH:MM:SSZ` (UTC, whole seconds) so
/// timestamps compare lexicographically. Unparseable input is returned unchanged.
fn utc(s: &str) -> String {
    parse_utc(s).unwrap_or_else(|| s.to_string())
}

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
    use super::utc;

    #[test]
    fn offsets_become_utc_z() {
        assert_eq!(utc("2026-10-25T05:30:00+05:30"), "2026-10-25T00:00:00Z");
        assert_eq!(utc("2026-10-25T00:00:00.123456Z"), "2026-10-25T00:00:00Z");
        assert_eq!(utc("2026-01-01T00:30:00+01:00"), "2025-12-31T23:30:00Z");
    }
}
