# Fanqie · 番茄终端阅读器

**在终端搜书、读小说，也能离线读自己的 TXT 和 EPUB。**

Fanqie 用 Rust、Ratatui 和 Crossterm 实现。界面借鉴 Claude Code：底部命令栏、可筛选的命令面板，以及不用离开键盘的阅读界面。

## 快速开始

如果 GitHub Releases 已有适合你系统的包，下载并解压后，在终端执行 `./fanqie --demo`（Windows PowerShell：`.\fanqie.exe --demo`）。也可以在仓库根目录用 Rust 1.89 或更新版本从源码运行：

```sh
cargo run --locked -- --demo
```

内置示例不需要联网。进入阅读页后，按 **空格** 翻页，在章末再按一次进入下一章；按 **Ctrl+K** 看全部命令，**Ctrl+Q** 保存进度并退出。建议使用能显示中文的交互终端。

```sh
cargo run --locked                                # 打开首页，直接输入书名即可搜索
cargo run --locked -- --search "十日终焉"          # 启动后直接搜索
cargo run --locked -- --book 7143038691944959011    # 按书籍 ID 打开，也可传完整书籍页链接
cargo run --locked -- --import "我的小说.epub"     # 导入 EPUB 或 TXT
```

想把 `fanqie` 装到 Cargo 的可执行文件目录，可在仓库根运行 `cargo install --path . --locked`。

## 阅读与管理

- **在线读书**：内置番茄书源，不需要自备 API。搜索结果支持书名别名；如果上游提供续页，可选「加载更多」，每次搜索最多取三页。主搜索异常时，程序会提示是否改用备用搜索或缓存。
- **离线续读**：阅读过的在线章节会缓存；导入的 TXT（UTF-8 / GB18030）和无 DRM 的文本 EPUB 会保存为本地快照，后续阅读不依赖原文件。EPUB 不渲染插图和网页样式。
- **连续阅读**：保存每本书的阅读进度，支持章末换章、手动刷新连载目录和取消网络加载。下一章预加载默认关闭，只能为当前会话开启。

| 快捷键 | 操作 |
| --- | --- |
| `Esc` / `/` | 切换阅读与输入焦点 / 打开命令输入 |
| `Ctrl+K`、`Ctrl+B`、`Ctrl+T` | 命令面板、书架、章节目录 |
| `↑` `↓` / `j` `k` | 阅读模式下逐行滚动 |
| `Space` | 翻页；在章末再按进入下一章 |
| `←` `→` / `p` `n` | 上一章 / 下一章 |
| `Ctrl+X`、`Ctrl+Q` | 取消加载、保存并退出 |

| 命令 | 用法 |
| --- | --- |
| `/search 书名`、`/more` | 搜书、加载下一页（仅在有续页时） |
| `/open 书籍ID或链接`、`/import 路径` | 打开在线书籍、导入 TXT 或 EPUB |
| `/resume`、`/shelf` | 继续上次阅读、打开书架 |
| `/toc`、`/goto 12`、`/refresh` | 看目录、跳到第 12 章、刷新当前在线书目录 |
| `/bookmark 地点`、`/bookmarks`、`/unbookmark` | 添加书签、查看和跳转、移除书签 |
| `/find 夜色` | 查找当前章的正文并跳到匹配处 |
| `/prefetch on`、`/prefetch off`、`/cancel` | 开关本次会话的预加载、取消加载 |
| `/remove`、`/cache`、`/cache clear` | 从书架移除、统计章节缓存、清理可重新下载的在线章节 |
| `/theme`、`/width 88` | 切换深浅配色、设置正文宽度（40–120 列） |

`/remove` 会删除该书在本机的章节缓存或导入快照，但保留进度、书签和原始文件。`/cache clear` 不删除 TXT / EPUB 快照。两项操作都需要确认，默认选中取消。

## 数据与在线限制

- 用 `/info` 查看当前数据目录，也可在启动时用 `--data-dir <路径>` 指定。书架、进度和书签保存在本机；不支持番茄官方账号登录或云同步。
- 在线书源依赖上游服务，可能遇到空响应、限流、访问限制或接口变更。遇到这些情况可以稍后重试，或继续阅读已缓存的章节；连载目录刷新遇到旧章节删减或重排时会保留原目录，以免进度错位。
- 可选的 `/api` 和 `/search-api` 会把自定义地址明文写入本机设置；对应的 `--content-api` 和 `--search-api` 仅在本次运行生效。不要提交数据目录、匿名设备凭据、含令牌的设置或小说缓存。
- 已在 Linux 上验证运行。发布流程配置了 macOS 和 Windows 的构建；Windows 的在线设备存储尚未在原生系统上实测。请仅阅读有权访问的内容，遵守书源条款和当地法律。本项目不是番茄小说官方客户端。

## 开发与发布

```sh
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
```

普通测试不访问线上书源；在线用例默认忽略，按 [AGENTS.md](AGENTS.md) 的说明单独运行。`src/app/` 管理交互和阅读状态，`src/ui/` 负责渲染，`src/library/` 处理书源与导入，`src/storage.rs` 和 `src/storage/` 负责本地数据。网络请求与取消集中在 `src/net.rs`。

推送 `vMAJOR.MINOR.PATCH` 格式的标签（例如 `v0.1.0`）会触发 [发布流程](.github/workflows/release.yml)。它会先跑离线测试，再打包 Linux x86-64 / ARM64、macOS Intel / Apple Silicon 和 Windows x86-64。所有平台通过后，才上传压缩包和 `SHA256SUMS`。

## 许可与致谢

项目代码采用 [MIT 许可证](LICENSE)。项目思路参考 [Tomato-Novel-Downloader](https://github.com/zhongbai2333/Tomato-Novel-Downloader)。APP 通道和网页字体映射分别参考了 [fanqie-novel-downloader](https://github.com/Jees996/fanqie-novel-downloader) 与 [mcp-server-fanqie](https://github.com/fysh1010/mcp-server-fanqie)，对应的上游版权和许可声明保留在 [licenses/](licenses/) 中。许可证不授予小说正文的版权。
