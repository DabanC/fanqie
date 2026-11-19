//! Bounded maintenance over recognized book and chapter records only.
use std::{
    collections::HashSet,
    ffi::OsStr,
    fs::{self, Metadata},
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::SystemTime,
};

use anyhow::{bail, ensure, Context, Result};

use super::{hash, valid_id, Recent, SavedProgress, Store, MAX_CHAPTER, MAX_RECORD, SEQUENCE};
use crate::models::{Book, Progress};

const MAX_ENTRIES: usize = 250_000;
const MAX_CATALOG_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct CacheUsage {
    pub(crate) files: usize,
    pub(crate) bytes: u64,
}

impl CacheUsage {
    fn record(&mut self, bytes: u64) {
        self.files += 1;
        self.bytes += bytes;
    }
}

#[derive(Default)]
pub(crate) struct CacheSummary {
    pub(crate) remote: CacheUsage,
    pub(crate) offline: CacheUsage,
    pub(crate) skipped: usize,
}

#[derive(Default)]
pub(crate) struct CacheClear {
    pub(crate) removed: CacheUsage,
    pub(crate) skipped: usize,
}

#[derive(Default)]
pub(crate) struct BookRemoval {
    pub(crate) cleanup_pending: bool,
}

struct ScanBudget {
    entries: usize,
    catalog_bytes: u64,
}

impl Default for ScanBudget {
    fn default() -> Self {
        Self {
            entries: MAX_ENTRIES,
            catalog_bytes: MAX_CATALOG_BYTES,
        }
    }
}

impl ScanBudget {
    fn entry(&mut self) -> Result<()> {
        ensure!(self.entries > 0, "缓存项目超过扫描上限，未开始清理");
        self.entries -= 1;
        Ok(())
    }

    fn catalog(&mut self, dir: &Path) -> Result<Result<Option<(Book, FileStamp)>>> {
        let path = dir.join("metadata.json");
        let metadata = match regular(&path) {
            Ok(Some(metadata)) => metadata,
            Ok(None) => return Ok(Ok(None)),
            Err(error) => return Ok(Err(error)),
        };
        if metadata.len() > MAX_RECORD {
            return Ok(Err(anyhow::anyhow!("书籍元数据过大")));
        }
        // Charge the read even if the JSON is invalid, and bound it if the file grows.
        let size = metadata.len();
        ensure!(
            size <= self.catalog_bytes,
            "书籍目录超过扫描上限，未开始清理"
        );
        self.catalog_bytes -= size;
        Ok((|| {
            let stamp = FileStamp::from(&metadata);
            let book: Book = read_record(&path, &stamp)?;
            book.validate()?;
            ensure!(
                dir.file_name() == Some(OsStr::new(&hash(book.id.as_bytes()))),
                "书籍缓存目录标识不匹配"
            );
            Ok(Some((book, stamp)))
        })())
    }
}

struct CachedFile {
    name: String,
    stamp: FileStamp,
}

struct RemoteFiles {
    dir: String,
    catalog: FileStamp,
    files: Vec<CachedFile>,
}

#[derive(PartialEq, Eq)]
struct FileStamp {
    bytes: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64),
}

impl From<&Metadata> for FileStamp {
    fn from(info: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        Self {
            bytes: info.len(),
            modified: info.modified().ok(),
            #[cfg(unix)]
            identity: (info.dev(), info.ino()),
        }
    }
}

impl FileStamp {
    fn matches(&self, info: &Metadata) -> bool {
        *self == Self::from(info)
    }
}

fn unchanged(path: &Path, expected: &FileStamp) -> bool {
    matches!(regular(path), Ok(Some(info)) if expected.matches(&info))
}

fn read_record<T: serde::de::DeserializeOwned>(path: &Path, stamp: &FileStamp) -> Result<T> {
    ensure!(stamp.bytes <= MAX_RECORD, "本地状态文件过大或已损坏");
    let file = fs::File::open(path).context("无法打开本地状态")?;
    let actual = file.metadata().context("无法读取本地状态")?;
    ensure!(
        actual.is_file() && stamp.matches(&actual) && unchanged(path, stamp),
        "本地状态已变化"
    );
    let mut bytes = Vec::new();
    file.take(stamp.bytes + 1)
        .read_to_end(&mut bytes)
        .context("无法读取本地状态")?;
    ensure!(
        bytes.len() as u64 == stamp.bytes && unchanged(path, stamp),
        "本地状态已变化"
    );
    Store::decode_record(&bytes)
}

