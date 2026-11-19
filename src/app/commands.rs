//! Slash-command definitions and dispatch.
use std::path::PathBuf;

use super::{App, Outcome};
use crate::models::Settings;

pub const COMMANDS: &[(&str, &str)] = &[
    ("/resume", "继续上次阅读"),
    ("/shelf", "打开本地书架"),
    ("/remove", "从书架移除书籍（保留进度与书签）"),
    ("/cache", "查看缓存占用；/cache clear 清理在线正文"),
    ("/search ", "在线搜索番茄小说，同时匹配本地书架"),
    ("/more", "加载上一条搜索的下一页"),
    ("/open ", "打开番茄书籍 ID 或链接"),
    ("/import ", "导入本地 TXT 或 EPUB 文件"),
    ("/toc", "章节目录 · 输入关键词筛选"),
    ("/refresh", "刷新番茄连载目录，保留阅读位置"),
    (
        "/prefetch ",
        "下一章预加载：on 启用 / off 关闭，本次会话有效",
    ),
    ("/cancel", "取消加载，保留当前阅读位置"),
    ("/bookmark ", "为当前位置添加书签，可自定义名称"),
    ("/bookmarks", "查看并跳转到本书书签"),
    ("/unbookmark", "移除本书书签"),
    ("/find ", "查找本章正文，例如 /find 夜色"),
    ("/goto ", "跳转到章节序号，如 /goto 12"),
    ("/next", "下一章"),
    ("/prev", "上一章"),
    ("/demo", "阅读内置原创示例 · 完全离线"),
    ("/home", "回到首页"),
    ("/theme", "切换深色 / 浅色主题"),
    ("/width ", "设置正文宽度，40–120 列"),
    ("/api ", "保存正文 API；/api off 清除"),
    ("/search-api ", "保存搜索 API；/search-api off 清除"),
    ("/info", "查看数据目录与书源状态"),
    ("/help", "快捷键与使用说明"),
    ("/quit", "保存进度并退出"),
];

