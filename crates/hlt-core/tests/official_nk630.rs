//! 공식 통합명세서작성기(NK-Editor 6.30, 2025-10-01)가 Wine 에서 만든 실물 제출파일(fixtures/official-nk630)과
//! 바이트로 대조한다. 시험 원고는 공개 가능한 글(문서 처리 방법)이다.
//!
//! 경로: 공식 `.hlz` 안 XML → `generate_hlt` → `export_kipo_xml(layout = Nk630)` → 같은 XML.

use hlt_core::{KipoExportOptions, XmlLayout, export_kipo_xml_path};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};

fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

/// 공식 `.hlz`(zip)에서 XML 과 그림을 꺼낸다.
fn unpack(hlz: &Path, into: &Path) -> (PathBuf, String) {
    let file = std::fs::File::open(hlz).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut xml = None;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).unwrap();
        let name = entry.name().to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        let path = into.join(&name);
        std::fs::write(&path, &bytes).unwrap();
        if name.ends_with(".xml") {
            xml = Some((path, String::from_utf8(bytes).unwrap()));
        }
    }
    xml.unwrap()
}

/// 생성은 CLI 와 같은 길(`hlt-cli generate-hlt`)로 한다. CLI 가 없으면 건너뛴다(None).
fn cli() -> Option<PathBuf> {
    let root = repo("target");
    ["debug/hlt-cli", "release/hlt-cli"]
        .iter()
        .map(|p| root.join(p))
        .find(|p| p.exists())
}

fn generate(xml: &Path, dir: &Path, case: &str) -> PathBuf {
    let hlt = dir.join(format!("{case}.HLT"));
    let cli = cli().expect("hlt-cli 가 필요하다: cargo build -p hlt-cli");
    let out = Command::new(cli)
        .args([
            "generate-hlt",
            xml.to_str().unwrap(),
            hlt.to_str().unwrap(),
            "--dialect",
            "patent-specification-ko",
        ])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("\"ok\":true"),
        "generate-hlt: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    hlt
}

fn mask(xml: &str) -> String {
    let start = xml.find("documentID=\"").unwrap() + "documentID=\"".len();
    let end = start + xml[start..].find('"').unwrap();
    format!("{}X{}", &xml[..start], &xml[end..])
}

fn page_count(xml: &str) -> u32 {
    let start = xml.find("pageCount=\"").unwrap() + "pageCount=\"".len();
    let end = start + xml[start..].find('"').unwrap();
    xml[start..end].parse().unwrap()
}

fn round_trip(case: &str) -> (String, String) {
    let work = tempfile::tempdir().unwrap();
    let (xml_path, official) = unpack(
        &repo(&format!("fixtures/official-nk630/{case}.official.hlz")),
        work.path(),
    );
    let hlt = generate(&xml_path, work.path(), case);
    let options = KipoExportOptions {
        document_id: "X".into(),
        page_count: Some(page_count(&official)),
        keaps_version: "5.5".into(),
        layout: XmlLayout::Nk630,
        ..KipoExportOptions::default()
    };
    let export = export_kipo_xml_path(&hlt, &options).unwrap();
    (mask(&official), mask(&export.xml))
}

#[test]
fn a_specification_without_drawings_matches_nkeditor_byte_for_byte() {
    let (official, ours) = round_trip("t2");
    assert_eq!(ours, official);
}

#[test]
fn a_specification_with_a_bilevel_drawing_matches_nkeditor_byte_for_byte() {
    let (official, ours) = round_trip("t3");
    // 1비트 도면은 TIFF(G4)로 남아야 한다 — 8비트로 풀리면 `pat00001.jpg` 가 된다.
    assert!(ours.contains("file=\"pat00001.tif\" img-format=\"tif\""));
    assert_eq!(ours, official);
}

#[test]
fn the_default_layout_is_unchanged() {
    // 서안 기본 꼴(요소마다 `\n`)은 그대로다 — 기존 사용자 · 보정서 대조에 영향이 없어야 한다.
    let work = tempfile::tempdir().unwrap();
    let (xml_path, _) = unpack(
        &repo("fixtures/official-nk630/t2.official.hlz"),
        work.path(),
    );
    let hlt = generate(&xml_path, work.path(), "t2");
    let export = export_kipo_xml_path(&hlt, &KipoExportOptions::default()).unwrap();
    assert!(export.xml.contains("<KIPO keapsVersion=\"5.6\""));
    assert!(export.xml.contains("<technical-field>\n<p num=\"0001\">"));
    assert!(!export.xml.contains('\r'));
}
