//! Command-line arguments; startup selection is applied by main.
use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(
    version,
    about = "番茄终端阅读器 · Rust / Ratatui",
    long_about = "Claude Code 风格的番茄在线小说阅读器。输入书名直接搜索，打开后自动读取正文并缓存。\n内置官方搜索、网页字体还原和匿名阅读通道，无需自备 API；同时支持本地 TXT、EPUB 与离线续读。"
)]
pub(crate) struct Args {
    /// 打开内置原创示例，无需联网
    #[arg(long)]
    pub(crate) demo: bool,
    /// 导入 TXT（UTF-8 / GB18030）或 EPUB 小说
    #[arg(long, value_name = "PATH", conflicts_with_all = ["book", "demo"])]
    pub(crate) import: Option<PathBuf>,
    /// 打开番茄书籍 ID 或完整书籍链接
    #[arg(long, value_name = "ID_OR_URL", conflicts_with = "demo")]
    pub(crate) book: Option<String>,
    /// 启动后直接在线搜索小说
    #[arg(long, value_name = "QUERY", conflicts_with_all = ["book", "demo", "import"])]
    pub(crate) search: Option<String>,
    /// 自定义书架、缓存和设置目录
    #[arg(long, value_name = "PATH")]
    pub(crate) data_dir: Option<PathBuf>,
    /// 兼容 batch_full 的正文服务或含 {chapter_id} 的 URL 模板
    #[arg(long, value_name = "URL")]
    pub(crate) content_api: Option<String>,
    /// 搜索服务 URL，支持 {query} 模板或 ?q= 参数
    #[arg(long, value_name = "URL")]
    pub(crate) search_api: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_search_can_be_launched_without_api_configuration() {
        let args = Args::try_parse_from(["fanqie", "--search", "十日终焉"]).unwrap();
        assert_eq!(args.search.as_deref(), Some("十日终焉"));
        assert!(args.content_api.is_none());
        assert!(args.search_api.is_none());
    }

    #[test]
    fn search_and_book_launches_are_mutually_exclusive() {
        assert!(Args::try_parse_from([
            "fanqie",
            "--search",
            "十日终焉",
            "--book",
            "7143038691944959011"
        ])
        .is_err());
    }
}
