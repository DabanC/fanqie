//! Fail-closed decoding of the one supported Fanqie web font.
mod mapping;
#[cfg(test)]
mod tests;

use anyhow::{bail, Result};
use mapping::MAPPING;
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::collections::HashSet;

const FAMILY: &str = "DNMrHsV173Pd4pgy";

/// Decode extracted paragraphs, using only font declarations in the supplied HTML.
/// Plain text needs no font metadata. PUA text requires both the known asset and
/// a matching font-family rule on its HTML element (or an ancestor). No scripts,
/// network requests, external processes, or best-effort substitutions are used.
pub(super) fn decode(page_html: &str, text: &str) -> Result<String> {
    if !text.chars().any(private_use) {
        return Ok(text.to_owned());
    }
    let covered = recognized_characters(page_html)?;
    let mut decoded = String::with_capacity(text.len());
    for ch in text.chars() {
        if !private_use(ch) {
            decoded.push(ch);
            continue;
        }
        let replacement = MAPPING
            .binary_search_by_key(&(ch as u32), |&(code, _)| code)
            .ok()
            .map(|index| MAPPING[index].1);
        let Some(replacement) = replacement else {
            bail!("正文含未识别的字体字符 U+{:04X}，拒绝返回乱码", ch as u32);
        };
        if !covered.contains(&ch) {
            bail!("正文字符缺少已知字体的实际使用声明，拒绝解码");
        }
        decoded.push(replacement);
    }
    if decoded.chars().any(private_use) {
        bail!("字体解码后仍含私用区字符，拒绝返回乱码");
    }
    Ok(decoded)
}

fn private_use(ch: char) -> bool {
    matches!(ch as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
}

// This deliberately accepts the site's simple rules, not arbitrary CSS. Unknown
// or ambiguous font declarations fail closed rather than guessing the cascade.
fn family(declarations: &str) -> Result<Option<String>> {
    let mut result = None;
    for declaration in declarations.split(';') {
        let Some((name, value)) = declaration.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("font") {
            bail!("网页包含不支持的字体简写声明，拒绝解码");
        }
        if !name.trim().eq_ignore_ascii_case("font-family") {
            continue;
        }
        let value = value.trim();
        let value = value.strip_suffix("!important").unwrap_or(value).trim();
        let first = value.split(',').next().unwrap_or("").trim();
        let first = if first.len() >= 2
            && ((first.starts_with('\'') && first.ends_with('\''))
                || (first.starts_with('"') && first.ends_with('"')))
        {
            &first[1..first.len() - 1]
        } else {
            first
        };
        if result.as_deref().is_some_and(|old| old != first) {
            bail!("网页字体声明互相冲突，拒绝解码");
        }
        result = Some(first.to_owned());
    }
    Ok(result)
}

fn known_asset(declarations: &str) -> bool {
    let mut found = false;
    let urls = Regex::new(
        r#"(?i)^url\(\s*(?:"([^"]+)"|'([^']+)'|([^\s"'()]+))\s*\)(?:\s*format\(\s*(?:"(?:woff2?|opentype)"|'(?:woff2?|opentype)')\s*\))?\s*$"#,
    ).unwrap();
    for declaration in declarations.split(';') {
        let Some((name, value)) = declaration.split_once(':') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("src") {
            continue;
        }
        for source in value.split(',') {
            let Some(capture) = urls.captures(source.trim()) else {
                return false;
            };
            let address = capture
                .get(1)
                .or_else(|| capture.get(2))
                .or_else(|| capture.get(3))
                .unwrap()
                .as_str();
            let Ok(url) = url::Url::parse(address) else {
                return false;
            };
            if url.scheme() != "https"
                || !matches!(
                    url.host_str(),
                    Some("lf3-awef.bytetos.com" | "lf6-awef.bytetos.com")
                )
                || !url.username().is_empty()
                || url.password().is_some()
                || url.port().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return false;
            }
            let Some(filename) = url.path().strip_prefix("/obj/awesome-font/c/") else {
                return false;
            };
            let Some((stem, extension)) = filename.rsplit_once('.') else {
                return false;
            };
            if !matches!(
                stem,
                "dc027189e0ba4cd" | "dc027189e0ba4cd-500" | "dc027189e0ba4cd-700"
            ) || !matches!(extension, "woff2" | "woff" | "otf")
            {
                return false;
            }
        }
        found = true;
    }
    found
}

