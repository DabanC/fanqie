use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::models::{Book, Progress, Settings};

mod bookmarks;
mod maintenance;

const MAX_RECORD: u64 = 16 * 1024 * 1024;
const MAX_CHAPTER: u64 = 32 * 1024 * 1024;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub struct Store {
    root: Arc<PathBuf>,
    warnings: Arc<Mutex<Vec<String>>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record<T> {
    version: u32,
    value: T,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedProgress {
    book_id: String,
    chapter: usize,
    fraction: f64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recent {
    book_id: String,
}

pub(crate) fn valid_id(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 2048 || value.chars().any(char::is_control) {
        bail!("无效的书籍或章节标识");
    }
    Ok(())
}

pub(crate) fn hash(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

impl Store {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root).context("无法创建数据目录，请检查权限")?;
        Ok(Self {
            root: Arc::new(root),
            warnings: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    fn book_dir(&self, id: &str) -> Result<PathBuf> {
        valid_id(id)?;
        Ok(self.root.join("books").join(hash(id.as_bytes())))
    }

    fn chapter_path(&self, id: &str, chapter: &str) -> Result<PathBuf> {
        valid_id(chapter)?;
        Ok(self
            .book_dir(id)?
            .join("chapters")
            .join(format!("{}.txt", hash(chapter.as_bytes()))))
    }

    fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
        let parent = path.parent().context("无效的缓存路径")?;
        fs::create_dir_all(parent).context("无法创建缓存目录")?;
        let mut temporary = None;
        let result = (|| -> Result<()> {
            for _ in 0..16 {
                let stamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let candidate = parent.join(format!(
                    ".pending-{}-{stamp}-{}",
                    std::process::id(),
                    SEQUENCE.fetch_add(1, Ordering::Relaxed)
                ));
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = match options.open(&candidate) {
                    Ok(file) => file,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(_) => bail!("无法写入缓存，请检查磁盘空间和目录权限"),
                };
                temporary = Some(candidate.clone());
                file.write_all(bytes).context("无法写入完整缓存")?;
                file.sync_all().context("无法同步缓存到磁盘")?;
                drop(file);
                fs::rename(&candidate, path).context("无法原子替换缓存文件")?;
                temporary = None;
                #[cfg(unix)]
                fs::File::open(parent)?
                    .sync_all()
                    .context("无法同步缓存目录")?;
                return Ok(());
            }
            bail!("无法分配临时缓存文件");
        })();
        if let Some(path) = temporary {
            let _ = fs::remove_file(path);
        }
        result
    }

    fn read_bytes(path: &Path, max: u64) -> Result<Option<Vec<u8>>> {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => bail!("无法读取本地状态，请检查目录权限"),
        };
        let mut bytes = Vec::new();
        file.take(max + 1)
            .read_to_end(&mut bytes)
            .context("无法读取完整本地状态")?;
        if bytes.len() as u64 > max {
            bail!("本地状态文件过大或已损坏");
        }
        Ok(Some(bytes))
    }

    fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
        let Some(bytes) = Self::read_bytes(path, MAX_RECORD)? else {
            return Ok(None);
        };
        Self::decode_record(&bytes).map(Some)
    }

    fn decode_record<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
        let record: Record<T> = serde_json::from_slice(bytes)
            .map_err(|_| anyhow!("本地 JSON 状态已损坏或版本不兼容，原文件未修改"))?;
        if record.version != 1 {
            bail!("不支持的本地状态版本，原文件未修改");
        }
        Ok(record.value)
    }

    fn write<T: Serialize>(path: &Path, value: T) -> Result<()> {
        let bytes =
            serde_json::to_vec(&Record { version: 1, value }).context("无法序列化本地状态")?;
        if bytes.len() as u64 > MAX_RECORD {
            bail!("本地状态过大");
        }
        Self::atomic(path, &bytes)
    }

    pub fn settings(&self) -> Result<Settings> {
        let settings: Settings = Self::read(&self.root.join("settings.json"))
            .map_err(|_| anyhow!("设置文件损坏、不可读或版本不兼容；可能含 API 密钥，请先修复或手动移走 settings.json，程序不会用默认设置覆盖它"))?
            .unwrap_or_default();
        settings
            .validate()
            .map_err(|_| anyhow!("设置内容无效；请修复 settings.json，程序不会覆盖已有密钥"))?;
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        settings.validate()?;
        // A failed settings read must never become a default-settings write.
        self.settings()?;
        Self::write(&self.root.join("settings.json"), settings)
    }

    pub fn save_book(&self, book: &Book) -> Result<()> {
        book.validate()?;
        Self::write(&self.book_dir(&book.id)?.join("metadata.json"), book)
    }

    pub fn get_book(&self, id: &str) -> Result<Option<Book>> {
        let Some(book): Option<Book> = Self::read(&self.book_dir(id)?.join("metadata.json"))?
        else {
            return Ok(None);
        };
        book.validate()?;
        if book.id != id {
            bail!("书籍缓存标识不匹配，原文件未修改");
        }
        Ok(Some(book))
    }

    pub fn books(&self) -> Result<Vec<Book>> {
        let entries = match fs::read_dir(self.root.join("books")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => bail!("无法读取书架目录"),
        };
        let mut books = Vec::new();
        for entry in entries {
            let entry = entry.context("无法读取书架目录项目")?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let path = entry.path().join("metadata.json");
            let result = (|| -> Result<Option<Book>> {
                let Some(book): Option<Book> = Self::read(&path)? else {
                    return Ok(None);
                };
                book.validate()?;
                if entry.file_name().to_string_lossy() != hash(book.id.as_bytes()) {
                    bail!("书籍缓存目录标识不匹配");
                }
                Ok(Some(book))
            })();
            match result {
                Ok(Some(book)) => books.push(book),
                Ok(None) => {}
                Err(_) => self.warn("已跳过损坏或不可读的书籍元数据；原文件未修改"),
            }
        }
        let recent = match self.last_book() {
            Ok(recent) => recent,
            Err(_) => {
                self.warn("最近阅读标记损坏或不可读；原文件未修改");
                None
            }
        };
        books.sort_by_cached_key(|book| {
            let directory = self.root.join("books").join(hash(book.id.as_bytes()));
            let modified = fs::metadata(directory.join("progress.json"))
                .and_then(|meta| meta.modified())
                .or_else(|_| {
                    fs::metadata(directory.join("metadata.json")).and_then(|meta| meta.modified())
                })
                .unwrap_or(UNIX_EPOCH);
            (
                recent.as_deref() != Some(book.id.as_str()),
                std::cmp::Reverse(modified),
                book.title.clone(),
                book.id.clone(),
            )
        });
        Ok(books)
    }

    fn warn(&self, message: &str) {
        if let Ok(mut warnings) = self.warnings.lock() {
            warnings.push(message.to_owned());
        }
    }

    pub fn take_warnings(&self) -> Vec<String> {
        self.warnings
            .lock()
            .map(|mut messages| std::mem::take(&mut *messages))
            .unwrap_or_default()
    }

    pub fn cache_search_books(&self, books: &[Book]) -> Result<()> {
        for book in books.iter().filter(|book| book.source == "fanqie") {
            let mut metadata = book.clone();
            metadata.chapters.clear();
            metadata.validate()?;
            // A separate record per ID/title preserves aliases without a shared
            // read-modify-write index or adding unopened books to the shelf.
            let key = hash(format!("{}\0{}", metadata.id, metadata.title).as_bytes());
            Self::write(
                &self.root.join("search-index").join(format!("{key}.json")),
                metadata,
            )?;
        }
        Ok(())
    }

    pub fn cached_search_books(&self, query: &str) -> Result<Vec<Book>> {
        let entries = match fs::read_dir(self.root.join("search-index")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => bail!("无法读取搜索缓存目录"),
        };
        let normalize = |value: &str| {
            value
                .chars()
                .filter(|ch| ch.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        };
        let needle = normalize(query);
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let mut books = Vec::new();
        for entry in entries {
            let entry = entry.context("无法读取搜索缓存项目")?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let cached = (|| -> Result<Option<Book>> {
                let Some(book): Option<Book> = Self::read(&entry.path())? else {
                    return Ok(None);
                };
                book.validate()?;
                if book.source != "fanqie" || !book.chapters.is_empty() {
                    return Ok(None);
                }
                let key = hash(format!("{}\0{}", book.id, book.title).as_bytes());
                if entry.file_name().to_string_lossy() != format!("{key}.json") {
                    return Ok(None);
                }
                Ok(Some(book))
            })();
            match cached {
                Ok(Some(book))
                    if normalize(&book.title).contains(&needle)
                        || normalize(&book.author).contains(&needle)
                        || book.id == query =>
                {
                    books.push(book)
                }
                Ok(_) => {}
                Err(_) => self.warn("已跳过损坏的搜索缓存；原文件未修改"),
            }
        }
        books.sort_by(|a, b| a.id.cmp(&b.id).then(a.title.cmp(&b.title)));
        Ok(books)
    }

    pub fn cache_chapter(&self, id: &str, chapter: &str, text: &str) -> Result<()> {
        if text.trim().is_empty() || text.contains('\0') || text.len() as u64 > MAX_CHAPTER {
            bail!("章节正文为空、过大或不是有效文本");
        }
        Self::atomic(&self.chapter_path(id, chapter)?, text.as_bytes())
    }

    pub fn cached_chapter(&self, id: &str, chapter: &str) -> Result<Option<String>> {
        let Some(bytes) = Self::read_bytes(&self.chapter_path(id, chapter)?, MAX_CHAPTER)? else {
            return Ok(None);
        };
        match String::from_utf8(bytes) {
            Ok(text) if !text.trim().is_empty() && !text.contains('\0') => Ok(Some(text)),
            _ => {
                self.warn("已忽略损坏的章节缓存；原文件未修改");
                Ok(None)
            }
        }
    }

    pub fn progress(&self, id: &str) -> Result<Progress> {
        let Some(saved): Option<SavedProgress> =
            Self::read(&self.book_dir(id)?.join("progress.json"))?
        else {
            return Ok(Progress::default());
        };
        if saved.book_id != id {
            bail!("阅读进度书籍标识不匹配");
        }
        let progress = Progress {
            chapter: saved.chapter,
            fraction: saved.fraction,
        };
        progress.validate()?;
        Ok(progress)
    }

    pub fn save_progress(&self, id: &str, chapter: usize, fraction: f64) -> Result<()> {
        let progress = Progress { chapter, fraction };
        progress.validate()?;
        // Only progress.json is rewritten: catalog workers own metadata.json.
        Self::write(
            &self.book_dir(id)?.join("progress.json"),
            SavedProgress {
                book_id: id.to_owned(),
                chapter,
                fraction,
            },
        )?;
        Self::write(
            &self.root.join("recent.json"),
            Recent {
                book_id: id.to_owned(),
            },
        )
    }

    pub fn last_book(&self) -> Result<Option<String>> {
        let Some(recent): Option<Recent> = Self::read(&self.root.join("recent.json"))? else {
            return Ok(None);
        };
        valid_id(&recent.book_id)?;
        Ok(Some(recent.book_id))
    }
}

#[cfg(test)]
mod tests;
