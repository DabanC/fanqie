use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::{bail, Context, Result};
use percent_encoding::percent_decode_str;
use zip::read::{ArchiveOffset, Config};
use zip::{CompressionMethod, ZipArchive};

const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DIRECTORY_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTRIES: usize = 8192;
const MAX_PATH_BYTES: usize = 1024;

pub(super) struct Archive {
    zip: ZipArchive<Cursor<Vec<u8>>>,
    remaining_compressed: u64,
    remaining_expanded: u64,
}

impl Archive {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let metadata = path.metadata().context("无法读取 EPUB 文件信息")?;
        if !metadata.is_file() {
            bail!("EPUB 导入只接受普通文件");
        }
        if metadata.len() > MAX_ARCHIVE_BYTES {
            bail!("EPUB 文件不能超过 32 MiB");
        }
        let file = File::open(path).context("无法打开 EPUB 文件")?;
        if !file.metadata().context("无法读取 EPUB 文件信息")?.is_file() {
            bail!("EPUB 导入只接受普通文件");
        }
        let mut bytes = Vec::new();
        file.take(MAX_ARCHIVE_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("无法读取 EPUB 文件")?;
        if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
            bail!("EPUB 文件不能超过 32 MiB");
        }

        let (count, start, footer) = directory_layout(&bytes)?;
        // Remove the optional archive comment so embedded signatures cannot select another footer.
        bytes.truncate(footer + 22);
        bytes[footer + 20..].fill(0);
        let mut zip = ZipArchive::with_config(
            Config {
                archive_offset: ArchiveOffset::Known(0),
            },
            Cursor::new(bytes),
        )
        .context("EPUB ZIP 目录无效")?;
        if zip.len() != count || zip.central_directory_start() != start as u64 {
            bail!("EPUB ZIP 目录包含重复条目或无效偏移");
        }
        let mut ranges = Vec::with_capacity(count);
        for index in 0..count {
            let entry = zip.by_index_raw(index).context("EPUB ZIP 条目无效")?;
            let name = std::str::from_utf8(entry.name_raw()).context("EPUB ZIP 路径不是 UTF-8")?;
            validate_name(name)?;
            let end = entry
                .data_start()
                .and_then(|offset| offset.checked_add(entry.compressed_size()))
                .filter(|&offset| offset <= start as u64)
                .context("EPUB ZIP 条目越过数据区")?;
            ranges.push((entry.header_start(), end));
        }
        ranges.sort_unstable();
        if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            bail!("EPUB ZIP 条目数据区重叠");
        }
        Ok(Self {
            zip,
            remaining_compressed: MAX_TOTAL_BYTES,
            remaining_expanded: MAX_TOTAL_BYTES,
        })
    }

    pub(super) fn read_xml(&mut self, name: &str, max_bytes: u64) -> Result<String> {
        let mut entry = self
            .zip
            .by_name(name)
            .context("EPUB 目录引用了不可读取的 ZIP 条目")?;
        if !entry.is_file()
            || entry
                .unix_mode()
                .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000))
            || !matches!(
                entry.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
        {
            bail!("EPUB 的 XML 或正文条目不是普通的存储或 DEFLATE 文件");
        }
        let limit = max_bytes.min(self.remaining_expanded);
        let declared = entry.size();
        if declared > limit || entry.compressed_size() > self.remaining_compressed {
            bail!("EPUB 条目超过大小上限或累计读取上限");
        }
        self.remaining_compressed -= entry.compressed_size();
        let mut data = Vec::new();
        (&mut entry)
            .take(limit + 1)
            .read_to_end(&mut data)
            .context("无法解压或校验 EPUB 条目")?;
        let actual = data.len() as u64;
        if actual > limit || actual != declared {
            bail!("EPUB 条目解压大小无效或超过上限");
        }
        self.remaining_expanded -= actual;
        String::from_utf8(data).context("EPUB 的 XML 和 XHTML 必须使用 UTF-8 编码")
    }
}