fn recognized_characters(page_html: &str) -> Result<HashSet<char>> {
    let html = Html::parse_document(page_html);
    let comments = Regex::new(r"(?s)/\*.*?\*/").unwrap();
    let mut rules = Vec::new();
    let mut declared = false;
    for style in html.select(&Selector::parse("style").unwrap()) {
        if style
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|element| matches!(element.value().name(), "template" | "noscript"))
            || style.value().attr("type").is_some_and(|kind| {
                !kind.trim().is_empty() && !kind.trim().eq_ignore_ascii_case("text/css")
            })
        {
            continue;
        }
        let css = style.text().collect::<String>();
        let css = comments.replace_all(&css, "");
        if style.value().attr("media").is_some_and(|media| {
            !media.trim().is_empty() && !media.trim().eq_ignore_ascii_case("all")
        }) {
            if css.to_ascii_lowercase().contains("font") {
                bail!("网页字体依赖不支持的媒体条件，拒绝解码");
            }
            continue;
        }
        let mut remaining = css.as_ref();
        while let Some(open) = remaining.find('{') {
            let selector = remaining[..open].trim();
            let mut depth = 1;
            let mut end = None;
            let mut nested = false;
            for (offset, ch) in remaining[open + 1..].char_indices() {
                match ch {
                    '{' => {
                        depth += 1;
                        nested = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    end = Some(open + 1 + offset);
                    break;
                }
            }
            let Some(end) = end else {
                bail!("网页字体样式不完整，拒绝解码");
            };
            let declarations = &remaining[open + 1..end];
            remaining = &remaining[end + 1..];
            if nested || (selector.starts_with('@') && !selector.eq_ignore_ascii_case("@font-face"))
            {
                if declarations.to_ascii_lowercase().contains("font") {
                    bail!("网页包含不支持的嵌套字体样式，拒绝解码");
                }
                continue;
            }
            let Some(font) = family(declarations)? else {
                continue;
            };
            if selector.eq_ignore_ascii_case("@font-face") {
                if font == FAMILY {
                    if !known_asset(declarations) {
                        bail!("网页字体资源与已知字体不匹配，拒绝解码");
                    }
                    declared = true;
                }
            } else {
                let selector = Selector::parse(selector)
                    .map_err(|_| anyhow::anyhow!("无法识别网页字体选择器，拒绝解码"))?;
                rules.push((selector, font));
            }
        }
    }
    if !declared {
        bail!("网页未声明已知字体及资源，拒绝解码");
    }

    let mut covered = HashSet::new();
    for node in html.tree.nodes() {
        let Some(value) = node.value().as_text() else {
            continue;
        };
        if !value.chars().any(private_use) {
            continue;
        }
        let ancestors: Vec<_> = node.ancestors().filter_map(ElementRef::wrap).collect();
        if ancestors.iter().any(|element| {
            matches!(
                element.value().name(),
                "script" | "style" | "template" | "noscript"
            )
        }) {
            continue;
        }
        let mut used = false;
        for element in ancestors {
            let inline = element.value().attr("style").unwrap_or("");
            let inline = comments.replace_all(inline, "");
            let mut fonts = Vec::new();
            if let Some(font) = family(&inline)? {
                fonts.push(font);
            }
            for (selector, font) in &rules {
                if selector.matches(&element) {
                    fonts.push(font.clone());
                }
            }
            if fonts.is_empty() {
                continue;
            }
            if fonts.iter().any(|font| font != FAMILY) {
                bail!("正文使用未知或冲突的字体，拒绝解码");
            }
            used = true;
            break;
        }
        if !used {
            bail!("正文未实际使用已知字体，拒绝解码");
        }
        covered.extend(value.chars().filter(|&ch| private_use(ch)));
    }
    Ok(covered)
}
