use std::collections::HashSet;

use anyhow::Result;

use crate::models::{Book, SearchReport};

pub(super) fn merge_search_report(
    query: &str,
    local: Vec<Book>,
    cached: Vec<Book>,
    online: Result<SearchReport>,
) -> Result<SearchReport> {
    let mut report = match online {
        Ok(mut report) => {
            if report.notice.is_some() && !cached.is_empty() {
                report.books.extend(cached);
                report.add_notice("同时显示匹配的历史书名缓存");
            }
            report.books.extend(local);
            report
        }
        Err(error) => {
            if local.is_empty() && cached.is_empty() {
                return Err(error);
            }
            let mut books = cached;
            books.extend(local);
            SearchReport {
                books,
                notice: Some("在线搜索暂不可用，仅显示本地书架与历史搜索缓存（可能过期）".into()),
                ..SearchReport::default()
            }
        }
    };
    rank_and_dedup(query, &mut report.books);
    Ok(report)
}

pub(super) fn append_search_page(
    query: &str,
    previous: &SearchReport,
    mut page: SearchReport,
    cached: Vec<Book>,
) -> SearchReport {
    let mut books = previous.books.clone();
    books.append(&mut page.books);
    if !cached.is_empty() {
        books.extend(cached);
        page.add_notice("同时显示匹配的历史书名缓存");
    }
    rank_and_dedup(query, &mut books);
    page.books = books;
    page
}

pub(super) fn rank_and_dedup(query: &str, books: &mut Vec<Book>) {
    rank_search_results(query, books);
    let mut seen = HashSet::new();
    books.retain(|book| seen.insert(book.id.clone()));
}

// Normalize typography, not wording: similar titles must never become exact hits.
pub(super) fn normalized_title(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .filter(|c| {
            !matches!(
                c,
                ',' | '，'
                    | '、'
                    | '.'
                    | '。'
                    | '!'
                    | '！'
                    | '?'
                    | '？'
                    | ':'
                    | '：'
                    | ';'
                    | '；'
                    | '"'
                    | '\''
                    | '“'
                    | '”'
                    | '‘'
                    | '’'
                    | '《'
                    | '》'
                    | '('
                    | ')'
                    | '（'
                    | '）'
                    | '['
                    | ']'
                    | '【'
                    | '】'
            )
        })
        .flat_map(char::to_lowercase)
        .collect()
}

pub(super) fn title_match(wanted: &str, title: &str) -> u8 {
    match_normalized_title(wanted, &normalized_title(title))
}

fn match_normalized_title(wanted: &str, title: &str) -> u8 {
    if wanted.is_empty() {
        0
    } else if title == wanted {
        2
    } else if title.contains(wanted) {
        1
    } else {
        0
    }
}

pub(super) fn rank_search_results(query: &str, books: &mut [Book]) {
    let query = normalized_title(query);
    if query.is_empty() {
        return;
    }
    let query_chars: Vec<char> = query.chars().collect();
    let pairs: Vec<String> = query_chars
        .windows(2)
        .map(|pair| pair.iter().collect())
        .collect();
    let score = |book: &Book| {
        let title = normalized_title(&book.title);
        let matched = match_normalized_title(&query, &title);
        if matched != 0 {
            return (matched, 0);
        }
        let pairs = pairs
            .iter()
            .filter(|pair| title.contains(pair.as_str()))
            .count();
        (0, pairs)
    };
    // Preserve upstream order for ties, but never bury exact titles under recommendations.
    books.sort_by_cached_key(|book| std::cmp::Reverse(score(book)));
}
