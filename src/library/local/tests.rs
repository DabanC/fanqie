use super::*;
use std::fs;
use tempfile::tempdir;

fn import_bytes(bytes: &[u8]) -> Result<(Book, Vec<String>)> {
    let directory = tempdir()?;
    let path = directory.path().join("测试 小说.txt");
    fs::write(&path, bytes)?;
    import_txt(&path)
}

#[test]
fn chinese_headings_keep_preface_and_titles() {
    let input = "这是序言。\n还有一段。\n\n第十二章 风从河上来\n正文甲。\n第十三节 夜航\n正文乙。\n第十四回 归来\n正文丙。\n第二卷 新路\n正文丁。";
    let (book, bodies) = import_bytes(input.as_bytes()).unwrap();
    assert_eq!(book.source, "local");
    assert_eq!(book.title, "测试 小说");
    assert_eq!(book.chapters.len(), 5);
    assert_eq!(book.chapters[0].title, "前言");
    assert_eq!(book.chapters[1].title, "第十二章 风从河上来");
    assert_eq!(book.chapters[2].title, "第十三节 夜航");
    assert_eq!(book.chapters[3].title, "第十四回 归来");
    assert_eq!(book.chapters[4].title, "第二卷 新路");
    assert_eq!(
        bodies,
        [
            "这是序言。\n还有一段。",
            "正文甲。",
            "正文乙。",
            "正文丙。",
            "正文丁。"
        ]
    );
}

#[test]
fn indented_and_tabbed_headings_produce_valid_metadata() {
    let input = "第  十 二  章\t河灯\n正文。\n\tChapter\tIV:\tCrossing\nAnother body.";
    let (book, bodies) = import_bytes(input.as_bytes()).unwrap();
    book.validate().unwrap();
    assert_eq!(book.chapters.len(), 2);
    assert_eq!(book.chapters[0].title, "第 十 二 章 河灯");
    assert_eq!(book.chapters[1].title, "Chapter IV: Crossing");
    assert_eq!(bodies, ["正文。", "Another body."]);
}

#[test]
fn english_numbers_roman_numerals_and_words() {
    let input = "Chapter 1: Arrival\nA river.\nCHAPTER IV — Crossing\nA bridge.\nChapter One The Lamp\nA light.\nChapter Twenty-One: Home\nA house.";
    let (book, bodies) = import_bytes(input.as_bytes()).unwrap();
    assert_eq!(book.chapters.len(), 4);
    assert_eq!(book.chapters[0].title, "Chapter 1: Arrival");
    assert_eq!(book.chapters[1].title, "CHAPTER IV — Crossing");
    assert_eq!(book.chapters[2].title, "Chapter One The Lamp");
    assert_eq!(book.chapters[3].title, "Chapter Twenty-One: Home");
    assert_eq!(bodies, ["A river.", "A bridge.", "A light.", "A house."]);
}

#[test]
fn utf8_bom_gb18030_and_line_endings_have_equal_identity() {
    let text = "第十二章 相逢\n河岸的灯亮了。\n第二节 远行\n他们沿着小路回家。";
    let utf8 = import_bytes(text.as_bytes()).unwrap();
    let mut bom = b"\xef\xbb\xbf".to_vec();
    bom.extend_from_slice(text.replace('\n', "\r\n").as_bytes());
    let with_bom = import_bytes(&bom).unwrap();
    let (gb, _, errors) = GB18030.encode(text);
    assert!(!errors);
    let legacy = import_bytes(&gb).unwrap();
    for other in [with_bom, legacy] {
        assert_eq!(utf8.0.id, other.0.id);
        assert_eq!(utf8.1, other.1);
        assert_eq!(utf8.0.chapters[1].id, other.0.chapters[1].id);
    }
}

#[test]
fn rejects_binary_controls_invalid_encodings_and_empty_input() {
    for bytes in [
        b"".as_slice(),
        b" \t\r\n",
        b"hello\0world",
        b"hello\x1b[31m",
        b"hello\x7f",
        b"\xff",
        b"\x81",
        b"\xef\xbb\xbf\xff",
        b"%PDF-1.7\nplain looking data",
        b"GIF89aonly printable bytes",
        b"PK\x03\x04data",
        b"\xff\xfeA\0",
        "正文\u{0085}尾部".as_bytes(),
        "正文\u{202e}尾部".as_bytes(),
        "\u{feff}".as_bytes(),
    ] {
        assert!(
            import_bytes(bytes).is_err(),
            "unexpectedly accepted {bytes:?}"
        );
    }
}

#[test]
fn input_limit_and_non_files_are_rejected() {
    let directory = tempdir().unwrap();
    assert!(import_txt(directory.path()).is_err());
    assert!(import_txt(&directory.path().join("missing.txt")).is_err());
    let path = directory.path().join("too-large.txt");
    File::create(&path)
        .unwrap()
        .set_len(MAX_FILE_BYTES + 1)
        .unwrap();
    assert!(import_txt(&path).is_err());
}

