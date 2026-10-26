//! Studio presets: the copies kept in the catalogue (per studio) synced with LensFlow's
//! `galleries/edit-presets`. Runs on the `account` lane at sign-in and after each local
//! save or delete, only while the gate allows sync (`Decision::sync_allowed`, false in
//! read-only).
//!
//! The merge rules are [`lf_account::presets::Sync::merge`]'s. Timestamps: the server returns
//! `updated_at` on each preset; when a reply lacks it (an older server), the remote time is
//! empty, `merge` cannot order the two sides, and it keeps both copies (a conflict) rather
//! than guess. After each push or pull the local row takes the server's `updated_at` as both
//! its `updated_at` and `synced_at`, so the next sync compares against the server's own clock.

use lf_account::presets::{self, LocalPreset, Preset, Sync};
use lf_account::{AccountError, Client};
use lf_catalog::Catalog;
use lf_catalog::presets::{self as local, LocalPresetRow};
use std::fmt;

/// What a sync did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Created or updated on the server.
    pub pushed: usize,
    /// Taken from the server into the catalogue.
    pub pulled: usize,
    /// Deleted on the server, or removed here because the server no longer has them.
    pub deleted: usize,
    /// Changed on both sides: both copies kept.
    pub conflicts: usize,
}

/// Why a sync stopped.
#[derive(Debug)]
pub enum SyncError {
    Account(AccountError),
    Catalog(String),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::Account(e) => write!(f, "{e}"),
            SyncError::Catalog(e) => write!(f, "{e}"),
        }
    }
}

impl From<AccountError> for SyncError {
    fn from(e: AccountError) -> Self {
        SyncError::Account(e)
    }
}

impl From<lf_catalog::CatalogError> for SyncError {
    fn from(e: lf_catalog::CatalogError) -> Self {
        SyncError::Catalog(e.to_string())
    }
}

/// Syncs `studio_id`'s presets in `catalog` with the server `client` talks to.
pub fn run(client: &Client, catalog: &Catalog, studio_id: i64) -> Result<SyncReport, SyncError> {
    let remote = presets::list(client)?;
    let live = local::list(catalog, studio_id)?;
    let dead = local::tombstones(catalog, studio_id)?;
    let plan = Sync::merge(&as_local(&live), &as_local(&dead), &remote);
    let mut report = SyncReport {
        conflicts: plan.conflicts.len(),
        ..SyncReport::default()
    };
    for &id in &plan.delete_remote {
        match presets::delete(client, id) {
            // Already gone there: nothing left to do but forget it here.
            Ok(()) | Err(AccountError::Server(404, _)) => {}
            Err(e) => return Err(e.into()),
        }
        for row in dead.iter().filter(|r| r.remote_id == Some(id)) {
            local::purge(catalog, row.id)?;
        }
        report.deleted += 1;
    }
    for name in &plan.purge {
        if let Some(row) = live.iter().chain(&dead).find(|r| &r.name == name) {
            local::purge(catalog, row.id)?;
            report.deleted += 1;
        }
    }
    for p in &plan.push {
        let saved = match p.remote_id {
            Some(id) => presets::update(client, id, &p.name, &p.settings)?,
            None => presets::create(client, &p.name, &p.settings)?,
        };
        let at = server_time(&saved);
        local::upsert(
            catalog,
            studio_id,
            &p.name,
            &p.settings,
            &at,
            Some(saved.id),
            Some(&at),
        )?;
        report.pushed += 1;
    }
    for r in &plan.pull {
        let at = server_time(r);
        local::upsert(
            catalog,
            studio_id,
            &r.name,
            &r.settings,
            &at,
            Some(r.id),
            Some(&at),
        )?;
        report.pulled += 1;
    }
    log::info!("studio presets synced: {report:?}");
    Ok(report)
}

fn as_local(rows: &[LocalPresetRow]) -> Vec<LocalPreset> {
    rows.iter()
        .map(|r| LocalPreset {
            name: r.name.clone(),
            settings: r.settings.clone(),
            updated_at: r.updated_at.clone(),
            remote_id: r.remote_id,
            synced_at: r.synced_at.clone(),
        })
        .collect()
}