// Bound metadata before handing it to the ZIP parser, which allocates from the entry count.
// Files this small need neither ZIP64 nor multi-disk directories.
fn directory_layout(bytes: &[u8]) -> Result<(usize, usize, usize)> {
    const END_SIZE: usize = 22;
    const HEADER_SIZE: usize = 46;
    if bytes.len() < END_SIZE || !bytes.starts_with(b"PK\x03\x04") {
        bail!("EPUB 必须是有效的 ZIP 文件");
    }
    let lowest = bytes.len().saturating_sub(END_SIZE + usize::from(u16::MAX));
    let footer = (lowest..=bytes.len() - END_SIZE)
        .rev()
        .find(|&offset| {
            let end = &bytes[offset..];
            end.starts_with(b"PK\x05\x06") && END_SIZE + le16(end, 20) == end.len()
        })
        .context("EPUB 缺少有效的 ZIP 目录尾部")?;
    let end = &bytes[footer..];
    let count = le16(end, 10);
    let size = le32(end, 12);
    let start = le32(end, 16);
    if le16(end, 4) != 0
        || le16(end, 6) != 0
        || le16(end, 8) != count
        || !(1..=MAX_ENTRIES).contains(&count)
        || !(count * HEADER_SIZE..=MAX_DIRECTORY_BYTES).contains(&size)
        || start.checked_add(size) != Some(footer)
    {
        bail!("EPUB ZIP 目录超出上限或使用不支持的布局");
    }
    let mut directory = &bytes[start..footer];
    for _ in 0..count {
        if directory.len() < HEADER_SIZE || !directory.starts_with(b"PK\x01\x02") {
            bail!("EPUB ZIP 目录条目无效");
        }
        let name_len = le16(directory, 28);
        if !(1..=MAX_PATH_BYTES).contains(&name_len) {
            bail!("EPUB ZIP 路径为空或过长");
        }
        let entry_size = HEADER_SIZE + name_len + le16(directory, 30) + le16(directory, 32);
        directory = directory
            .get(entry_size..)
            .context("EPUB ZIP 目录条目越界")?;
    }
    if !directory.is_empty() {
        bail!("EPUB ZIP 目录的条目数不匹配");
    }
    Ok((count, start, footer))
}

fn le16(bytes: &[u8], offset: usize) -> usize {
    usize::from(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
}

fn le32(bytes: &[u8], offset: usize) -> usize {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]) as usize
}

fn valid_component(component: &str) -> bool {
    !component.is_empty()
        && !component.contains(['/', '\\', ':'])
        && !component.chars().any(char::is_control)
}

fn validate_name(name: &str) -> Result<()> {
    if name.len() > MAX_PATH_BYTES
        || name
            .strip_suffix('/')
            .unwrap_or(name)
            .split('/')
            .any(|part| !valid_component(part) || matches!(part, "." | ".."))
    {
        bail!("EPUB ZIP 包含不安全的路径");
    }
    Ok(())
}

pub(super) fn resolve_path(base: &str, reference: &str) -> Result<String> {
    if reference.is_empty() || reference.len() > MAX_PATH_BYTES || reference.contains(['#', '?']) {
        bail!("EPUB 引用必须指向包内文件，且不能包含片段或查询参数");
    }
    let mut parts: Vec<_> = base
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    let mut segments = reference.split('/').peekable();
    while let Some(segment) = segments.next() {
        let raw = segment.as_bytes();
        if raw.iter().enumerate().any(|(index, &byte)| {
            byte == b'%'
                && !raw
                    .get(index + 1..index + 3)
                    .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
        }) {
            bail!("EPUB 引用包含无效的百分号编码");
        }
        let part = percent_decode_str(segment)
            .decode_utf8()
            .context("EPUB 引用路径不是 UTF-8")?;
        if !valid_component(&part) {
            bail!("EPUB 引用包含不安全的路径");
        }
        match part.as_ref() {
            "." if segments.peek().is_some() => {}
            ".." if segments.peek().is_some() && parts.pop().is_some() => {}
            "." | ".." => bail!("EPUB 引用超出 ZIP 根目录或并未指向文件"),
            _ => parts.push(part.into_owned()),
        }
    }
    let name = parts.join("/");
    validate_name(&name)?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;

    #[test]
    fn repeated_member_reads_charge_both_budgets() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file("test.xml", options).unwrap();
        writer.write_all(b"<p/>").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        for (remaining_compressed, remaining_expanded) in [(7, 80), (80, 7)] {
            let zip = ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
            let mut archive = Archive {
                zip,
                remaining_compressed,
                remaining_expanded,
            };
            assert_eq!(archive.read_xml("test.xml", 10).unwrap(), "<p/>");
            assert!(archive.read_xml("test.xml", 10).is_err());
        }
    }
}
