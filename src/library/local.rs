use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{bail, Context, Result};
use encoding_rs::GB18030;
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::models::{Book, Chapter};

mod epub;

pub(super) use epub::import_epub;

const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MIN_CHUNK_CHARS: usize = 3000;
const TARGET_CHUNK_CHARS: usize = 4500;
const MAX_CHUNK_CHARS: usize = 6000;
const MAX_BOOK_CHAPTERS: usize = 200_000;

/// Return a complete snapshot; the caller must cache every body before saving the book.
pub(super) fn import_txt(path: &Path) -> Result<(Book, Vec<String>)> {
    let metadata = path.metadata().context("无法读取 TXT 文件信息")?;
    if !metadata.is_file() {
        bail!("TXT 导入只接受普通文件");
    }
    if metadata.len() > MAX_FILE_BYTES {
        bail!("TXT 文件不能超过 32 MiB");
    }
    let file = File::open(path).context("无法打开 TXT 文件")?;
    if !file.metadata().context("无法读取 TXT 文件信息")?.is_file() {
        bail!("TXT 导入只接受普通文件");
    }
    // The limit also applies when a file grows between metadata lookup and reading.
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("无法读取 TXT 文件")?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        bail!("TXT 文件不能超过 32 MiB");
    }
    let text = decode_text(&bytes)?;
    let sections = split_sections(&text)?;
    let (book, bodies) = make_book(
        filename_title(path),
        "local",
        "txt",
        "本地 TXT 完整快照；阅读不依赖原始文件。",
        sections,
    );
    book.validate()?;
    Ok((book, bodies))
}

fn decode_text(bytes: &[u8]) -> Result<String> {
    const BINARY_SIGNATURES: &[&[u8]] = &[
        b"%PDF-",
        b"\x89PNG\r\n\x1a\n",
        b"\xff\xd8\xff",
        b"GIF87a",
        b"GIF89a",
        b"PK\x03\x04",
        b"PK\x05\x06",
        b"PK\x07\x08",
        b"\x1f\x8b",
        b"BZh",
        b"\xfd7zXZ\0",
        b"7z\xbc\xaf\x27\x1c",
        b"Rar!",
        b"\x7fELF",
        b"SQLite format 3\0",
        b"RIFF",
        b"OggS",
        b"fLaC",
        b"ID3",
        b"\0asm",
    ];
    if bytes.is_empty() {
        bail!("TXT 文件为空");
    }
    let signature_bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    if BINARY_SIGNATURES
        .iter()
        .any(|signature| signature_bytes.starts_with(signature))
    {
        bail!("文件看起来是二进制数据，而不是 TXT 小说");
    }
    if bytes
        .iter()
        .any(|&byte| byte < 0x20 && !matches!(byte, b'\t' | b'\r' | b'\n') || byte == 0x7f)
    {
        bail!("TXT 文件包含 NUL 或不支持的控制字符");
    }
    let decoded = if let Some(body) = bytes.strip_prefix(b"\xef\xbb\xbf") {
        // An explicit UTF-8 BOM is authoritative, never a reason to guess GB18030.
        std::str::from_utf8(body)
            .context("带 BOM 的 TXT 文件不是有效的 UTF-8")?
            .to_owned()
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        text.to_owned()
    } else {
        GB18030
            .decode_without_bom_handling_and_without_replacement(bytes)
            .context("TXT 必须使用有效的 UTF-8 或 GB18030 编码")?
            .into_owned()
    };
    if decoded.len() as u64 > MAX_FILE_BYTES {
        bail!("TXT 解码后正文不能超过 32 MiB");
    }
    if decoded.chars().any(invalid_text_char) {
        bail!("TXT 文件包含控制字符、无效字符或二进制数据");
    }
    let normalized = decoded.replace("\r\n", "\n").replace('\r', "\n");
    let normalized = normalized.trim();
    if normalized.is_empty() {
        bail!("TXT 文件没有可阅读的正文");
    }
    Ok(normalized.to_owned())
}

