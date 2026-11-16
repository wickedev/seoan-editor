//! 글 원고(`.txt`) → KIPO 제출 XML(NK-Editor 6.30 꼴).
//!
//! 원고는 별지 제15·16·17호 식별항목을 한 줄에 하나(【발명의 명칭】 · 【청구항 1】 · 【도 1】 …)로 쓰고,
//! 그 다음 줄부터 내용을 쓴다. 문단번호(【0001】)는 쓰지 않는다(작성기가 매긴다 · 쓰면 E-201/E-214).
//! 도면은 【도 N】 아래에 그림 경로 한 줄(`[drawings/fig1.tif 삽입]` 또는 경로만, 원고 기준 상대 경로).
//!
//! 여기서 만든 XML 은 `inspect_kipo_xml_path` → HLT 생성으로 이어진다. 내는 XML 자체가 공식 작성기 제출 XML 과
//! 같은 꼴이라(`fixtures/official-nk630` 바이트 일치) 그대로 `.hlz` 로 묶어도 된다.
//!
//! 공식 작성기가 실제로 거부한 것은 원고 단계에서 막는다:
//! 【명세서】 표제 · 【0001】 번호(E-201/E-214), 두 줄 명칭(E-262), 「제 3도」 대표도(E-242),
//! 내용 없는 표제(E-204), CP949 밖 글자(작성기가 `?` 로 바꾼다).

use std::{collections::BTreeMap, path::Path};

use crate::{CoreError, ErrorCode};

/// 식별항목 → (CAF 요소, 묶음 요소).
const SECTIONS: [(&str, &str, Option<&str>); 13] = [
    ("기술분야", "technical-field", None),
    ("발명의 배경이 되는 기술", "background-art", None),
    ("선행기술문헌", "citation-list", None),
    ("특허문헌", "patent-literature", Some("citation-list")),
    ("비특허문헌", "non-patent-literature", Some("citation-list")),
    (
        "해결하려는 과제",
        "tech-problem",
        Some("summary-of-invention"),
    ),
    (
        "해결하고자 하는 과제",
        "tech-problem",
        Some("summary-of-invention"),
    ),
    (
        "과제의 해결 수단",
        "tech-solution",
        Some("summary-of-invention"),
    ),
    (
        "발명의 효과",
        "advantageous-effects",
        Some("summary-of-invention"),
    ),
    ("도면의 간단한 설명", "description-of-drawings", None),
    (
        "발명을 실시하기 위한 구체적인 내용",
        "description-of-embodiments",
        None,
    ),
    ("산업상 이용가능성", "industrial-applicability", None),
    ("부호의 설명", "reference-signs-list", None),
];
/// 내용 없이 다음 표제가 와야 하는 묶음 표제.
const STRUCTURAL: [&str; 5] = ["발명의 설명", "발명의 내용", "청구범위", "요약서", "도면"];

/// 원고에서 읽은 것.
#[derive(Debug, Default)]
pub struct Manuscript {
    pub title: String,
    sections: Vec<(&'static str, Vec<String>)>,
    pub claims: Vec<String>,
    pub abstract_lines: Vec<String>,
    pub representative_figure: Option<u32>,
    /// (도 번호, 원고 기준 그림 경로).
    pub figures: Vec<(u32, String)>,
}

fn manuscript_error(phase: &str, detail: String) -> CoreError {
    let mut error = CoreError::new(ErrorCode::StructureInvalid, "manuscript", false);
    error.details.insert("phase".into(), phase.into());
    error.details.insert("issues".into(), detail);
    error
}

fn heading(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let rest = line.strip_prefix('【')?;
    let (name, after) = rest.split_once('】')?;
    Some((
        name.split_whitespace().collect::<Vec<_>>().join(" "),
        after.trim().to_string(),
    ))
}

fn is_para_number(name: &str) -> bool {
    name.len() == 4 && name.bytes().all(|b| b.is_ascii_digit())
}

/// 원고 글을 읽는다. 오류는 모아서 한 번에 돌려준다(작성기 오류 코드와 함께).
pub fn parse_manuscript(text: &str) -> Result<Manuscript, CoreError> {
    let mut items: Vec<(String, Vec<String>)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for raw in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        if let Some((name, after)) = heading(raw) {
            if is_para_number(&name) {
                errors.push(format!(
                    "E-201 【{name}】 문단번호는 쓰지 않습니다(작성기가 매깁니다)"
                ));
                continue;
            }
            items.push((name, Vec::new()));
            if !after.is_empty() {
                errors.push(format!(
                    "E-214 【{}】 같은 줄에 내용 — 다음 줄에 씁니다",
                    items.last().unwrap().0
                ));
                items.last_mut().unwrap().1.push(after);
            }
            continue;
        }
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        match items.last_mut() {
            Some((_, lines)) => lines.push(line.to_string()),
            None => errors.push(format!(
                "식별항목 앞에 글: {}",
                line.chars().take(30).collect::<String>()
            )),
        }
    }

