use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use roxmltree::{Document, Node, ParsingOptions};

use super::{filename_title, invalid_text_char, make_book, Book};
use archive::{resolve_path, Archive};

mod archive;

const CONTAINER_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:container";
const OPF_NS: &str = "http://www.idpf.org/2007/opf";
const DC_NS: &str = "http://purl.org/dc/elements/1.1/";
const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const OPF_TYPE: &str = "application/oebps-package+xml";
const XHTML_TYPE: &str = "application/xhtml+xml";
const MAX_INDEX_BYTES: u64 = 1024 * 1024;
const MAX_XHTML_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SPINE_ITEMS: usize = 8192;
const MAX_XML_NODES: u32 = 150_000;
const MAX_LABEL_CHARS: usize = 200;

/// Return a complete snapshot; no chapter lookup depends on the original EPUB.
pub(in crate::library) fn import_epub(path: &Path) -> Result<(Book, Vec<String>)> {
    let mut archive = Archive::open(path)?;
    let container = archive.read_xml("META-INF/container.xml", MAX_INDEX_BYTES)?;
    let package_path = package_path(&container)?;
    let package_xml = archive.read_xml(&package_path, MAX_INDEX_BYTES)?;
    let package = parse_xml(&package_xml, false).context("EPUB OPF 无效")?;
    let root = document_element(&package, OPF_NS, "package")?;
    let metadata = child(root, "metadata");
    let title = metadata
        .and_then(|node| metadata_label(node, "title"))
        .unwrap_or_else(|| filename_title(path));
    let author = metadata
        .and_then(|node| metadata_label(node, "creator"))
        .unwrap_or_default();
    let base = package_path.rsplit_once('/').map_or("", |(base, _)| base);
    let spine = spine_paths(root, base)?;
    let mut sections = Vec::new();
    for (index, name) in spine.iter().enumerate() {
        let xhtml = archive.read_xml(name, MAX_XHTML_BYTES)?;
        if let Some((heading, text)) = chapter(&xhtml, index + 1)? {
            sections.push((heading, text));
        }
    }
    if sections.is_empty() {
        bail!("EPUB 没有可阅读的正文");
    }
    let (mut book, bodies) = make_book(
        title,
        "local",
        "epub",
        "本地 EPUB 完整快照；阅读不依赖原始文件。",
        sections,
    );
    book.author = author;
    book.validate()?;
    Ok((book, bodies))
}

fn package_path(container: &str) -> Result<String> {
    let document = parse_xml(container, false).context("EPUB container.xml 无效")?;
    let root = document_element(&document, CONTAINER_NS, "container")?;
    let files = child(root, "rootfiles").context("EPUB 缺少根目录清单")?;
    let file = files
        .children()
        .find(|node| {
            tagged(*node, root.tag_name().namespace(), "rootfile")
                && matches!(node.attribute("media-type"), Some(OPF_TYPE) | None)
        })
        .context("EPUB 缺少 OPF 根文件")?;
    let path = file.attribute("full-path").context("EPUB 缺少 OPF 路径")?;
    resolve_path("", path)
}

fn spine_paths(package: Node<'_, '_>, base: &str) -> Result<Vec<String>> {
    let manifest = child(package, "manifest").context("EPUB OPF 缺少资源清单")?;
    let spine = child(package, "spine").context("EPUB OPF 缺少阅读顺序")?;
    let namespace = package.tag_name().namespace();
    let mut by_id = HashMap::new();
    for item in manifest
        .children()
        .filter(|node| tagged(*node, namespace, "item"))
    {
        let id = item
            .attribute("id")
            .filter(|id| !id.is_empty())
            .context("EPUB 资源缺少标识")?;
        if by_id.insert(id, item).is_some() {
            bail!("EPUB 资源清单含重复标识");
        }
    }
    let mut paths = Vec::new();
    let items = spine
        .children()
        .filter(|node| tagged(*node, namespace, "itemref"));
    for (index, itemref) in items.enumerate() {
        if index >= MAX_SPINE_ITEMS {
            bail!("EPUB 阅读顺序条目过多");
        }
        if itemref.attribute("linear") == Some("no") {
            continue;
        }
        let id = itemref
            .attribute("idref")
            .context("EPUB 阅读顺序缺少资源标识")?;
        let item = by_id.get(id).context("EPUB 阅读顺序引用了未声明的资源")?;
        if item.attribute("media-type") != Some(XHTML_TYPE) {
            bail!("EPUB 主阅读顺序仅支持 XHTML 正文");
        }
        if [package, manifest, *item]
            .iter()
            .any(|node| node.attribute((XML_NS, "base")).is_some())
        {
            bail!("EPUB OPF 不支持 xml:base 路径覆盖");
        }
        let href = item.attribute("href").context("EPUB XHTML 资源缺少路径")?;
        paths.push(resolve_path(base, href)?);
    }
    Ok(paths)
}

fn parse_xml(input: &str, allow_doctype: bool) -> Result<Document<'_>> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    // Permit legacy XHTML DOCTYPE declarations, but never resolve external DTDs or expand custom entities.
    if allow_doctype && input.contains("<!ENTITY") {
        bail!("EPUB XHTML 不支持自定义 XML 实体");
    }
    let options = ParsingOptions {
        allow_dtd: allow_doctype,
        nodes_limit: MAX_XML_NODES,
        entity_resolver: None,
    };
    Document::parse_with_options(input, options)
        .map_err(|_| anyhow::anyhow!("XML 格式无效或节点数量超出上限"))
}