fn invalid_text_char(ch: char) -> bool {
    let code = ch as u32;
    (ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        || matches!(code, 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x206f | 0xfeff | 0xfffd)
        || matches!(code, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
        || matches!(code, 0xfdd0..=0xfdef)
        || (code & 0xffff) >= 0xfffe
}

fn filename_title(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let clean: String = stem
        .chars()
        .map(|ch| {
            if ch == '_' || invalid_text_char(ch) {
                ' '
            } else {
                ch
            }
        })
        .collect();
    let title = clean.trim_matches(|ch: char| ch.is_whitespace() || ch == '.');
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        "未命名文本".to_owned()
    } else {
        title.chars().take(200).collect()
    }
}

fn split_sections(text: &str) -> Result<Vec<(String, String)>> {
    let chinese = Regex::new(r"^第[ \t]*[0-9０-９零〇一二三四五六七八九十百千万萬亿億两兩廿卅卌壹贰貳叁參肆伍陆陸柒捌玖拾佰仟][0-9０-９零〇一二三四五六七八九十百千万萬亿億两兩廿卅卌壹贰貳叁參肆伍陆陸柒捌玖拾佰仟 \t]*[章节回卷].*$").expect("valid heading regex");
    let english = Regex::new(concat!(
        r"(?i)^chapter[ \t]+(?P<number>[0-9]+|",
        r"(?:zero|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|hundred|thousand)",
        r"(?:[- \t]+(?:zero|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|hundred|thousand|and))*|[ivxlcdm]+)",
        r"(?:[ \t]*[.:：、\-—–][ \t]*.*|[ \t]+.*)?$"
    )).expect("valid heading regex");
    let roman = Regex::new(r"(?i)^M{0,3}(CM|CD|D?C{0,3})(XC|XL|L?X{0,3})(IX|IV|V?I{0,3})$")
        .expect("valid Roman numeral regex");
    let mut sections = Vec::new();
    let mut title: Option<String> = None;
    let mut body = String::new();
    for line in text.lines() {
        let candidate = line.trim();
        let is_heading = candidate.chars().count() <= 180
            && (chinese.is_match(candidate) || {
                english.captures(candidate).is_some_and(|captures| {
                    let number = &captures["number"];
                    !number.chars().all(|ch| "ivxlcdmIVXLCDM".contains(ch))
                        || roman.is_match(number)
                })
            });
        if is_heading {
            if let Some(previous) = title.take() {
                push_section(&mut sections, previous, &body);
            } else if !body.trim().is_empty() {
                push_section(&mut sections, "前言".to_owned(), &body);
            }
            if sections.len() >= MAX_BOOK_CHAPTERS {
                bail!("TXT 章节数量过多");
            }
            title = Some(candidate.split_whitespace().collect::<Vec<_>>().join(" "));
            body.clear();
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(previous) = title {
        push_section(&mut sections, previous, &body);
        Ok(sections)
    } else {
        Ok(chunk_unheaded(text)
            .into_iter()
            .enumerate()
            .map(|(index, content)| (format!("第 {} 部分", index + 1), content))
            .collect())
    }
}

fn push_section(sections: &mut Vec<(String, String)>, title: String, body: &str) {
    let body = body.trim();
    // Volume labels and consecutive headings still need a readable, nonempty snapshot.
    let content = if body.is_empty() {
        title.clone()
    } else {
        body.to_owned()
    };
    sections.push((title, content));
}

fn chunk_unheaded(text: &str) -> Vec<String> {
    let paragraph = Regex::new(r"\n[ \t]*\n+").expect("valid paragraph regex");
    let newline = Regex::new(r"\n").expect("valid newline regex");
    let sentence = Regex::new(r#"[。！？!?][”’"']?|\.[ \t]+"#).expect("valid sentence regex");
    let mut remaining = text.trim();
    let mut remaining_chars = remaining.chars().count();
    let mut chunks = Vec::new();
    while remaining_chars > MAX_CHUNK_CHARS {
        // Leave at least a normal-sized final chunk instead of a tiny trailing fragment.
        let upper_chars = MAX_CHUNK_CHARS.min(remaining_chars - MIN_CHUNK_CHARS);
        let target_chars = TARGET_CHUNK_CHARS.min(upper_chars);
        let offsets: Vec<usize> = remaining
            .char_indices()
            .take(upper_chars + 1)
            .map(|(byte, _)| byte)
            .collect();
        let lower = offsets[MIN_CHUNK_CHARS];
        let target = offsets[target_chars];
        let upper = offsets[upper_chars];
        let prefix = &remaining[..upper];
        let boundary = [&paragraph, &newline, &sentence]
            .iter()
            .find_map(|pattern| {
                pattern
                    .find_iter(prefix)
                    .map(|matched| matched.end())
                    .filter(|&end| end >= lower && end <= upper)
                    .min_by_key(|&end| end.abs_diff(target))
            })
            .unwrap_or(target);
        let (chunk, rest) = remaining.split_at(boundary);
        chunks.push(chunk.trim().to_owned());
        remaining_chars -= chunk.chars().count();
        let trimmed = rest.trim_start();
        remaining_chars -= rest[..rest.len() - trimmed.len()].chars().count();
        remaining = trimmed;
    }
    if !remaining.trim().is_empty() {
        chunks.push(remaining.trim().to_owned());
    }
    chunks
}

fn make_book(
    title: String,
    source: &str,
    namespace: &str,
    description: &str,
    sections: Vec<(String, String)>,
) -> (Book, Vec<String>) {
    let mut digest = Sha256::new();
    // Length framing makes the content identity independent of filenames and unambiguous.
    for (heading, body) in &sections {
        for value in [heading, body] {
            digest.update((value.len() as u64).to_le_bytes());
            digest.update(value.as_bytes());
        }
    }
    let id = format!("{namespace}:{:x}", digest.finalize());
    let mut chapters = Vec::with_capacity(sections.len());
    let mut bodies = Vec::with_capacity(sections.len());
    for (index, (title, body)) in sections.into_iter().enumerate() {
        chapters.push(Chapter {
            id: format!("{id}:{:04}", index + 1),
            title,
        });
        bodies.push(body);
    }
    (
        Book {
            id,
            title,
            author: String::new(),
            description: description.to_owned(),
            source: source.to_owned(),
            chapters,
        },
        bodies,
    )
}

/// This story is original demonstration material, not a platform novel or an extract.
pub(super) fn demo() -> (Book, Vec<String>) {
    let sections = vec![
        ("第一章 雨里的旧水尺".to_owned(), DEMO_ONE.to_owned()),
        ("第二章 灯下的空白图".to_owned(), DEMO_TWO.to_owned()),
        ("第三章 河流记住的名字".to_owned(), DEMO_THREE.to_owned()),
    ];
    let (mut book, bodies) = make_book(
        "听河人".to_owned(),
        "demo",
        "demo",
        "原创演示小说（demo:original）：专为本终端阅读器编写的虚构故事，用于离线阅读、章节缓存与进度演示；不是番茄平台作品，不代表任何真实作者或出版物。",
        sections,
    );
    book.author = "终端阅读器原创演示".to_owned();
    (book, bodies)
}

const DEMO_ONE: &str = r#"林汀回到白鹭镇的那天，雨把渡口的石阶洗成了深青色。她抱着祖母留下的木箱，在最后一班渡船上站了很久。箱子没有锁，只有一根磨白的棉线绕过提手，像是主人出门买菜之前随意打下的结。可祖母已经去世三个月，她始终没有勇气解开它。

镇上的人都知道，林家从前守着一根水尺。那根尺立在东堤边，刻着不同年份的洪水高度。林汀小时候觉得它像一棵不会长叶子的树，每逢夏天，祖母便带她去树下念数字，说念熟了这些数字，就能听出河水什么时候正在撒谎。

如今渡口旁立起了新的电子水位牌，蓝色数字在雨帘后安静闪动。来接她的修船师傅周伯说，旧尺早就不管用了，河道挖深以后，过去的刻痕只剩一点纪念价值。他说得轻松，手指却一直揉着雨衣上的裂口，没有看东边那条窄窄的堤路。

林汀拖着箱子走进老屋，先闻到干燥的橘皮气味。窗台上整齐排着六只空玻璃瓶，每只瓶底压着一张纸，写有日期和天气。祖母过去总把它们叫作河的耳朵。她曾经认真检查过，瓶里既没有水，也没有什么能够录音的机关，只有落进去的一点灰。

她终于解开棉线。箱中最上面是一件洗薄了的棉外套，下面压着厚厚一沓账纸。纸上没有钱数，只有船名、风向和奇怪的短句：午后有木头撞桥，夜里芦苇先倒，河心比岸边凉。最后一页写着，若有人回来，先不要相信新桥下面的安静。

屋后忽然传来敲门声。一个穿黄色雨鞋的女孩探进头来，怀里抱着浸湿的练习册，自称叫许豆，是邻居许师傅的孙女。她听说林家的姐姐做地图，便问能不能画一条不会淹水的上学路。练习册上的路画到新桥便停了，桥后是一片被橡皮擦得发毛的空白。

林汀原本只打算住两晚，把房屋交给中介，然后返回城里的测绘公司。她告诉许豆，地图只能画出已经知道的地方，不能保证明天不会下雨。女孩想了想，把练习册放在桌上，说不知道的地方也可以画一个问号，总比假装那里没有路要好。

傍晚雨小了一些，两人沿着东堤去找旧水尺。水尺还在，却被施工围挡遮住大半。围挡上贴着整齐的景观改造效果图，图中河水永远湛蓝，树影永远笔直，没有一个人提着裤脚走路。真正的河就在图后，颜色像刚磨开的旧墨，缓缓擦过石头。

林汀蹲下看水尺的底座，发现最低的三道刻痕都埋在新铺的碎石里。她想起祖母每年汛期都要带一把小铲，把尺脚周围清干净。那些细小的工作曾令她厌烦，仿佛大人永远可以为了同一块石头，耽搁一个孩子去看露天电影的整个傍晚。

许豆从围挡缝里伸进手，捡出一片折弯的铁皮。铁皮下面露出半个箭头，指向河岸一处几乎看不见的凹口。林汀擦去泥浆，认出那是祖母惯用的记号。她在账纸上见过同样的箭头，旁边写着两字：听口。字迹很重，像写字的人怕后来者轻易忽略。

所谓听口，其实是旧排水渠通向河道的出口。小时候涨水之前，这里总先传来低低的轰鸣，仿佛有人在地下搬动石磨。现在出口被新种的灌木挡住，连声音也被雨水吞没。林汀伏下身，耳朵靠近潮湿的砖面，听见里面有一种细而急的摩擦声。

周伯提着灯找来时，她们已经把灌木旁的落叶清开。渠口卡着一只破塑料筐，筐里塞满树枝和灰色泡沫。周伯没有立刻动手，而是先用木棍试了试水流。他说，堵塞不一定发生在眼前，贸然扯开这只筐，可能把更深处压住的东西一起带出来。

三个人在雨里做了简单的标记，退到安全的石阶上。林汀给镇防汛值班室打电话，报出渠口的位置，又把照片和时间发送过去。接电话的人问她是凭什么判断有风险。她望着木箱里带来的那页账纸，回答说现在还不能判断，所以才需要请人检查。

夜里，屋檐的水滴敲着铁盆，间隔忽长忽短。许豆被爷爷接走以后，老屋一下显得很空。林汀打开电脑，把原本准备发送给中介的消息删掉，另建了一张白鹭镇水路图。她没有急着落线，只在东堤、新桥、旧渠口各放了一个小小的问号。

临睡之前，她把祖母的空玻璃瓶搬到窗边。风经过瓶口，发出一种低而轻的声音。那并不神秘，不过是空气找到出口时产生的振动。可林汀忽然明白，祖母留下这些瓶子，不是要替河保守什么秘密，而是提醒后来的人，先停下来，认真听一次。

她在笔记本的第一页写下今天的日期，接着写下雨量、水位和三个尚未核实的问题。最后，她另外添了一句：旧水尺没有回答所有事情，却让我们知道，应该从哪里开始问。这一次，她不再急着把空白填满。窗外的河在夜色中继续向南，没有催促任何人。"#;

const DEMO_TWO: &str = r#"第二天清早，林汀被敲窗声叫醒。许豆站在院外，手里举着一袋还热的烧饼，说防汛站的人已经到了旧渠口。林汀来不及梳头，穿上雨靴便往东堤跑。小镇刚刚醒来，卖豆浆的推车冒着白汽，路面的积水把每一个匆忙的身影都拉得很长。

检查人员姓程，是个说话慢、做事很利落的年轻工程师。她先在渠口外设置警戒线，再查看上游检查井，最后把一台小摄像机送进管道。屏幕里出现成团的树根，还有一片倾斜的混凝土板。板子并没有彻底封住通道，却把原来的出口挤成一条窄缝。

程工解释，平时水量小，窄缝也能排走地面的雨水，看起来不会有明显异常。可一旦河水上涨，管内流速和压力都会变化，仅凭岸边的声音无法判断全部情况。她说林汀做对的不是猜中了什么，而是没有钻进排水渠，也没有自行搬动堵塞物。

清障需要专业队伍和设备，暂时不能开始。林汀便问，有没有可以先核对的旧图纸。程工从平板里调出一张管线图，图上的渠口离她们站的位置竟有几十步远，另一条支渠则直接穿过了已经建起房子的地块。许豆盯着屏幕说，这条线画进我家厨房了。

午后，她们去了镇档案室。管理员打开锈涩的柜门，抱出几卷潮软的图纸。最早的一张没有准确比例，旁边还画着两棵柳树。后来的图越画越清楚，街道有了编号，屋顶有了边界，唯独那条拐进菜市场背后的支渠，忽然在某一版中消失了。

管理员说，也许是改造时填平了，也许只是誊图的人漏了一笔。那些年施工队换过几次，纸上的签名有些已经联系不上。林汀没有把猜测写成结论，而是给每一张图拍照，记录出处、年代和不能确定的地方。她知道一条漂亮的线，比一句诚实的不知道更容易误导别人。

周伯听说她们查旧渠，带来了修船铺里的一块门板。门板背面刻满粗细不一的线，是他父亲年轻时记下的码头位置。他指着其中一个缺口，说过去每逢大雨，菜市场后边的井会先冒气泡。他不记得那口井叫什么，只记得卖糖人的摊子总摆在旁边。

卖糖人的老人已经去世，摊位如今成了手机维修店。店主起初摇头，后来翻出装修时的照片，照片角落果然露着半圈石头井沿。井口没有被填，只是盖上铁板，藏在柜台下面。程工赶到后请店主暂停营业，按程序检测，再打开盖板检查，没有让围观的人靠近。

夕阳斜照在空出来的柜台上，几块多年没有见光的地砖呈现不同的颜色。林汀站在警戒线外，看见手机维修店主把一张小板凳递给周伯。谁也没有因此变成英雄，人们只是慢慢把各自记得的一点事情交出来，好让同一张图上的空白不再那么陌生。

许豆却有些失望。她原本以为找到旧井以后，就能立刻画好那条不会淹水的上学路。林汀陪她坐在桥边的高处，解释说道路是否安全，要看实时雨情、水位和管理部门的通知，不能靠一张固定的地图保证。地图能做的，是把危险点和可选择的方向标得更清楚。

女孩在练习册上重新画了一遍，把原来鲜艳的蓝色路线改成了两条虚线。其中一条经过新桥，另一条绕过低洼的市场。她在旁边认真写道：大雨时先听学校和家长安排，不自己去河边查看。写完，她问林汀，这样是不是太不像一张厉害的地图。

林汀说，真正有用的地图，不一定让画图的人显得厉害。它应该让拿着地图的人知道什么已经核实，什么还要等待，什么时候应该停下来。许豆似懂非懂地点头，给两条虚线加上不同日期，还留了一小块地方，准备以后把变化过的路继续画上去。

夜里，老屋第一次来了许多客人。有人带旧照片，有人带施工收据，还有人什么都没带，只记得从前的水声。林汀把大纸铺在桌上，让每个人先说自己亲眼见过的事情。听来的故事记在另一边，不能确定的年份也不勉强拼凑，免得许多记忆互相借用了名字。

周伯说起自己的父亲，声音突然低下来。那年父亲为了转移一条漏水的船，错过了回家吃饭的时间，后来每逢汛期，母亲总要多留一盏灯。林汀这才发现，水路图里的每一个弯，都和谁家的饭桌、谁的担心连在一起，并不只是电脑屏幕上一组待修正的坐标。

客人散去之后，她给程工发送了整理好的资料，注明这些只供现场核查参考。她没有把村民的名字放进公开图层，只保留对工作必要的联系信息。随后，她给公司请了几天假。电话那头的同事问她忙什么，她想了想，说正在学习怎样为地图留下诚实的空白。

窗边的玻璃瓶被许豆贴上新标签，分别写着风、雨、河和等一等。第四只瓶子没有发声，却最让林汀喜欢。她把未完成的图卷好，压在木箱旁，第一次觉得祖母那一沓看似琐碎的记录并不沉重。它们不是要她背着过去走，而是让她和眼前的人一起，把路看得更明白。"#;

const DEMO_THREE: &str = r#"连续三天的雨终于在周末停下，白鹭镇的天空像洗净的玻璃，远处山脊显出清楚的轮廓。清障队伍已经完成旧渠的检查和修复，程工把现场记录逐项核对，提醒大家这不意味着以后不会积水。维护有周期，雨情有变化，安心不能只靠一次热闹的行动。

林汀没有组织庆祝，而是和许豆一起，把收集来的资料分成可以公开的地图、需要继续核查的清单和妥善保存的原件。祖母的账纸被放进干净纸袋，袋外写明来源和日期。她不再用那些模糊的句子直接证明什么，却也不舍得丢弃，因为每一句都留下了观察生活的方法。

东堤的围挡暂时挪开一角，施工方按照核实后的方案保留旧水尺的位置，同时在旁边设置新的说明牌。旧刻度并不代替现行测量基准，说明牌把这一点写得很清楚。林汀读了两遍，觉得这样的保留比单纯把旧物刷成鲜艳颜色更好，过去终于有了可以理解的位置。

周伯拎来一只装满工具的布袋，却没有像往常那样立即干活。他把袋子放在地上，问负责人员，什么是居民可以参与的，什么需要留给专业队伍。得到答复后，他只在指定区域清理岸上的废纸和落叶。许豆给他拍照，他摆摆手，说别把人拍得像又做了一件多大的事情。

上午，学校请林汀给孩子们讲一次地图。她没有展示公司的漂亮成果，也没有讲自己如何发现旧渠，只在黑板上画了教室、操场和校门，让孩子们标出平时容易忽略的地方。很快，图上出现了打滑的台阶、看不见来车的转角，以及一到下午就被花盆挡住的通道。

有个男孩把食堂的香味也画成一条线，惹得全班大笑。林汀没有擦掉它，而是在图例上添了一项：感受。她说地图可以记录许多事情，但感受、猜测和实际测量应该用不同符号表达。我们不必赶走自己的想象，只需要让别人知道，每一种线条分别依据什么。

许豆在最后一排举起手，问一张地图什么时候才算完成。林汀望着窗外正在生长的树，回答说，交付时可以有一个完成的版本，可道路和生活还会变化，所以要留下更新的时间，也留下提出问题的办法。女孩听完，在练习册封面认真写下了第一版三个字。

午后，她们沿着重新开放的步道走了一段。河水退去，石缝里挂着细细的草根，几只不知名字的小鸟在浅滩上跳动。许豆忍不住问，如果祖母还在，会不会夸她们做得好。林汀停下来看旧水尺的影子，说大概会先问，午饭吃了没有，鞋底有没有打滑。

这句话让两个人都笑起来。过去几天，她们谈了许多宏大的词，安全、记录、责任和记忆，可真正把人从屋里带出来的，常常只是一碗没吃上的热饭，或一条让孩子绕远了的上学路。那些小事并不比大道理轻，反而让大道理终于有了能落脚的地方。

回到老屋，林汀发现中介又发来一条消息，问房屋还要不要挂牌。她没有立刻作答，而是把每扇窗都打开，让潮湿的木头慢慢透气。留下房子不是一个动人的瞬间就能决定的事情，她要算修缮的费用，也要安排自己的工作，更要弄清楚以后谁来照看它。

晚上，她和周伯坐在门口商量，决定先请人检查屋顶，再把临街的一间小屋整理成资料角。借阅的规矩要简单明确，照片的主人要同意，孩子们来时要有大人照看。周伯说听起来像有很多麻烦。林汀点头，说是有很多，所以不能只靠一个人一时兴起。

许豆给资料角想了十几个响亮的名字，最后留下的却是听河小屋。门牌由维修店主用边角木料做成，没有雕花，只有一条弯弯的线。线的尽头故意没有封口，像一条仍要往前走的河。大家看了都说好，连一向嫌字写得不端正的周伯也没有挑毛病。

临回城的前夜，林汀再次打开祖母的木箱。在夹层里，她发现一张年轻时候的合影。照片中祖母站在水尺旁，手里拿着本子，身边有修船的人、卖菜的人和几个满脸泥点的孩子。原来那些记录从来不是一个人的功劳，只是最后由一个人小心地收进了箱子。

她把照片扫描存好，原件放回原处，没有擅自把每一张陌生的脸写上猜测的名字。然后她在自己的本子里补了一句：河流记住的，不是最响亮的那个人，而是那些愿意轮流守望的人。写完她又笑，觉得这句话太像书里的结尾，便在下面补上了屋顶维修师傅的电话号码。

第二天渡船离岸时，许豆站在岸上挥着练习册，喊她下次回来要看第二版地图。林汀说好，但不用等她，发现变化就先记下日期，遇到危险就找大人和专业人员。女孩郑重地点头，随后转身跑向等在安全护栏后面的爷爷，黄色雨鞋在晨光里一闪一闪。

船驶过新桥，河面铺着细碎的金光。林汀没有把木箱带走，只带了一份整理好的资料和一本新的空白本子。城里的工作还在等她，白鹭镇也不会停在这个晴朗的早晨。她想，所谓回来，也许不是永远留在原地，而是无论走到哪里，都知道该怎样认真地看，诚实地记，并把听见的声音交给下一个愿意倾听的人。"#;

#[cfg(test)]
mod tests;
