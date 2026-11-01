//! Reading Lightroom preset files into the neutral `LrPreset`.

use lf_settings::lightroom::{LrKind, LrPreset, ReadError, read_any, read_lrtemplate, read_xmp};
use serde_json::json;
use std::path::PathBuf;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/lightroom")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn warm_fade_checks(p: &LrPreset) {
    assert_eq!(p.name, "Warm Fade");
    assert_eq!(p.group.as_deref(), Some("Studio"));
    assert_eq!(p.kind, LrKind::Preset);
    assert_eq!(p.process_version.as_deref(), Some("11.0"));
    assert_eq!(p.settings["Exposure2012"], json!(0.5));
    assert_eq!(p.settings["Contrast2012"], json!(20));
    assert_eq!(p.settings["ConvertToGrayscale"], json!(false));
    assert_eq!(
        p.settings["ToneCurvePV2012"],
        json!(["0, 0", "128, 140", "255, 255"])
    );
    assert!(!p.settings.contains_key("Name"));
    assert!(!p.settings.contains_key("Group"));
}

#[test]
fn attrs_and_elements_read_the_same() {
    let a = read_xmp(&fixture("attrs.xmp")).unwrap();
    let e = read_xmp(&fixture("elements.xmp")).unwrap();
    warm_fade_checks(&a);
    warm_fade_checks(&e);
}

#[test]
fn nested_structures_mirror_the_rdf() {
    let e = read_xmp(&fixture("elements.xmp")).unwrap();
    let mask = &e.settings["MaskGroupBasedCorrections"][0];
    assert_eq!(mask["What"], json!("Correction"));
    assert_eq!(mask["CorrectionAmount"], json!(1));
    assert_eq!(mask["LocalExposure2012"], json!(0.25));
    assert_eq!(mask["CorrectionMasks"][0]["What"], json!("Mask/Gradient"));
    assert_eq!(mask["CorrectionMasks"][0]["ZeroX"], json!(0.1));
}

#[test]
fn bom_and_utf16_files_read() {
    let utf16 = read_xmp(&fixture("bom-utf16.xmp")).unwrap();
    warm_fade_checks(&utf16);
    let mut bom = vec![0xEF, 0xBB, 0xBF];
    bom.extend(fixture("attrs.xmp"));
    warm_fade_checks(&read_xmp(&bom).unwrap());
}

#[test]
fn a_profile_is_a_profile() {
    let p = read_xmp(&fixture("profile.xmp")).unwrap();
    assert_eq!(p.kind, LrKind::Profile);
    assert_eq!(p.name, "Faded Film");
    assert_eq!(p.settings["Look"]["Name"], json!("Faded Film"));
    assert_eq!(
        p.settings["Look"]["Parameters"]["ConvertToGrayscale"],
        json!(false)
    );
    // ProfileName alone also makes a profile.
    let text = String::from_utf8(fixture("attrs.xmp")).unwrap().replace(
        "crs:PresetType=\"Normal\"",
        "crs:ProfileName=\"Adobe Color\"",
    );
    assert_eq!(read_xmp(text.as_bytes()).unwrap().kind, LrKind::Profile);
}

#[test]
fn lrtemplate_reads_its_settings() {
    let text = String::from_utf8(fixture("legacy.lrtemplate")).unwrap();
    let p = read_lrtemplate(&text).unwrap();
    assert_eq!(p.name, "Moody");
    assert_eq!(p.group, None);
    assert_eq!(p.kind, LrKind::Preset);
    assert_eq!(p.process_version.as_deref(), Some("6.7"));
    assert_eq!(p.settings["Exposure2012"], json!(-0.3));
    assert_eq!(p.settings["Contrast2012"], json!(25));
    assert_eq!(p.settings["ConvertToGrayscale"], json!(true));
    assert_eq!(p.settings["ToneCurvePV2012"], json!(["0, 0", "255, 255"]));
    assert_eq!(p.settings["ToneCurveName2012"], json!("Linear"));
}

#[test]
fn read_any_goes_by_extension() {
    let x = read_any("Warm.XMP", &fixture("attrs.xmp")).unwrap();
    assert_eq!(x.name, "Warm Fade");
    let l = read_any("Moody.lrtemplate", &fixture("legacy.lrtemplate")).unwrap();
    assert_eq!(l.name, "Moody");
    assert!(matches!(
        read_any("notes.txt", b"hello"),
        Err(ReadError::NotAPreset)
    ));
}

#[test]
fn broken_and_huge_files_are_refused() {
    let mut huge = String::from("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">");
    while huge.len() <= 1 << 20 {
        huge.push_str("<!-- padding padding padding padding padding padding -->\n");
    }
    assert!(matches!(
        read_xmp(huge.as_bytes()),
        Err(ReadError::TooLarge)
    ));
    assert!(matches!(read_lrtemplate(&huge), Err(ReadError::TooLarge)));
    assert!(matches!(read_xmp(b"junk"), Err(ReadError::NotXml)));
    assert!(matches!(
        read_xmp(&fixture("broken.xmp")),
        Err(ReadError::NotXml)
    ));
    let plain = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" dc:title="x"/></rdf:RDF></x:xmpmeta>"#;
    assert!(matches!(read_xmp(plain), Err(ReadError::NotAPreset)));
    assert!(matches!(
        read_lrtemplate("s = { title = \"x\" }"),
        Err(ReadError::NotAPreset)
    ));
    assert!(matches!(
        read_lrtemplate("s = { value = {"),
        Err(ReadError::NotAPreset)
    ));
}

const SHELL: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/" ATTRS/></rdf:RDF></x:xmpmeta>"#;

#[test]
fn only_crs_keys_reach_the_settings() {
    let xml = SHELL.replace(
        "ATTRS",
        r#"crs:Exposure2012="+1.0" xmp:CreatorTool="Lightroom" dc:title="x""#,
    );
    let p = read_xmp(xml.as_bytes()).unwrap();
    assert_eq!(p.settings["Exposure2012"], json!(1.0));
    assert!(!p.settings.contains_key("CreatorTool"));
    assert!(!p.settings.contains_key("title"));
}

#[test]
fn numeric_and_boolean_looking_names_stay_text() {
    let xml = SHELL.replace(
        "ATTRS",
        r#"crs:Name="1985" crs:Group="True" crs:Exposure2012="1""#,
    );
    let p = read_xmp(xml.as_bytes()).unwrap();
    assert_eq!(p.name, "1985");
    assert_eq!(p.group.as_deref(), Some("True"));
}

#[test]
fn only_zstr_keys_are_split_at_the_equals_sign() {
    let src = |title: &str| {
        format!(
            "s = {{ title = ZSTR \"{title}\", value = {{ settings = {{ Exposure2012 = 1 }} }} }}"
        )
    };
    assert_eq!(
        read_lrtemplate(&src("$$$/A/B=Moody")).unwrap().name,
        "Moody"
    );
    assert_eq!(read_lrtemplate(&src("A=B")).unwrap().name, "A=B");
}