/// The server's `updated_at` in the catalogue's form; now when it has none.
fn server_time(p: &Preset) -> String {
    use lf_catalog::model::{now, parse_ts, ts};
    parse_ts(&p.updated_at).map_or_else(|| ts(now()), ts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use lf_account::Instance;
    use serde_json::json;

    const STUDIO: i64 = 3;
    const T0: &str = "2026-10-10T00:00:00.000Z";

    fn client(server: &MockServer) -> Client {
        let url = reqwest::Url::parse(&server.url("/api")).unwrap();
        Client::new(Instance::custom(url).unwrap(), "1.0.0")
            .with_token("t")
            .with_studio("frames")
    }

    fn catalogue() -> (tempfile::TempDir, Catalog) {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::open(dir.path()).unwrap();
        (dir, c)
    }

    fn rows(c: &Catalog) -> Vec<(String, Option<i64>, serde_json::Value)> {
        local::list(c, STUDIO)
            .unwrap()
            .into_iter()
            .map(|r| (r.name, r.remote_id, r.settings))
            .collect()
    }

    #[test]
    fn sync_pushes_new_local_and_pulls_new_remote() {
        let server = MockServer::start();
        let (_d, c) = catalogue();
        local::save(&c, STUDIO, "Warm", &json!({"v": 2, "temperature": 20})).unwrap();
        server.mock(|w, t| {
            w.method(GET).path("/api/galleries/edit-presets");
            t.status(200).json_body(json!({"data": [
                {"id": 5, "name": "Cool", "settings": {"v": 2, "temperature": -20},
                 "created_at": T0, "updated_at": "2026-10-11T00:00:00+00:00"}
            ]}));
        });
        let post = server.mock(|w, t| {
            w.method(POST)
                .path("/api/galleries/edit-presets")
                .json_body(json!({"name": "Warm", "settings": {"v": 2, "temperature": 20}}));
            t.status(201).json_body(json!({"data":
                {"id": 6, "name": "Warm", "settings": {"v": 2, "temperature": 20},
                 "created_at": T0, "updated_at": "2026-10-12T00:00:00+00:00"}
            }));
        });
        let report = run(&client(&server), &c, STUDIO).unwrap();
        post.assert();
        assert_eq!(
            report,
            SyncReport {
                pushed: 1,
                pulled: 1,
                deleted: 0,
                conflicts: 0
            }
        );
        assert_eq!(
            rows(&c),
            [
                ("Cool".into(), Some(5), json!({"v": 2, "temperature": -20})),
                ("Warm".into(), Some(6), json!({"v": 2, "temperature": 20})),
            ]
        );
        let cool = local::find(&c, STUDIO, "Cool").unwrap().unwrap();
        assert!(cool.synced_at.is_some());
    }

    #[test]
    fn sync_conflict_keeps_both() {
        let server = MockServer::start();
        let (_d, c) = catalogue();
        // Synced at T0, then changed here (Oct 12) and on the web (Oct 13).
        local::upsert(
            &c,
            STUDIO,
            "A",
            &json!({"v": 2, "exposure": 1}),
            "2026-10-12T00:00:00.000Z",
            Some(9),
            Some(T0),
        )
        .unwrap();
        server.mock(|w, t| {
            w.method(GET).path("/api/galleries/edit-presets");
            t.status(200).json_body(json!({"data": [
                {"id": 9, "name": "A", "settings": {"v": 2, "exposure": -1},
                 "created_at": T0, "updated_at": "2026-10-13T00:00:00+00:00"}
            ]}));
        });
        let post = server.mock(|w, t| {
            w.method(POST)
                .path("/api/galleries/edit-presets")
                .json_body(json!({"name": "A (desktop)", "settings": {"v": 2, "exposure": 1}}));
            t.status(201).json_body(json!({"data":
                {"id": 10, "name": "A (desktop)", "settings": {"v": 2, "exposure": 1},
                 "created_at": T0, "updated_at": "2026-10-14T00:00:00+00:00"}
            }));
        });
        let report = run(&client(&server), &c, STUDIO).unwrap();
        post.assert();
        assert_eq!(report.conflicts, 1);
        assert_eq!(
            rows(&c),
            [
                ("A".into(), Some(9), json!({"v": 2, "exposure": -1})),
                (
                    "A (desktop)".into(),
                    Some(10),
                    json!({"v": 2, "exposure": 1})
                ),
            ],
            "the web's edit keeps the name; this computer's is kept as a copy"
        );
    }

    #[test]
    fn sync_deletes_a_tombstone_on_the_server_then_forgets_it() {
        let server = MockServer::start();
        let (_d, c) = catalogue();
        let id = local::upsert(&c, STUDIO, "Old", &json!({"v": 2}), T0, Some(9), Some(T0)).unwrap();
        local::delete(&c, id).unwrap();
        server.mock(|w, t| {
            w.method(GET).path("/api/galleries/edit-presets");
            t.status(200).json_body(json!({"data": [
                {"id": 9, "name": "Old", "settings": {"v": 2}, "created_at": T0, "updated_at": T0}
            ]}));
        });
        let del = server.mock(|w, t| {
            w.method(DELETE).path("/api/galleries/edit-presets/9");
            t.status(204);
        });
        let report = run(&client(&server), &c, STUDIO).unwrap();
        del.assert();
        assert_eq!(report.deleted, 1);
        assert!(local::list(&c, STUDIO).unwrap().is_empty());
        assert!(local::tombstones(&c, STUDIO).unwrap().is_empty());
    }

    #[test]
    fn a_revoked_token_stops_the_sync_as_unauthorized() {
        let server = MockServer::start();
        let (_d, c) = catalogue();
        server.mock(|w, t| {
            w.method(GET).path("/api/galleries/edit-presets");
            t.status(401)
                .json_body(json!({"message": "Unauthenticated."}));
        });
        match run(&client(&server), &c, STUDIO) {
            Err(SyncError::Account(AccountError::Unauthorized)) => {}
            other => panic!("{other:?}"),
        }
    }
}