#[test]
fn short_bare_text_is_one_chapter_and_preserves_paragraphs() {
    let text = "风从河上来。\n\n这不是章节标题。\n\n灯在岸边亮了。";
    let (book, bodies) = import_bytes(text.as_bytes()).unwrap();
    assert_eq!(book.chapters.len(), 1);
    assert_eq!(bodies, [text]);
    assert_eq!(chunk_unheaded(&"字".repeat(MAX_CHUNK_CHARS)).len(), 1);
}

#[test]
fn unheaded_chunks_prefer_paragraphs_without_tiny_tail() {
    let paragraph = "河".repeat(900);
    let text = vec![paragraph; 15].join("\n\n");
    let chunks = chunk_unheaded(&text);
    assert!(chunks.len() >= 3);
    assert_eq!(chunks.join("\n\n"), text);
    for body in &chunks {
        assert!((MIN_CHUNK_CHARS..=MAX_CHUNK_CHARS).contains(&body.chars().count()));
    }
    for count in [6001, 6100, 9001, 12001] {
        let text = "字".repeat(count);
        let chunks = chunk_unheaded(&text);
        assert_eq!(chunks.concat(), text);
        assert!(chunks
            .iter()
            .all(|chunk| (MIN_CHUNK_CHARS..=MAX_CHUNK_CHARS).contains(&chunk.chars().count())));
    }
}

#[test]
fn snapshot_survives_source_deletion_and_ids_ignore_filename() {
    let directory = tempdir().unwrap();
    let first = directory.path().join("first_name.txt");
    let second = directory.path().join("renamed.txt");
    let text = "第十二章 开始\n完整正文。\n第十三章 继续\n另一章完整正文。";
    fs::write(&first, text).unwrap();
    fs::write(&second, text).unwrap();
    let (book, bodies) = import_txt(&first).unwrap();
    let (renamed, other_bodies) = import_txt(&second).unwrap();
    fs::remove_file(first).unwrap();
    fs::remove_file(second).unwrap();
    assert!(book.id.starts_with("txt:"));
    assert_eq!(book.id, renamed.id);
    assert_eq!(book.title, "first name");
    assert_eq!(book.chapters[0].id, renamed.chapters[0].id);
    assert_ne!(book.chapters[0].id, book.chapters[1].id);
    assert_eq!(book.chapters.len(), bodies.len());
    assert_eq!(bodies, ["完整正文。", "另一章完整正文。"]);
    assert_eq!(bodies, other_bodies);
    let (changed, _) = import_bytes(format!("{text}改变").as_bytes()).unwrap();
    assert_ne!(book.id, changed.id);
}

#[test]
fn cached_txt_and_demo_reopen_without_original_source() {
    use crate::storage::Store;

    let directory = tempdir().unwrap();
    let source = directory.path().join("source.txt");
    fs::write(
        &source,
        "前言内容。\n第一章 启程\n这是第一章完整正文。\n第二章 归来\n这是第二章完整正文。",
    )
    .unwrap();
    let imported = import_txt(&source).unwrap();
    let snapshots = [imported, demo()];
    let root = directory.path().join("library");
    let store = Store::new(root.clone()).unwrap();
    for (book, bodies) in &snapshots {
        for (chapter, body) in book.chapters.iter().zip(bodies) {
            store.cache_chapter(&book.id, &chapter.id, body).unwrap();
        }
        assert!(store.get_book(&book.id).unwrap().is_none());
        store.save_book(book).unwrap();
    }
    fs::remove_file(source).unwrap();
    drop(store);
    let reopened = Store::new(root).unwrap();
    for (book, bodies) in snapshots {
        let saved = reopened.get_book(&book.id).unwrap().unwrap();
        assert_eq!(saved, book);
        for (chapter, body) in saved.chapters.iter().zip(bodies) {
            assert_eq!(
                reopened.cached_chapter(&saved.id, &chapter.id).unwrap(),
                Some(body)
            );
        }
    }
}

#[test]
fn consecutive_or_bare_headings_have_nonempty_snapshots() {
    let (book, bodies) = import_bytes("第一卷\n第一章\n正文。\n第二章".as_bytes()).unwrap();
    assert_eq!(book.chapters.len(), 3);
    assert_eq!(bodies, ["第一卷", "正文。", "第二章"]);
    assert_eq!(book.chapters.len(), bodies.len());
}

#[test]
fn demo_is_original_long_chinese_and_self_contained() {
    let (book, bodies) = demo();
    let (again, again_bodies) = demo();
    book.validate().unwrap();
    assert_eq!(book.source, "demo");
    assert!(book.id.starts_with("demo:"));
    assert!(book.description.contains("原创演示"));
    assert!(book.description.contains("demo:original"));
    assert!(book.description.contains("不是番茄平台作品"));
    assert!(book.chapters.len() >= 3);
    assert_eq!(book.chapters.len(), bodies.len());
    assert_eq!(book.id, again.id);
    assert_eq!(bodies, again_bodies);
    for (index, body) in bodies.iter().enumerate() {
        let chinese = body
            .chars()
            .filter(|ch| matches!(*ch as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff))
            .count();
        assert!(
            chinese >= 1000,
            "chapter {index} has only {chinese} Chinese characters"
        );
        assert!(body.contains('\n'));
        assert!(!body.contains('\u{fffd}'));
    }
    assert!(bodies[0] != bodies[1] && bodies[1] != bodies[2]);
}
