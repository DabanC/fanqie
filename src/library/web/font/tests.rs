use super::*;

fn page(text: &str) -> String {
    format!(
        r#"<style>
            @font-face {{ font-family: DNMrHsV173Pd4pgy; src: url('https://lf6-awef.bytetos.com/obj/awesome-font/c/dc027189e0ba4cd.woff2'); }}
            .reader {{ font-family: 'DNMrHsV173Pd4pgy', sans-serif; }}
            </style><div class="reader"><p>{text}</p></div>"#
    )
}

#[test]
fn known_snippet() {
    let text = "乾皇朝，疆域亿。";
    assert_eq!(decode(&page(text), text).unwrap(), "大乾皇朝，疆域亿万里。");
}

#[test]
fn corrected_entries() {
    let codes = [
        58527, 58536, 58554, 58577, 58589, 58593, 58606, 58620, 58652, 58676,
    ];
    let text: String = codes
        .into_iter()
        .map(|code| char::from_u32(code).unwrap())
        .collect();
    assert_eq!(decode(&page(&text), &text).unwrap(), "意点本给行动名时情己");
}

#[test]
fn exact_table_coverage() {
    assert_eq!(MAPPING.len(), 362);
    assert!(MAPPING.windows(2).all(|pair| pair[0].0 < pair[1].0));
    let encoded: String = MAPPING
        .iter()
        .map(|&(code, _)| char::from_u32(code).unwrap())
        .collect();
    let expected: String = MAPPING.iter().map(|&(_, ch)| ch).collect();
    let decoded = decode(&page(&encoded), &encoded).unwrap();
    assert_eq!(decoded, expected);
    assert!(!decoded.chars().any(private_use));
}

#[test]
fn plaintext_is_unchanged_without_font_checks() {
    for text in [
        "",
        "普通正文\n A1，😀。",
        "\u{d7ff}\u{f900}\u{effff}\u{ffffe}\u{10fffe}",
    ] {
        assert_eq!(decode("not even HTML", text).unwrap(), text);
    }
}

#[test]
fn all_pua_ranges_are_rejected_when_unmapped() {
    for ch in [
        '\u{e000}',
        '\u{e3e7}',
        '\u{e3f6}',
        '\u{f8ff}',
        '\u{f0000}',
        '\u{ffffd}',
        '\u{100000}',
        '\u{10fffd}',
    ] {
        let text = format!("正常{ch}");
        assert!(decode(&page(&text), &text)
            .unwrap_err()
            .to_string()
            .contains("未识别"));
    }
}

#[test]
fn unknown_or_mismatched_font_is_rejected() {
    let text = "";
    let valid = page(text);
    for invalid in [
        String::new(),
        valid.replace(FAMILY, "otherFont"),
        valid.replace("dc027189e0ba4cd", "unknown"),
        valid.replace(".reader {", ".unused {"),
        valid.replace("class=\"reader\"", "class=\"other\""),
        valid.replace("<p>", "<p style=\"font-family: otherFont\">"),
        valid.replace(".woff2", ".woff2.bad"),
        valid.replace("lf6-awef.bytetos.com", "example.com"),
        valid.replace("<p>", "<p style=\"font-family: '\">"),
        valid.replace(".woff2');", ".woff2'), local('Other');"),
        valid.replace(
            ".woff2');",
            ".woff2'), url('https://example.com/other.woff2');",
        ),
        valid.replace(".woff2');", ".woff2') junk;"),
        valid.replace(".woff2');", ".woff2'broken);"),
        valid
            .replace(".woff2');", ".woff2') format(\"woff2\");")
            .replace("<p>", "<p style=\"font: 12px otherFont\">"),
        valid.replace(
            "font-family: 'DNMrHsV173Pd4pgy', sans-serif",
            "font-family: sans-serif, 'DNMrHsV173Pd4pgy'",
        ),
    ] {
        assert!(decode(&invalid, text).is_err(), "accepted {invalid}");
    }
}

#[test]
fn inline_usage_and_known_weighted_assets() {
    let text = "";
    for suffix in ["", "-500", "-700"] {
        let html = page(text)
            .replace(
                "dc027189e0ba4cd.woff2",
                &format!("dc027189e0ba4cd{suffix}.woff"),
            )
            .replace(
                ".reader { font-family: 'DNMrHsV173Pd4pgy', sans-serif; }",
                "",
            )
            .replace("<p>", "<p style=\"font-family: DNMrHsV173Pd4pgy\">");
        assert_eq!(decode(&html, text).unwrap(), "大");
    }
}

#[test]
fn mere_markers_comments_and_scripts_are_not_declarations() {
    let text = "";
    for html in [
        format!("<!-- {} -->", page(text)),
        format!("<script>{}</script>", page(text)),
        format!("<template>{}</template>", page(text)),
        page(text).replace("<style>", "<style media=\"not all\">"),
        page(text).replace("<style>", "<style type=\"text/plain\">"),
        format!("<div data-font=\"{FAMILY}\" data-asset=\"dc027189e0ba4cd\">{text}</div>"),
        page(text)
            .replace(".reader {", "/* .reader {")
            .replace("sans-serif; }", "sans-serif; } */"),
        page(text)
            .replace(".reader {", "@media not all { .reader {")
            .replace("sans-serif; }", "sans-serif; } }"),
    ] {
        assert!(decode(&html, text).is_err());
    }
    assert!(decode(&page("正文"), text).is_err());
}
