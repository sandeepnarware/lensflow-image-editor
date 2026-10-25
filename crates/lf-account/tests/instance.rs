use lf_account::auth::StudioSummary;
use lf_account::{AccountError, Instance};
use url::Url;

fn custom(s: &str) -> Result<Instance, AccountError> {
    Instance::custom(Url::parse(s).unwrap())
}

#[test]
fn a_custom_instance_must_be_https_unless_local() {
    assert!(custom("https://lensflow.example/api").is_ok());
    assert!(custom("http://localhost:8000/api").is_ok());
    assert!(custom("http://127.0.0.1:9/api").is_ok());
    for bad in [
        "http://lensflow.example/api",
        "http://10.0.0.5/api",
        "ftp://lensflow.example/api",
        "http://localhost.evil.example/api",
    ] {
        let err = custom(bad).unwrap_err();
        assert!(
            matches!(&err, AccountError::Validation(m) if m["url"].contains("https")),
            "{bad}: {err:?}"
        );
    }
    let i = custom("https://lensflow.example/api").unwrap();
    assert_eq!(i.base_url().as_str(), "https://lensflow.example/api");
}

#[test]
fn can_use_desktop_needs_galleries_manage() {
    let studio = |perms: &[&str]| -> StudioSummary {
        serde_json::from_value(serde_json::json!({
            "id": 1, "name": "S", "slug": "s",
            "permissions": perms,
        }))
        .unwrap()
    };
    assert!(studio(&["galleries.view", "galleries.manage"]).can_use_desktop());
    assert!(!studio(&["galleries.view"]).can_use_desktop());
    assert!(!studio(&[]).can_use_desktop());
}