fn metadata(path: &Path) -> Result<Option<Metadata>> {
    let info = match fs::symlink_metadata(path) {
        Ok(info) => info,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("无法读取本地状态信息"),
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        ensure!(
            info.file_attributes() & REPARSE_POINT == 0,
            "已跳过重解析点"
        );
    }
    ensure!(!info.file_type().is_symlink(), "已跳过符号链接");
    Ok(Some(info))
}

fn directory(path: &Path) -> Result<bool> {
    match metadata(path)? {
        None => Ok(false),
        Some(info) if info.is_dir() => Ok(true),
        Some(_) => bail!("缓存路径不是普通目录"),
    }
}

fn regular(path: &Path) -> Result<Option<Metadata>> {
    let Some(info) = metadata(path)? else {
        return Ok(None);
    };
    ensure!(info.is_file(), "缓存项目不是普通文件");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(info.nlink() == 1, "已跳过硬链接");
    }
    Ok(Some(info))
}

fn chapter_names(book: &Book) -> HashSet<String> {
    book.chapters
        .iter()
        .map(|chapter| format!("{}.txt", hash(chapter.id.as_bytes())))
        .collect()
}

fn chapter_files(
    path: &Path,
    known: &HashSet<String>,
    budget: &mut ScanBudget,
    max_size: u64,
) -> Result<(Vec<CachedFile>, usize)> {
    let mut files = Vec::new();
    let mut skipped = 0;
    for entry in fs::read_dir(path).context("无法读取章节目录")? {
        budget.entry()?;
        let entry = entry.context("无法读取章节项目")?;
        let name = entry.file_name();
        if let Some(name) = name.to_str().filter(|name| known.contains(*name)) {
            if let Ok(Some(info)) = regular(&entry.path()) {
                if info.len() <= max_size {
                    files.push(CachedFile {
                        name: name.to_owned(),
                        stamp: FileStamp::from(&info),
                    });
                    continue;
                }
            }
        }
        skipped += 1;
    }
    Ok((files, skipped))
}

impl Store {
    fn maintenance_root(&self) -> Result<PathBuf> {
        ensure!(directory(self.root())?, "找不到本地数据目录");
        self.root().canonicalize().context("无法定位本地数据目录")
    }