fn document_element<'a, 'input>(
    document: &'a Document<'input>,
    namespace: &str,
    name: &str,
) -> Result<Node<'a, 'input>> {
    let root = document.root_element();
    if !tagged(root, Some(namespace), name) && !tagged(root, None, name) {
        bail!("EPUB XML 缺少预期的根元素");
    }
    Ok(root)
}

fn tagged(node: Node<'_, '_>, namespace: Option<&str>, name: &str) -> bool {
    node.tag_name().namespace() == namespace && node.has_tag_name(name)
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|candidate| tagged(*candidate, node.tag_name().namespace(), name))
}

fn metadata_label(node: Node<'_, '_>, name: &str) -> Option<String> {
    node.children()
        .filter(|child| {
            child.has_tag_name((DC_NS, name))
                || (node.tag_name().namespace().is_none() && tagged(*child, None, name))
        })
        .find_map(node_label)
}

fn node_label(node: Node<'_, '_>) -> Option<String> {
    let text = node
        .descendants()
        .filter(Node::is_text)
        .filter_map(|node| node.text());
    label(text.flat_map(str::chars))
}

fn label(chars: impl Iterator<Item = char>) -> Option<String> {
    let mut text = Readable::default();
    text.push(chars);
    let mut label: String = text.output.chars().take(MAX_LABEL_CHARS).collect();
    label.truncate(label.trim_end().len());
    (!label.is_empty()).then_some(label)
}

fn chapter(xhtml: &str, position: usize) -> Result<Option<(String, String)>> {
    let document = parse_xml(xhtml, true).context("EPUB XHTML 正文无效")?;
    let html = document_element(&document, XHTML_NS, "html")?;
    let body = child(html, "body").context("EPUB XHTML 缺少正文元素")?;
    let (heading, text) = render_body(body);
    if text.len() as u64 > MAX_XHTML_BYTES {
        bail!("EPUB XHTML 提取后的正文超过大小上限");
    }
    if text.is_empty() {
        return Ok(None);
    }
    let title = heading
        .or_else(|| {
            child(html, "head")
                .and_then(|head| child(head, "title"))
                .and_then(node_label)
        })
        .unwrap_or_else(|| format!("第 {position} 章"));
    Ok(Some((title, text)))
}

fn render_body(body: Node<'_, '_>) -> (Option<String>, String) {
    let namespace = body.tag_name().namespace();
    let mut current = body;
    let mut entering = true;
    let mut content = Readable::default();
    let mut heading = None;
    let mut heading_start = None;
    // Walk iteratively so hostile markup depth cannot grow this function's call stack.
    loop {
        let name = current.tag_name().name();
        let visible = !current.is_element()
            || (current.tag_name().namespace() == namespace
                && !matches!(
                    name,
                    "script"
                        | "style"
                        | "noscript"
                        | "template"
                        | "head"
                        | "svg"
                        | "math"
                        | "rt"
                        | "rp"
                )
                && current.attribute("hidden").is_none());
        let is_heading = matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6");
        let block = is_heading
            || matches!(
                name,
                "address"
                    | "article"
                    | "aside"
                    | "blockquote"
                    | "dd"
                    | "div"
                    | "dl"
                    | "dt"
                    | "figcaption"
                    | "figure"
                    | "footer"
                    | "header"
                    | "hr"
                    | "li"
                    | "main"
                    | "nav"
                    | "ol"
                    | "p"
                    | "pre"
                    | "section"
                    | "table"
                    | "tr"
                    | "ul"
            );
        if visible {
            if entering {
                if block {
                    content.gap = 3;
                } else if name == "br" {
                    content.gap = content.gap.max(2);
                }
                if is_heading && heading.is_none() && heading_start.is_none() {
                    heading_start = Some((current, content.output.len()));
                }
                if current.is_text() {
                    content.push(current.text().unwrap_or_default().chars());
                }
                if let Some(child) = current.first_child() {
                    current = child;
                    continue;
                }
            }
            if let Some((node, start)) = heading_start {
                if current == node {
                    heading = label(content.output[start..].chars());
                    heading_start = None;
                }
            }
            if block {
                content.gap = 3;
            }
        }
        if current == body {
            break;
        }
        if let Some(sibling) = current.next_sibling() {
            current = sibling;
            entering = true;
        } else if let Some(parent) = current.parent() {
            current = parent;
            entering = false;
        } else {
            break;
        }
    }
    (heading, content.output)
}

#[derive(Default)]
struct Readable {
    output: String,
    // 0: no gap, 1: inline space, 2: line break, 3: paragraph break.
    gap: u8,
}

impl Readable {
    fn push(&mut self, chars: impl Iterator<Item = char>) {
        for ch in chars {
            if invalid_text_char(ch) {
                continue;
            }
            if ch.is_whitespace() {
                self.gap = self.gap.max(1);
                continue;
            }
            if !self.output.is_empty() {
                self.output.push_str(match self.gap {
                    1 => " ",
                    2 => "\n",
                    3 => "\n\n",
                    _ => "",
                });
            }
            self.gap = 0;
            self.output.push(ch);
        }
    }
}

#[cfg(test)]
mod tests;
