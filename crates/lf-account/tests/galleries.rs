//! Export to a gallery against a mock LensFlow API: the gallery list, chunked and direct
//! uploads, and a duplicate.

use httpmock::prelude::*;
use lf_account::galleries::{self, Uploaded};
use lf_account::{Client, Instance};
use serde_json::json;

fn client(server: &MockServer) -> Client {
    let url = url::Url::parse(&server.url("/api")).unwrap();
    Client::new(Instance::custom(url).unwrap(), "1.0.0")
        .with_token("t")
        .with_studio("frames")
}

fn session(chunk: u64, total: u64, direct: Option<String>) -> serde_json::Value {
    json!({"data": {
        "id": "8a1f0b0e-1111-4c4c-9999-123456789abc",
        "chunk_size": chunk,
        "chunks_total": total,
        "status": "pending",
        "direct": direct.map(|url| json!({"url": url, "block_size": chunk, "expires_at": "x"})),
    }})
}

const UPLOADS: &str = "/api/galleries/5/uploads";
const SESSION: &str = "/api/galleries/5/uploads/8a1f0b0e-1111-4c4c-9999-123456789abc";

#[test]
fn the_gallery_list_asks_for_active_galleries_and_reads_them() {
    let server = MockServer::start();
    let list = server.mock(|w, t| {
        w.method(GET)
            .path("/api/galleries")
            .query_param("status", "active")
            .query_param("per_page", "100")
            .query_param("search", "Ana & Raj");
        t.status(200).json_body(json!({"data": [
            {"id": 5, "title": "Ana & Raj", "event_date": "2026-11-02", "status": "published"},
            {"id": 6, "title": null, "status": "draft"}
        ]}));
    });
    let sections = server.mock(|w, t| {
        w.method(GET).path("/api/galleries/5/sections");
        t.status(200)
            .json_body(json!({"data": [{"id": 1, "name": "Haldi"}, {"id": 2, "name": "Wedding"}]}));
    });
    let c = client(&server);
    let g = galleries::list(&c, Some(" Ana & Raj ")).unwrap();
    list.assert();
    assert_eq!(g.len(), 2);
    assert_eq!((g[0].id, g[0].title.as_str()), (5, "Ana & Raj"));
    assert_eq!(g[0].event_date.as_deref(), Some("2026-11-02"));
    assert_eq!(g[1].title, "Untitled gallery");
    let s = galleries::sections(&c, 5).unwrap();
    sections.assert();
    assert_eq!(s[1].name, "Wedding");
}

#[test]
fn a_chunked_upload_sends_each_chunk_then_completes() {
    let server = MockServer::start();
    let jpeg: Vec<u8> = (0..25u8).collect();
    let start = server.mock(|w, t| {
        w.method(POST)
            .path(UPLOADS)
            .header("x-studio", "frames")
            .json_body_includes(
                r#"{"filename": "a.jpg", "bytes": 25, "mime": "image/jpeg", "section_id": 2}"#,
            );
        t.status(201).json_body(session(10, 3, None));
    });
    let chunks: Vec<_> = (0..3)
        .map(|i| {
            let part = jpeg[i * 10..((i + 1) * 10).min(25)].to_vec();
            server.mock(move |w, t| {
                w.method(PUT)
                    .path(format!("{SESSION}/chunks/{i}"))
                    .body(String::from_utf8_lossy(&part).to_string());
                t.status(200).json_body(session(10, 3, None));
            })
        })
        .collect();
    let complete = server.mock(|w, t| {
        w.method(POST).path(format!("{SESSION}/complete"));
        t.status(201).json_body(
            json!({"data": {"upload": {}, "photo": {"id": 901, "status": "processing"}}}),
        );
    });
    let mut sent = Vec::new();
    let got = galleries::upload(&client(&server), 5, Some(2), "a.jpg", &jpeg, &mut |n| {
        sent.push(n)
    })
    .unwrap();
    start.assert();
    for c in &chunks {
        c.assert();
    }
    complete.assert();
    assert_eq!(got, Uploaded::Photo(901));
    assert_eq!(sent, [10, 20, 25]);
}

#[test]
fn a_direct_upload_puts_blocks_to_storage_and_joins_them() {
    let server = MockServer::start();
    let jpeg = vec![7u8; 15];
    let blob = server.url("/blob/p.jpg?sv=1&sig=x");
    server.mock(|w, t| {
        w.method(POST).path(UPLOADS);
        t.status(201).json_body(session(8, 2, Some(blob.clone())));
    });
    let blocks: Vec<_> = [0usize, 1]
        .into_iter()
        .map(|i| {
            server.mock(move |w, t| {
                w.method(PUT)
                    .path("/blob/p.jpg")
                    .query_param("comp", "block")
                    .query_param("blockid", galleries::block_id(i))
                    // Storage links carry their own permission: no API token goes there.
                    .is_true(|req| {
                        !req.headers()
                            .iter()
                            .any(|(k, _)| k.as_str().eq_ignore_ascii_case("authorization"))
                    });
                t.status(201);
            })
        })
        .collect();
    let list = server.mock(|w, t| {
        w.method(PUT)
            .path("/blob/p.jpg")
            .query_param("comp", "blocklist")
            .header("x-ms-blob-content-type", "image/jpeg")
            .body_includes(format!(
                "<Latest>{}</Latest><Latest>{}</Latest>",
                galleries::block_id(0),
                galleries::block_id(1)
            ));
        t.status(201);
    });
    let complete = server.mock(|w, t| {
        w.method(POST).path(format!("{SESSION}/complete"));
        t.status(201)
            .json_body(json!({"data": {"upload": {}, "photo": {"id": 77, "status": "ready"}}}));
    });
    let got = galleries::upload(&client(&server), 5, None, "p.jpg", &jpeg, &mut |_| {}).unwrap();
    for b in &blocks {
        b.assert();
    }
    list.assert();
    complete.assert();
    assert_eq!(got, Uploaded::Photo(77));
}

#[test]
fn a_photo_the_gallery_already_has_is_a_duplicate_not_an_error() {
    let server = MockServer::start();
    server.mock(|w, t| {
        w.method(POST).path(UPLOADS);
        t.status(409)
            .json_body(json!({"message": "Already in this gallery.", "duplicate_photo_id": 33}));
    });
    let got = galleries::upload(&client(&server), 5, None, "a.jpg", b"jpeg", &mut |_| {}).unwrap();
    assert_eq!(got, Uploaded::Duplicate(Some(33)));
}

#[test]
fn a_refusal_is_an_error_with_the_servers_message() {
    let server = MockServer::start();
    server.mock(|w, t| {
        w.method(POST).path(UPLOADS);
        t.status(422).json_body(json!({
            "message": "This gallery is archived. Restore it before adding photos."
        }));
    });
    let e = galleries::upload(&client(&server), 5, None, "a.jpg", b"jpeg", &mut |_| {})
        .unwrap_err()
        .to_string();
    assert!(e.contains("archived"), "{e}");
}