    fn inventory(
        root: &Path,
        plan_removal: bool,
        budget: &mut ScanBudget,
    ) -> Result<(CacheSummary, Vec<RemoteFiles>)> {
        let books = root.join("books");
        let mut summary = CacheSummary::default();
        let mut targets = Vec::new();
        if !directory(&books)? {
            return Ok((summary, targets));
        }
        for entry in fs::read_dir(&books).context("无法读取书架目录")? {
            budget.entry()?;
            let entry = entry.context("无法读取书架项目")?;
            let name = entry.file_name();
            let path = entry.path();
            let Some(name) = name.to_str().filter(|name| {
                name.len() == 64
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }) else {
                summary.skipped += 1;
                continue;
            };
            if !matches!(directory(&path), Ok(true)) {
                summary.skipped += 1;
                continue;
            }
            let chapters = path.join("chapters");
            match directory(&chapters) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => {
                    summary.skipped += 1;
                    continue;
                }
            }
            let Ok(Some((book, catalog))) = budget.catalog(&path)? else {
                summary.skipped += 1;
                continue;
            };
            let remote = book.source == "fanqie";
            let (files, skipped) =
                chapter_files(&chapters, &chapter_names(&book), budget, MAX_CHAPTER)?;
            let usage = if remote {
                &mut summary.remote
            } else {
                &mut summary.offline
            };
            for file in &files {
                usage.record(file.stamp.bytes);
            }
            summary.skipped += skipped;
            if plan_removal && remote && !files.is_empty() {
                targets.push(RemoteFiles {
                    dir: name.to_owned(),
                    catalog,
                    files,
                });
            }
        }
        Ok((summary, targets))
    }

    pub(crate) fn chapter_cache_summary(&self) -> Result<CacheSummary> {
        let root = self.maintenance_root()?;
        Self::inventory(&root, false, &mut ScanBudget::default()).map(|(summary, _)| summary)
    }

    pub(crate) fn clear_remote_chapter_cache(&self) -> Result<CacheClear> {
        let root = self.maintenance_root()?;
        let (summary, targets) = Self::inventory(&root, true, &mut ScanBudget::default())?;
        let books = root.join("books");
        let mut result = CacheClear {
            skipped: summary.skipped,
            ..CacheClear::default()
        };
        for target in targets {
            let path = books.join(&target.dir);
            let chapters = path.join("chapters");
            if !matches!(directory(&root), Ok(true))
                || !matches!(directory(&books), Ok(true))
                || !matches!(directory(&path), Ok(true))
                || !matches!(directory(&chapters), Ok(true))
                || !unchanged(&path.join("metadata.json"), &target.catalog)
            {
                result.skipped += target.files.len();
                continue;
            }
            for file in target.files {
                let path = chapters.join(&file.name);
                if unchanged(&path, &file.stamp) && fs::remove_file(path).is_ok() {
                    result.removed.record(file.stamp.bytes);
                } else {
                    result.skipped += 1;
                }
            }
        }
        Ok(result)
    }

    pub(crate) fn remove_shelf_book(
        &self,
        expected: &Book,
        progress: Option<Progress>,
    ) -> Result<BookRemoval> {
        valid_id(&expected.id)?;
        let root = self.maintenance_root()?;
        let books = root.join("books");
        let path = books.join(hash(expected.id.as_bytes()));
        ensure!(
            directory(&books)? && directory(&path)?,
            "书架中已找不到这本书"
        );
        let mut budget = ScanBudget::default();
        let (book, catalog) = budget.catalog(&path)??.context("书架中已找不到这本书")?;
        ensure!(
            book.id == expected.id
                && book.source == expected.source
                && book.title == expected.title
                && book.chapters == expected.chapters,
            "书籍标识、来源或目录已变化，请重新打开书架确认"
        );
        let chapters = path.join("chapters");
        let has_chapters = directory(&chapters)?;
        let files = if has_chapters {
            let (files, skipped) =
                chapter_files(&chapters, &chapter_names(&book), &mut budget, u64::MAX)?;
            ensure!(skipped == 0, "章节目录含无法确认归属的项目，未移除书籍");
            files
        } else {
            Vec::new()
        };

        let metadata_path = path.join("metadata.json");
        ensure!(
            directory(&root)?
                && directory(&books)?
                && directory(&path)?
                && unchanged(&metadata_path, &catalog),
            "书架记录已变化，未移除书籍"
        );

        let progress_path = path.join("progress.json");
        if let Some(progress) = progress {
            progress.validate()?;
            if let Some(info) = regular(&progress_path)? {
                let saved: SavedProgress = read_record(&progress_path, &FileStamp::from(&info))?;
                ensure!(saved.book_id == book.id, "原阅读进度标识不匹配");
                Progress {
                    chapter: saved.chapter,
                    fraction: saved.fraction,
                }
                .validate()?;
            }
            Self::write(
                &progress_path,
                SavedProgress {
                    book_id: book.id.clone(),
                    chapter: progress.chapter,
                    fraction: progress.fraction,
                },
            )?;
        }

        // Move the validated snapshot out of the live path before unpublishing
        // it, so a metadata unlink failure does not leave a partial local book.
        let staged = if has_chapters {
            let staging = path.join(format!(
                ".removed-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&staging).context("无法创建移除暂存目录；未移除书籍")?;
            let staged_chapters = staging.join("chapters");
            if let Err(error) = fs::rename(&chapters, &staged_chapters) {
                let _ = fs::remove_dir(&staging);
                return Err(error).context("无法移走章节；未移除书籍");
            }
            Some((staging, staged_chapters))
        } else {
            None
        };
        let removed = if unchanged(&metadata_path, &catalog) {
            fs::remove_file(&metadata_path).context("无法移除书架记录")
        } else {
            Err(anyhow::anyhow!("书架记录已变化，未移除书籍"))
        };
        if let Err(error) = removed {
            if let Some((staging, staged_chapters)) = &staged {
                fs::rename(staged_chapters, &chapters)
                    .context("无法恢复暂存章节；正文仍在书籍的暂存目录中")?;
                let _ = fs::remove_dir(staging);
            }
            return Err(error);
        }

        let mut result = BookRemoval::default();
        if let Some((staging, staged_chapters)) = staged {
            if matches!(directory(&staging), Ok(true))
                && matches!(directory(&staged_chapters), Ok(true))
            {
                for file in files {
                    let path = staged_chapters.join(file.name);
                    if unchanged(&path, &file.stamp) {
                        let _ = fs::remove_file(path);
                    }
                }
                result.cleanup_pending =
                    fs::remove_dir(&staged_chapters).is_err() || fs::remove_dir(&staging).is_err();
            } else {
                result.cleanup_pending = true;
            }
        }
        let recent_path = root.join("recent.json");
        if let Ok(Some(info)) = regular(&recent_path) {
            let stamp = FileStamp::from(&info);
            if matches!(read_record::<Recent>(&recent_path, &stamp), Ok(recent) if recent.book_id == book.id)
                && unchanged(&recent_path, &stamp)
            {
                result.cleanup_pending |= fs::remove_file(recent_path).is_err();
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