    let mut m = Manuscript::default();
    let mut section_index: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (name, lines) in &items {
        let name = name.replace("고안", "발명");
        let name = name.as_str();
        if name == "명세서" {
            errors.push("E-249 【명세서】 표제는 본문에 쓰지 않습니다".into());
        } else if name == "발명의 명칭" {
            if lines.len() != 1 {
                errors.push(format!(
                    "E-262 【발명의 명칭】은 한 문단이어야 합니다({}문단)",
                    lines.len()
                ));
            }
            m.title = lines.join(" ");
        } else if let Some(&(_, element, _)) = SECTIONS.iter().find(|(n, _, _)| *n == name) {
            if lines.is_empty() && element != "citation-list" {
                errors.push(format!("E-204 【{name}】 내용이 없습니다"));
            }
            let at = *section_index.entry(element).or_insert_with(|| {
                m.sections.push((element, Vec::new()));
                m.sections.len() - 1
            });
            m.sections[at].1.extend(lines.iter().cloned());
        } else if let Some(number) = name.strip_prefix("청구항 ") {
            if lines.is_empty() {
                errors.push(format!("E-204 【청구항 {number}】 내용이 없습니다"));
            }
            if number.parse::<usize>().ok() != Some(m.claims.len() + 1) {
                errors.push(format!(
                    "E-231 청구항 번호가 1부터 이어지지 않습니다(【청구항 {number}】)"
                ));
            }
            m.claims.push(lines.join(" "));
        } else if name == "요약" {
            m.abstract_lines.extend(lines.iter().cloned());
        } else if name == "대표도" {
            let joined = lines.join(" ");
            if joined.trim_start().starts_with('제') {
                errors.push(format!(
                    "E-242 【대표도】는 「도 3」 꼴로 씁니다(「{joined}」)"
                ));
            }
            m.representative_figure = joined
                .chars()
                .filter(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok();
        } else if let Some(number) = name.strip_prefix("도 ") {
            let path = lines
                .iter()
                .map(|l| {
                    l.trim_matches(['[', ']', ' '])
                        .trim_end_matches("삽입")
                        .trim()
                })
                .find(|l| {
                    let l = l.to_ascii_lowercase();
                    [".tif", ".tiff", ".jpg", ".jpeg"]
                        .iter()
                        .any(|e| l.ends_with(e))
                });
            match (number.parse::<u32>(), path) {
                (Ok(n), Some(path)) => m.figures.push((n, path.to_string())),
                (_, None) => {
                    errors.push(format!("E-204 【도 {number}】 아래에 그림 경로가 없습니다"))
                }
                (Err(_), _) => errors.push(format!("E-232 【도 {number}】 번호")),
            }
        } else if STRUCTURAL.contains(&name) {
            if !lines.is_empty() {
                errors.push(format!(
                    "E-249 【{name}】 바로 아래에는 하위 식별항목이 와야 합니다"
                ));
            }
        } else {
            errors.push(format!("E-201 작성지침에 없는 식별항목 【{name}】"));
        }
    }
    if m.title.is_empty() {
        errors.push("E-245 【발명의 명칭】이 없습니다".into());
    }
    for need in ["technical-field", "description-of-embodiments"] {
        if !m.sections.iter().any(|(e, _)| *e == need) {
            errors.push(format!("E-245 필수 식별항목이 없습니다({need})"));
        }
    }
    if m.claims.is_empty() {
        errors.push("【청구항】이 없습니다".into());
    }
    if m.abstract_lines.is_empty() {
        errors.push("E-245 【요약】이 없습니다".into());
    }
    if !m.figures.is_empty() && m.representative_figure.is_none() {
        errors.push("도면이 있으면 【대표도】를 지정합니다".into());
    }
    let numbers: Vec<u32> = m.figures.iter().map(|(n, _)| *n).collect();
    if numbers != (1..=numbers.len() as u32).collect::<Vec<_>>() {
        errors.push("도면 번호가 1부터 이어지지 않습니다".into());
    }
    // CP949 밖 글자 — 작성기는 `?` 로 바꿔 저장한다(⑳ 등).
    let mut bad: Vec<char> = Vec::new();
    for ch in text.chars() {
        let mut buf = [0_u8; 4];
        let (_, _, unmappable) = encoding_rs::EUC_KR.encode(ch.encode_utf8(&mut buf));
        if unmappable && !bad.contains(&ch) {
            bad.push(ch);
        }
    }
    if !bad.is_empty() {
        errors.push(format!(
            "CP949 밖 글자(작성기가 ?로 바꿉니다): {}",
            bad.iter()
                .map(char::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    if errors.is_empty() {
        Ok(m)
    } else {
        Err(manuscript_error("parse", errors.join(" | ")))
    }
}

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 300dpi 1비트 TIFF 의 작성기 표시 크기(mm) = floor(픽셀 ÷ 11.3). 실측 6값 모두 재현.
fn editor_mm(pixels: u32) -> u32 {
    pixels * 10 / 113
}

/// 원고 → NK-Editor 6.30 제출 XML. 그림 크기는 `image_size(경로) -> (가로, 세로 픽셀)` 로 받는다.
pub fn manuscript_to_kipo_xml(
    m: &Manuscript,
    document_id: &str,
    page_count: u32,
    mut image_size: impl FnMut(&str) -> Result<(u32, u32), CoreError>,
) -> Result<String, CoreError> {
    let mut x: Vec<String> = vec![
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>".into(),
        format!(
            "<KIPO keapsVersion=\"5.5\" editorKind=\"K\" pageCount=\"{page_count}\" imgApply=\"N\" xmlns=\"http://www.kipo.go.kr\">"
        ),
        format!("<PatentCAFDOC docflag=\"1.0\" documentID=\"{document_id}\">"),
        String::new(),
        "<description>".into(),
        format!("<invention-title>{}</invention-title>", esc(&m.title)),
    ];
    let mut n = 0_u32;
    let paras = |lines: &[String], n: &mut u32| -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                *n += 1;
                format!("<p num=\"{:04}\">{}</p>", *n, esc(l))
            })
            .collect()
    };
    let parent_of = |e: &str| {
        SECTIONS
            .iter()
            .find(|(_, el, _)| *el == e)
            .and_then(|(_, _, p)| *p)
    };
    let last = m.sections.len().saturating_sub(1);
    let mut i = 0;
    while i < m.sections.len() {
        let (el, lines) = &m.sections[i];
        if parent_of(el) == Some("summary-of-invention") {
            x.push("<summary-of-invention>".into());
            let start = i;
            while i < m.sections.len() && parent_of(m.sections[i].0) == Some("summary-of-invention")
            {
                i += 1;
            }
            for k in start..i {
                let (g, gl) = &m.sections[k];
                let tail = if k == i - 1 {
                    "</summary-of-invention>"
                } else {
                    ""
                };
                x.push(format!(
                    "<{g}>{}\n</{g}>{tail}",
                    paras(gl, &mut n).join("\n")
                ));
            }
            continue;
        }
        if *el == "citation-list" {
            x.push("<citation-list>".into());
            i += 1;
            let mut subs = Vec::new();
            while i < m.sections.len()
                && matches!(
                    m.sections[i].0,
                    "patent-literature" | "non-patent-literature"
                )
            {
                subs.push(i);
                i += 1;
            }
            for (j, k) in subs.iter().enumerate() {
                let (sub, sl) = &m.sections[*k];
                let (tag, label) = if *sub == "patent-literature" {
                    ("patcit", "특허문헌")
                } else {
                    ("nplcit", "비특허문헌")
                };
                n += 1;
                let cites: Vec<String> = sl
                    .iter()
                    .enumerate()
                    .map(|(c, line)| {
                        let body = line
                            .strip_prefix(&format!("({label} "))
                            .and_then(|r| r.split_once(") "))
                            .map_or(line.as_str(), |(_, t)| t);
                        format!(
                            "<{tag} num=\"{:04}\"><text>{}</text></{tag}>",
                            c + 1,
                            esc(body)
                        )
                    })
                    .collect();
                x.push(format!("<{sub}>"));
                x.push(format!("<p num=\"{n:04}\">{}\n</p>", cites.join("\n")));
                let tail = if j == subs.len() - 1 {
                    "</citation-list>"
                } else {
                    ""
                };
                x.push(format!("</{sub}>{tail}"));
            }
            continue;
        }
        let end = if i == last { "</description>" } else { "" };
        if matches!(*el, "description-of-drawings" | "reference-signs-list") {
            // 목록형 절: 줄 전부가 한 문단 · `<br/>` + CRLF 로 잇는다.
            n += 1;
            let joined = lines
                .iter()
                .map(|l| esc(l))
                .collect::<Vec<_>>()
                .join("<br/>\r\n");
            x.push(format!(
                "<{el}>\n<p num=\"{n:04}\">{joined}</p>\n</{el}>{end}"
            ));
        } else {
            x.push(format!(
                "<{el}>{}\n</{el}>{end}",
                paras(lines, &mut n).join("\n")
            ));
        }
        i += 1;
    }
    let claims: Vec<String> = m
        .claims
        .iter()
        .enumerate()
        .map(|(k, t)| {
            format!(
                "<claim num=\"{}\"><claim-text>{}</claim-text></claim>",
                k + 1,
                esc(t)
            )
        })
        .collect();
    x.push(format!("<claims>\n{}</claims>", claims.join("\n")));
    x.push("<abstract>".into());
    x.push("<summary>".into());
    let mut a = 0;
    for line in &m.abstract_lines {
        a += 1;
        x.push(format!("<p num=\"{a:04}a\">{}</p>", esc(line)));
    }
    match m.representative_figure {
        None => x.push("</summary>\n</abstract>".into()),
        Some(fig) => {
            x.push("</summary>".into());
            x.push("<abstract-figure>".into());
            x.push(format!(
                "<p num=\"{:04}a\"><figref num=\"{fig}\"/></p>",
                a + 1
            ));
            x.push("</abstract-figure></abstract>".into());
        }
    }
    if !m.figures.is_empty() {
        x.push("<drawings>".into());
        for (k, path) in &m.figures {
            let (w, h) = image_size(path)?;
            let ext = if path.to_ascii_lowercase().ends_with(".jpg")
                || path.to_ascii_lowercase().ends_with(".jpeg")
            {
                "jpg"
            } else {
                "tif"
            };
            x.push(format!(
                "<figure num=\"{k}\"><img id=\"i{k:04}\" he=\"{}\" wi=\"{}\" file=\"pat{k:05}.{ext}\" img-format=\"{ext}\"/></figure>",
                editor_mm(h),
                editor_mm(w)
            ));
        }
        x.push("</drawings>".into());
    }
    x.push("</PatentCAFDOC>".into());
    x.push("</KIPO>".into());
    // NK-Editor 6.30 줄바꿈: 머리 넉 줄 · 여는 태그만인 줄은 CRLF, 나머지 LF, 끝 줄바꿈 없음.
    let joined = x.join("\n");
    let lines: Vec<&str> = joined.split('\n').collect();
    let mut out = String::with_capacity(joined.len() + 64);
    for (k, line) in lines.iter().enumerate() {
        out.push_str(line);
        if k + 1 == lines.len() {
            break;
        }
        let lone_open = line.starts_with('<')
            && !line.starts_with("</")
            && line.ends_with('>')
            && !line.contains(' ')
            && line.matches('<').count() == 1;
        let crlf = k < 4 || lone_open || line.ends_with('\r');
        if line.ends_with('\r') {
            out.push('\n');
        } else {
            out.push_str(if crlf { "\r\n" } else { "\n" });
        }
    }
    Ok(out)
}

/// 원고 파일 → (XML, 그림 목록(XML 안 이름, 원본 경로)). 그림 경로는 원고 폴더 기준.
pub fn manuscript_path_to_kipo_xml(
    path: &Path,
    document_id: &str,
    page_count: u32,
) -> Result<(String, Vec<(String, std::path::PathBuf)>), CoreError> {
    let text = std::fs::read_to_string(path)
        .map_err(|_| manuscript_error("read", path.display().to_string()))?;
    let m = parse_manuscript(&text)?;
    let base = path.parent().unwrap_or(Path::new("."));
    let xml = manuscript_to_kipo_xml(&m, document_id, page_count, |p| {
        let image = image::image_dimensions(base.join(p))
            .map_err(|_| manuscript_error("image", format!("그림을 읽을 수 없습니다: {p}")))?;
        Ok(image)
    })?;
    let files = m
        .figures
        .iter()
        .map(|(k, p)| {
            let ext = if p.to_ascii_lowercase().ends_with(".jpg")
                || p.to_ascii_lowercase().ends_with(".jpeg")
            {
                "jpg"
            } else {
                "tif"
            };
            (format!("pat{k:05}.{ext}"), base.join(p))
        })
        .collect();
    Ok((xml, files))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_what_the_official_editor_rejects() {
        let text = "【명세서】\n【발명의 설명】\n【발명의 명칭】\n첫 줄\n둘째 줄\n【기술분야】\n【0001】 본문\n【발명을 실시하기 위한 구체적인 내용】\n하위 항(①~⑳)\n【청구범위】\n【청구항 1】\n청구항\n【요약서】\n【요약】\n요약\n【대표도】\n제 1도\n【도면】\n【도 1】\n[fig1.tif 삽입]\n";
        let error = parse_manuscript(text).unwrap_err();
        let issues = &error.details["issues"];
        for code in ["E-249", "E-262", "E-201", "E-242", "CP949"] {
            assert!(issues.contains(code), "{code} 없음: {issues}");
        }
    }
}