impl App {
    pub fn execute(&mut self, input: &str) {
        let (command, argument) = input.split_once(char::is_whitespace).unwrap_or((input, ""));
        let argument = argument.trim();
        match command {
            "/" | "/commands" => self.show_commands(),
            "/help" => {
                self.show_commands();
                if let Some(overlay) = &mut self.overlay {
                    overlay.title = "帮助 / 快捷键".into();
                    overlay.subtitle = "Esc 切换输入/阅读 · Space 翻页/章末换章 · ←→ 换章 · Ctrl+B 书架 · Ctrl+T 目录 · Ctrl+Q 退出".into();
                }
                self.note("直接输入书名在线搜索。Space 翻页，到章末再按进入下一章；/refresh 刷新连载目录。自定义 /api、/search-api 的密钥会明文保存在本机。", false);
            }
            "/demo" => self.demo(),
            "/resume" => self.resume(),
            "/shelf" => {
                self.refresh_shelf();
                self.show_books("我的书架", self.shelf.clone());
            }
            "/remove" => self.remove_command(),
            "/cache" if argument.is_empty() => self.cache_command(),
            "/cache" if argument == "clear" => self.prompt_clear_cache(),
            "/cache" => self.note("用法：/cache 或 /cache clear。", true),
            "/bookmark" => self.add_bookmark(argument),
            "/bookmarks" => self.show_bookmarks(false),
            "/unbookmark" => self.show_bookmarks(true),
            "/find" => self.find_in_chapter(argument),
            "/more" => self.search_more(),
            "/toc" => self.show_toc(),
            "/refresh" => self.refresh_catalog(),
            "/prefetch" => self.configure_prefetch(argument),
            "/cancel" => self.cancel_loading(),
            "/next" => self.next_chapter(true),
            "/prev" => self.next_chapter(false),
            "/open" if !argument.is_empty() => self.open_remote(argument.into()),
            "/import" if !argument.is_empty() => {
                let argument = unquote(argument);
                let path = if let Some(relative) = argument.strip_prefix("~/") {
                    std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_default()
                        .join(relative)
                } else {
                    PathBuf::from(argument)
                };
                self.import(path);
            }
            "/search" if !argument.is_empty() => self.search(argument),
            "/goto" => match argument.parse::<usize>() {
                Ok(number)
                    if number > 0
                        && self
                            .current
                            .as_ref()
                            .is_some_and(|book| number <= book.chapters.len()) =>
                {
                    if !self.save_progress() {
                        return;
                    }
                    self.load_chapter(self.current.clone().unwrap(), number - 1, 0.0);
                }
                _ => self.note("请输入有效章节序号，例如 /goto 12。", true),
            },
            "/home" => {
                if !self.save_progress() {
                    return;
                }
                self.stop_loading();
                self.prefetch_attempted = None;
                self.busy = None;
                self.current = None;
                self.content.clear();
                self.lines.clear();
                self.refresh_shelf();
                self.note("已回到首页，阅读进度已保存。", false);
            }
            "/theme" => {
                let mut settings = self.settings.clone();
                settings.light_theme = !settings.light_theme;
                self.persist_settings(settings, None);
            }
            "/width" => match argument.parse::<u16>() {
                Ok(width @ 40..=120) => {
                    let mut settings = self.settings.clone();
                    settings.reader_width = width;
                    self.persist_settings(settings, None);
                }
                _ => self.note("正文宽度范围为 40–120 列，例如 /width 88。", true),
            },
            "/api" | "/search-api" if !argument.is_empty() => {
                let value = if argument == "off" { "" } else { argument };
                let probe = value
                    .replace("{query}", "test")
                    .replace("{chapter_id}", "1")
                    .replace("{book_id}", "1");
                if !value.is_empty()
                    && !url::Url::parse(&probe).is_ok_and(|url| {
                        matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
                    })
                {
                    self.note("服务地址必须是完整的 http:// 或 https:// URL。", true);
                    return;
                }
                let mut settings = self.settings.clone();
                if command == "/api" {
                    settings.content_api = value.into();
                } else {
                    settings.search_api = value.into();
                }
                self.persist_settings(settings, Some(command));
                if !self.is_error {
                    self.note("书源配置已保存（本机明文）。只使用你信任的服务。", false);
                }
            }
            "/quit" | "/exit" => self.quit = true,
            "/open" | "/search" | "/import" | "/api" | "/search-api" => {
                self.note("命令缺少参数。输入 /help 查看说明。", true)
            }
            "/info" => self.note(
                format!(
                    "数据目录：{} · 正文服务：{} · 搜索服务：{}",
                    self.store.root().display(),
                    if self.settings.content_api.is_empty() {
                        "内置番茄直连"
                    } else {
                        "自定义 API"
                    },
                    if self.settings.search_api.is_empty() {
                        "内置官方搜索"
                    } else {
                        "自定义 API"
                    }
                ),
                false,
            ),
            _ if is_local_book_path(unquote(input)) => self.import(PathBuf::from(unquote(input))),
            _ if command.starts_with('/') => self.note("未知命令。输入 / 查看可用命令。", true),
            _ if input.chars().all(|c| c.is_ascii_digit())
                || input.contains("/page/")
                || input.contains("book_id=") =>
            {
                self.open_remote(input.into())
            }
            _ => self.search(input),
        }
    }

    fn search(&mut self, query: &str) {
        let query = query.to_owned();
        self.job("搜索小说", move |library| {
            library.search_report(&query).map(Outcome::Results)
        });
    }

    fn persist_settings(&mut self, settings: Settings, api_field: Option<&str>) {
        let persisted = self
            .store
            .settings()
            .map(|mut saved| {
                saved.reader_width = settings.reader_width;
                saved.light_theme = settings.light_theme;
                if api_field == Some("/api") {
                    saved.content_api = settings.content_api.clone();
                }
                if api_field == Some("/search-api") {
                    saved.search_api = settings.search_api.clone();
                }
                saved
            })
            .and_then(|saved| self.store.save_settings(&saved));
        match persisted {
            Ok(()) => {
                self.settings = settings;
                self.note("设置已保存。", false);
            }
            Err(error) => self.note(format!("保存设置失败：{error}"), true),
        }
    }
}

fn unquote(input: &str) -> &str {
    if (input.starts_with('"') && input.ends_with('"'))
        || (input.starts_with('\'') && input.ends_with('\''))
    {
        input.get(1..input.len().saturating_sub(1)).unwrap_or(input)
    } else {
        input
    }
}

fn is_local_book_path(input: &str) -> bool {
    std::path::Path::new(input)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("txt") || ext.eq_ignore_ascii_case("epub"))
}
