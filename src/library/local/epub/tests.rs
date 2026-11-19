use std::fs::File;
use std::io::{Cursor, Write};
use std::path::Path;

use tempfile::tempdir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::*;

const CONTAINER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<c:container xmlns:c="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
  <c:rootfiles>
    <c:rootfile full-path="EPUB/index/book.opf" media-type="application/oebps-package+xml"/>
  </c:rootfiles>
</c:container>"#;

fn package(manifest: &str, spine: &str) -> String {
    format!(
        r#"<package xmlns="{OPF_NS}" xmlns:dc="{DC_NS}" version="3.0">
            <metadata><dc:title>海 &amp; 山</dc:title><dc:creator>张&#x20;三</dc:creator></metadata>
            <manifest>{manifest}</manifest><spine>{spine}</spine>
        </package>"#
    )
}

fn item(id: &str, href: &str) -> String {
    format!(r#"<item id="{id}" href="{href}" media-type="{XHTML_TYPE}"/>"#)
}

fn html(title: &str, body: &str) -> String {
    format!(
        r#"<html xmlns="{XHTML_NS}"><head><title>{title}</title></head><body>{body}</body></html>"#
    )
}

fn fixture(opf: &str, chapters: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let options = stored.compression_method(method);
    writer.start_file("mimetype", stored).unwrap();
    writer.write_all(b"application/epub+zip").unwrap();
    for (name, body) in chapters {
        writer.start_file(*name, options).unwrap();
        writer.write_all(body).unwrap();
    }
    for (name, bytes) in [
        ("META-INF/container.xml", CONTAINER.as_bytes()),
        ("EPUB/index/book.opf", opf.as_bytes()),
    ] {
        writer.start_file(name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn write_fixture(path: &Path, opf: &str, chapters: &[(&str, &[u8])], method: CompressionMethod) {
    std::fs::write(path, fixture(opf, chapters, method)).unwrap();
}

#[test]
fn imports_spine_in_order_with_metadata_and_readable_xhtml() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("display-name.epub");
    let manifest = format!(
        "{}{}{}{}",
        item("alpha", "alpha.xhtml"),
        item("beta", "../text/%E6%B5%B7%20wind.xhtml"),
        item("cover", "cover.xhtml"),
        item("notes", "https://example.invalid/never-read.xhtml")
    );
    let opf = package(
        &manifest,
        r#"<itemref idref="cover"/><itemref idref="beta"/><itemref idref="notes" linear="no"/><itemref idref="alpha"/>"#,
    );
    let alpha = html("低潮", "<div><p>船靠岸。</p><p>返航。</p></div>");
    let beta = format!(
        "\u{feff}<?xml version='1.0' encoding='UTF-8'?><!DOCTYPE html SYSTEM 'https://example.invalid/xhtml.dtd'>{}",
        html(
            "备用名字",
            "<section><h2>风<span>起</span></h2><p>甲 &amp; 乙<br/>下一行&#160;有风。</p>\n\
             <p><ruby>海<rt>hǎi</rt></ruby>港<span hidden='hidden'>不显示</span>。&#x202e;</p>\
             <script>not reader text</script><style>not reader text</style><template><p>hidden</p></template>\
             <svg xmlns='http://www.w3.org/2000/svg'><text>svg label</text></svg></section>"
        )
    );
    let cover = html("封面", "<img src='missing.jpg' alt='封面'/>");
    write_fixture(
        &path,
        &opf,
        &[
            ("EPUB/index/alpha.xhtml", alpha.as_bytes()),
            ("EPUB/text/海 wind.xhtml", beta.as_bytes()),
            ("EPUB/index/cover.xhtml", cover.as_bytes()),
        ],
        CompressionMethod::Deflated,
    );
    let (book, bodies) = import_epub(&path).unwrap();
    book.validate().unwrap();
    assert_eq!(book.title, "海 & 山");
    assert_eq!(book.author, "张 三");
    assert_eq!(book.source, "local");
    assert_eq!(book.chapters.len(), 2);
    assert_eq!(book.chapters[0].title, "风起");
    assert_eq!(book.chapters[1].title, "低潮");
    assert_eq!(
        bodies,
        [
            "风起\n\n甲 & 乙\n下一行 有风。\n\n海港。",
            "船靠岸。\n\n返航。"
        ]
    );
}

#[test]
fn opaque_snapshot_ids_survive_renames_and_repacking() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("initial.epub");
    let repacked = directory.path().join("new filename.epub");
    let opf = package(
        &item("some-private-id", "one.xhtml"),
        r#"<itemref idref="some-private-id"/>"#,
    );
    let first = html("", "<p>不变的正文。</p>");
    write_fixture(
        &path,
        &opf,
        &[("EPUB/index/one.xhtml", first.as_bytes())],
        CompressionMethod::Stored,
    );
    let second_opf = package(
        &item("another-id", "new.xhtml"),
        r#"<itemref idref="another-id"/>"#,
    );
    write_fixture(
        &repacked,
        &second_opf,
        &[("EPUB/index/new.xhtml", first.as_bytes())],
        CompressionMethod::Deflated,
    );
    let (book, bodies) = import_epub(&path).unwrap();
    let (same, same_bodies) = import_epub(&repacked).unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(book.id.starts_with("epub:"));
    assert_eq!(book.id.len(), "epub:".len() + 64);
    assert_eq!(book, same);
    assert_eq!(bodies, same_bodies);
    assert_eq!(book.chapters[0].title, "第 1 章");
    assert_eq!(bodies, ["不变的正文。"]);

    let changed = html("", "<p>变化后的正文。</p>");
    write_fixture(
        &repacked,
        &second_opf,
        &[("EPUB/index/new.xhtml", changed.as_bytes())],
        CompressionMethod::Stored,
    );
    assert_ne!(book.id, import_epub(&repacked).unwrap().0.id);
}

#[test]
fn rejects_unsafe_references_missing_spine_resources_and_invalid_text() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("bad.epub");
    let valid = html("", "<p>正文。</p>");
    for href in [
        "https://example.invalid/one.xhtml",
        "//example.invalid/one.xhtml",
        "/EPUB/index/one.xhtml",
        "../../../one.xhtml",
        "%2e%2e/%2e%2e/%2e%2e/one.xhtml",
        "some%2fone.xhtml",
        "%5cone.xhtml",
        "one%00.xhtml",
        "one%ZZ.xhtml",
        "absent.xhtml",
    ] {
        let opf = package(&item("one", href), r#"<itemref idref="one"/>"#);
        write_fixture(
            &path,
            &opf,
            &[("EPUB/index/one.xhtml", valid.as_bytes())],
            CompressionMethod::Stored,
        );
        assert!(import_epub(&path).is_err(), "unexpectedly accepted {href}");
    }
    for body in [
        b"\xff".as_slice(),
        b"<html><body><p>unclosed</body></html>",
        b"<!DOCTYPE html [<!ENTITY x SYSTEM 'file:///does-not-exist'>]><html><body>&x;</body></html>",
        b"<html><body><script>ignored</script><p>   </p></body></html>",
    ] {
        let opf = package(&item("one", "one.xhtml"), r#"<itemref idref="one"/>"#);
        write_fixture(&path, &opf, &[("EPUB/index/one.xhtml", body)], CompressionMethod::Stored);
        assert!(import_epub(&path).is_err());
    }
    let opf = package(
        &item("one", "one.xhtml"),
        r#"<itemref idref="undeclared"/>"#,
    );
    write_fixture(
        &path,
        &opf,
        &[("EPUB/index/one.xhtml", valid.as_bytes())],
        CompressionMethod::Stored,
    );
    assert!(import_epub(&path).is_err());
}

#[test]
fn enforces_package_member_and_expansion_limits() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("size.epub");
    assert!(import_epub(directory.path()).is_err());
    File::create(&path)
        .unwrap()
        .set_len(32 * 1024 * 1024 + 1)
        .unwrap();
    assert!(import_epub(&path).is_err());

    let opf = package(&item("one", "one.xhtml"), r#"<itemref idref="one"/>"#);
    let huge = html("", &"x".repeat(MAX_XHTML_BYTES as usize));
    write_fixture(
        &path,
        &opf,
        &[("EPUB/index/one.xhtml", huge.as_bytes())],
        CompressionMethod::Deflated,
    );
    assert!(import_epub(&path).unwrap_err().to_string().contains("上限"));

    let valid = html("", "<p>正文。</p>");
    let entries = [
        ("EPUB/index/one.xhtml", valid.as_bytes()),
        ("../unused", b"bad".as_slice()),
    ];
    write_fixture(&path, &opf, &entries, CompressionMethod::Stored);
    assert!(import_epub(&path).is_err());

    let mut bytes = fixture(
        &opf,
        &[("EPUB/index/one.xhtml", valid.as_bytes())],
        CompressionMethod::Stored,
    );
    let end = bytes.len() - 22;
    bytes[end + 8..end + 12].copy_from_slice(&[0xff; 4]);
    std::fs::write(&path, bytes).unwrap();
    assert!(import_epub(&path).is_err());
}

#[test]
fn rejects_a_symlink_as_a_spine_document() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("symlink.epub");
    let valid = html("", "<p>正文。</p>");
    let opf = package(&item("one", "one.xhtml"), r#"<itemref idref="one"/>"#);
    let bytes = fixture(
        &opf,
        &[("EPUB/index/target.xhtml", valid.as_bytes())],
        CompressionMethod::Stored,
    );
    let mut writer = ZipWriter::new_append(Cursor::new(bytes)).unwrap();
    writer
        .add_symlink(
            "EPUB/index/one.xhtml",
            "target.xhtml",
            SimpleFileOptions::default(),
        )
        .unwrap();
    std::fs::write(&path, writer.finish().unwrap().into_inner()).unwrap();
    assert!(import_epub(&path).is_err());
}
