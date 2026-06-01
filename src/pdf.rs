use crate::analysis::parser::PracticeQuestion;
use crate::config::PdfConfig;
use crate::db::models::PracticeSet;

use chrono::{Datelike, Timelike};
use std::collections::HashMap;
use std::path::Path;
use typst::diag::FileError;
use typst::foundations::{Bytes, Datetime};
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{compile, Library, LibraryExt, World};
use typst_pdf::{pdf, PdfOptions};

type ImageAssetMap = HashMap<String, Bytes>;

/// PDF 文件信息
#[derive(Debug, Clone)]
pub struct PdfOutput {
    pub path: String,
}

/// 生成巩固练习 PDF（通过 Typst）
pub fn generate_pdf(
    practice: &PracticeSet,
    pdf_config: &PdfConfig,
    generated_image_dir: Option<&Path>,
    pdf_path: &str,
) -> anyhow::Result<PdfOutput> {
    let questions: Vec<PracticeQuestion> =
        serde_json::from_str(&practice.questions).unwrap_or_default();

    // 加载字体
    let (fonts, font_family) = load_fonts(&pdf_config.font_path)?;

    // 构建 Typst 标记源码
    let image_assets = collect_image_assets(&questions, generated_image_dir)?;
    let markup = build_typst_markup(&questions, &practice.subject, &font_family, &image_assets);

    // 创建 Typst World
    let mut world = TypstWorld::new(markup, fonts, image_assets)?;

    // 编译
    let warned = compile(&mut world);
    let document = warned
        .output
        .map_err(|errors| anyhow::anyhow!("Typst 编译失败: {:?}", errors))?;

    for warning in &warned.warnings {
        tracing::warn!("Typst warning: {:?}", warning);
    }

    // 导出 PDF
    let pdf_bytes = pdf(&document, &PdfOptions::default())
        .map_err(|e| anyhow::anyhow!("PDF 导出失败: {:?}", e))?;

    // 写入文件
    std::fs::write(pdf_path, &pdf_bytes)
        .map_err(|e| anyhow::anyhow!("写入 PDF 失败 {}: {}", pdf_path, e))?;

    tracing::info!(path = pdf_path, "PDF 已生成 (Typst)");
    Ok(PdfOutput {
        path: pdf_path.to_string(),
    })
}

// ============================================================
// Typst World 实现
// ============================================================

/// 最小化的 Typst World 实现，用于编译单文件
struct TypstWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    source: Source,
    main_id: FileId,
    files: HashMap<String, Bytes>,
}

impl TypstWorld {
    fn new(markup: String, fonts: Vec<Font>, image_assets: ImageAssetMap) -> anyhow::Result<Self> {
        let library = Library::builder().build();
        let book = {
            let mut b = FontBook::default();
            for font in &fonts {
                b.push(font.info().clone());
            }
            b
        };
        let main_id = FileId::new_fake(VirtualPath::new("/main.typ"));
        let source = Source::new(main_id, markup);
        let files = image_assets;

        Ok(Self {
            library: LazyHash::new(library),
            book: LazyHash::new(book),
            fonts,
            source,
            main_id,
            files,
        })
    }
}

impl World for TypstWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main_id
    }

    fn source(&self, id: FileId) -> Result<Source, FileError> {
        if id == self.main_id {
            Ok(self.source.clone())
        } else {
            Err(FileError::NotFound(std::path::PathBuf::new()))
        }
    }

    fn file(&self, id: FileId) -> Result<Bytes, FileError> {
        let key = id.vpath().as_rooted_path().to_string_lossy().to_string();
        self.files
            .get(&key)
            .cloned()
            .ok_or_else(|| FileError::NotFound(id.vpath().as_rooted_path().to_path_buf()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<i64>) -> Option<Datetime> {
        let now = chrono::Local::now();
        Datetime::from_ymd_hms(
            now.year(),
            now.month() as u8,
            now.day() as u8,
            now.hour() as u8,
            now.minute() as u8,
            now.second() as u8,
        )
    }
}

// ============================================================
// 字体加载
// ============================================================

/// 从配置指定路径加载字体文件，返回字体列表和首选字体族名
fn load_fonts(path: &std::path::Path) -> anyhow::Result<(Vec<Font>, String)> {
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("读取字体文件失败 {}: {}", path.display(), e))?;
    let data = Bytes::new(bytes);
    let fonts: Vec<Font> = Font::iter(data).collect();

    if fonts.is_empty() {
        anyhow::bail!("字体文件中未解析出有效字体: {}", path.display());
    }

    let family = fonts[0].info().family.clone();
    tracing::debug!(family = %family, count = fonts.len(), path = %path.display(), "字体已加载");
    Ok((fonts, family))
}

// ============================================================
// Typst 标记源码生成
// ============================================================

/// 构建 Typst 标记源码：题目在前，答案 + 知识点在新页开始
fn build_typst_markup(
    questions: &[PracticeQuestion],
    subject: &str,
    font_family: &str,
    image_assets: &ImageAssetMap,
) -> String {
    let subject = escape_typst(&sanitize_text(subject));
    let font_family = escape_typst(font_family);
    let generated_date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut m = String::with_capacity(4096);

    // ---------- 文档全局设置 ----------
    m.push_str(
        "#set page(paper: \"a4\", margin: (left: 20mm, top: 20mm, right: 20mm, bottom: 20mm), footer: context { align(right)[#text(size: 9pt)[第 #counter(page).display(\"1\") 页 / 共 #counter(page).final().at(0) 页]] })\n",
    );
    m.push_str(&format!(
        "#set text(font: \"{}\", size: 11pt)\n",
        font_family
    ));
    m.push_str("#set par(leading: 1em, justify: false)\n\n");

    // ---------- 题目部分 ----------
    m.push_str("#align(center)[\n");
    m.push_str(&format!(
        "  #text(size: 18pt, weight: \"bold\")[巩固练习 - {}]\n",
        subject
    ));
    m.push_str("]\n\n");
    m.push_str(&format!(
        "#align(center)[#text(size: 10pt, fill: luma(120))[生成时间：{}]]\n\n",
        generated_date
    ));
    m.push_str("#v(8pt)\n\n");

    for (i, q) in questions.iter().enumerate() {
        m.push_str(&format!(
            "#text(size: 14pt, weight: \"bold\")[第 {} 题]\n",
            i + 1
        ));
        m.push_str("#v(4pt)\n\n");

        let (question_text, image_note) = split_image_description(&q.question);
        let question_markup = render_question_markup(&question_text);
        m.push_str(&question_markup);
        m.push('\n');
        if let Some(image_markup) = question_image_markup(q, image_assets) {
            m.push_str("\n#v(4pt)\n");
            m.push_str(&image_markup);
            m.push('\n');
        }
        if let Some(note) = image_note {
            m.push_str("#v(2pt)\n");
            m.push_str(&image_note_markup(&note));
            m.push('\n');
        }
        m.push_str("\n#v(10pt)\n\n");
    }

    // ---------- 无内容时至少输出标题 ----------
    if questions.is_empty() {
        return m;
    }

    // ---------- 答案部分（新页开始） ----------
    m.push_str("#pagebreak()\n\n");

    m.push_str("#align(center)[\n");
    m.push_str(&format!(
        "  #text(size: 18pt, weight: \"bold\")[参考答案与知识点 - {}]\n",
        subject
    ));
    m.push_str("]\n\n");
    m.push_str(&format!(
        "#align(center)[#text(size: 10pt, fill: luma(120))[生成时间：{}]]\n\n",
        generated_date
    ));
    m.push_str("#v(8pt)\n\n");

    for (i, q) in questions.iter().enumerate() {
        m.push_str(&format!(
            "#text(size: 14pt, weight: \"bold\")[第 {} 题]\n",
            i + 1
        ));
        m.push_str("#v(4pt)\n\n");

        let answer = escape_typst(&sanitize_text(&q.answer));
        push_labeled_multiline_typst(&mut m, "答案", &answer);

        let kp = escape_typst(&sanitize_text(&q.knowledge_points.join("、")));
        m.push_str(&format!("知识点: {}\n", kp));

        m.push_str("\n#v(10pt)\n\n");
    }

    m
}

// ============================================================
// 文本清洗与转义
// ============================================================

/// 过滤 emoji 类字符并去除首尾空白
fn sanitize_text(s: &str) -> String {
    s.chars()
        .filter(|&ch| !is_emoji_like(ch))
        .collect::<String>()
        .trim()
        .to_string()
}

/// 转义 Typst 标记中的特殊字符
fn escape_typst(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 4);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '#' => out.push_str("\\#"),
            '$' => out.push_str("\\$"),
            '<' => out.push_str("\\<"),
            '>' => out.push_str("\\>"),
            '[' => out.push_str("\\["),
            ']' => out.push_str("\\]"),
            '*' => out.push_str("\\*"),
            '_' => out.push_str("\\_"),
            '@' => out.push_str("\\@"),
            '~' => out.push_str("\\~"),
            '`' => out.push_str("\\`"),
            _ => out.push(ch),
        }
    }
    out
}

fn render_question_markup(s: &str) -> String {
    let text = normalize_markdown_tables(&sanitize_text(s)).replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0usize;
    let mut parts: Vec<String> = Vec::new();

    while i < lines.len() {
        if let Some((consumed, table_markup)) = parse_markdown_table(&lines[i..]) {
            parts.push(table_markup);
            i += consumed;
            continue;
        }

        let line = lines[i];
        if line.trim().is_empty() {
            parts.push(String::from("#v(4pt)"));
        } else {
            parts.push(render_inline_markup(line));
        }
        i += 1;
    }

    parts.join("\n#linebreak()\n")
}

fn collect_image_assets(
    questions: &[PracticeQuestion],
    generated_image_dir: Option<&Path>,
) -> anyhow::Result<ImageAssetMap> {
    let Some(base_dir) = generated_image_dir else {
        return Ok(HashMap::new());
    };

    let mut assets = HashMap::new();
    for q in questions {
        let Some(relative) = q.image_path.as_deref() else {
            continue;
        };
        let path = base_dir.join(relative);
        if !path.is_file() {
            tracing::warn!(path = %path.display(), "练习题配图文件不存在，PDF 中将跳过该图片");
            continue;
        }

        let bytes = std::fs::read(&path)
            .map(Bytes::new)
            .map_err(|e| anyhow::anyhow!("读取练习题配图失败 {}: {}", path.display(), e))?;
        let virtual_path = format!("/practice-images/{}", relative.replace('\\', "/"));
        assets.insert(virtual_path, bytes);
    }
    Ok(assets)
}

fn question_image_markup(q: &PracticeQuestion, image_assets: &ImageAssetMap) -> Option<String> {
    let relative = q.image_path.as_deref()?;
    let virtual_path = format!("/practice-images/{}", relative.replace('\\', "/"));
    if !image_assets.contains_key(&virtual_path) {
        return None;
    }

    Some(format!(
        "#align(center)[#image(\"{}\", width: {})]",
        escape_typst(&virtual_path),
        image_width_for_question(q)
    ))
}

fn image_width_for_question(q: &PracticeQuestion) -> &'static str {
    use crate::practice::planner::PracticeImageType;
    match q.image_type {
        Some(
            PracticeImageType::GeometryDiagram
            | PracticeImageType::Table
            | PracticeImageType::BarChart
            | PracticeImageType::LineChart
            | PracticeImageType::PieChart
            | PracticeImageType::SequenceDiagram
            | PracticeImageType::MapOrLayout,
        ) => "52%",
        _ => "60%",
    }
}

fn image_note_markup(note: &str) -> String {
    format!(
        "#align(left)[#text(size: 9pt, fill: luma(90))[图片说明：{}]]",
        escape_typst(&sanitize_text(note))
    )
}

fn split_image_description(question_text: &str) -> (String, Option<String>) {
    let start_marker = "[图片说明：";
    let Some(start) = question_text.find(start_marker) else {
        return (question_text.to_string(), None);
    };
    let prefix = &question_text[..start];
    let rest = &question_text[start + start_marker.len()..];
    let Some(end) = rest.find(']') else {
        return (question_text.to_string(), None);
    };
    let note = rest[..end].trim().to_string();
    let suffix = &rest[end + 1..];
    (format!("{}{}", prefix, suffix), Some(note))
}

fn render_inline_markup(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    let mut out = String::with_capacity(s.len() + 32);

    while i < chars.len() {
        if let Some((consumed, width_mm)) = match_blank_at(&chars[i..]) {
            out.push_str(&blank_markup(width_mm));
            i += consumed;
            continue;
        }

        out.push_str(&escape_typst(&chars[i].to_string()));
        i += 1;
    }
    out
}

fn parse_markdown_table(lines: &[&str]) -> Option<(usize, String)> {
    if lines.len() < 2 {
        return None;
    }
    if !looks_like_table_row(lines[0]) || !looks_like_separator_row(lines[1]) {
        return None;
    }

    let mut consumed = 2usize;
    let mut rows = vec![parse_table_row(lines[0])];
    while consumed < lines.len() && looks_like_table_row(lines[consumed]) {
        rows.push(parse_table_row(lines[consumed]));
        consumed += 1;
    }
    if rows.is_empty() || rows[0].is_empty() {
        return None;
    }

    let columns = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut cells = Vec::new();
    for row in rows {
        for idx in 0..columns {
            let cell = row.get(idx).cloned().unwrap_or_default();
            cells.push(format!("[{}]", render_inline_markup(&cell)));
        }
    }

    Some((
        consumed,
        format!(
            "#table(columns: {}, inset: 6pt, stroke: 0.5pt + luma(180), {})",
            columns,
            cells.join(", ")
        ),
    ))
}

fn normalize_markdown_tables(s: &str) -> String {
    let normalized = s.replace('│', "|").replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i].trim_end();
        if !looks_like_table_row(line) {
            out.push(line.to_string());
            i += 1;
            continue;
        }

        let mut row_buf = line.to_string();
        let mut consumed = 1usize;
        while (row_buf.matches('|').count() < 2 || non_empty_table_cells(&row_buf) < 2) && i + consumed < lines.len() {
            let next = lines[i + consumed].trim();
            if next.is_empty() {
                break;
            }
            row_buf.push(' ');
            row_buf.push_str(next);
            consumed += 1;
        }

        let mut handled_pair = false;
        if i + consumed < lines.len() {
            let mut next_idx = i + consumed;
            while next_idx < lines.len() {
                let next = lines[next_idx].trim();
                if next.is_empty() {
                    next_idx += 1;
                    continue;
                }
                if !looks_like_separator_row(next) && looks_like_table_row(next) {
                    let next_cols = parse_table_row(next).len();
                    let header_cols = parse_table_row(&row_buf).len();
                    if header_cols >= 2 && next_cols >= header_cols {
                        out.push(clean_table_row(&row_buf));
                        out.push(markdown_separator_row(header_cols));
                        out.push(clean_table_row(next));
                        i = next_idx + 1;
                        handled_pair = true;
                        break;
                    }
                }
                break;
            }
        }

        if handled_pair {
            continue;
        }

        out.push(clean_table_row(&row_buf));
        i += consumed;
    }

    insert_missing_table_separators(&out).join("\n")
}

fn clean_table_row(line: &str) -> String {
    let row = line.replace('│', "|").replace("(\n", "(").replace("\n)", ")");
    let collapsed = row.split_whitespace().collect::<Vec<_>>().join(" ");
    if looks_like_table_row(&collapsed) {
        let mut cells = parse_table_row(&collapsed);
        if cells.len() >= 2 && cells.first().is_some_and(|c| c.trim().is_empty()) {
            cells.remove(0);
        }
        format!("| {} |", cells.join(" | "))
    } else {
        collapsed
    }
}

fn non_empty_table_cells(line: &str) -> usize {
    parse_table_row(line)
        .into_iter()
        .filter(|cell| !cell.trim().is_empty() && cell.trim() != "│")
        .count()
}

fn markdown_separator_row(columns: usize) -> String {
    format!("| {} |", vec!["---"; columns].join(" | "))
}

fn insert_missing_table_separators(lines: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        out.push(lines[i].clone());
        if i + 1 < lines.len()
            && looks_like_table_row(&lines[i])
            && !looks_like_separator_row(&lines[i + 1])
            && looks_like_table_row(&lines[i + 1])
        {
            let cols = parse_table_row(&lines[i]).len();
            if cols >= 2 {
                out.push(markdown_separator_row(cols));
            }
        }
        i += 1;
    }
    out
}

fn looks_like_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('|') && trimmed.matches('|').count() >= 2
}

fn looks_like_separator_row(line: &str) -> bool {
    let trimmed = line.trim();
    if !looks_like_table_row(trimmed) {
        return false;
    }
    trimmed
        .chars()
        .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

fn parse_table_row(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

#[cfg(test)]
fn push_multiline_typst(out: &mut String, text: &str) {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut parts = normalized.split('\n').peekable();

    while let Some(line) = parts.next() {
        out.push_str(line);
        out.push('\n');
        if parts.peek().is_some() {
            out.push_str("#linebreak()\n");
        }
    }
}

fn push_labeled_multiline_typst(out: &mut String, label: &str, text: &str) {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut parts = normalized.split('\n');

    if let Some(first) = parts.next() {
        out.push_str(&format!("{}: {}\n", label, first));
    } else {
        out.push_str(&format!("{}:\n", label));
        return;
    }

    for line in parts {
        out.push_str("#linebreak()\n");
        out.push_str(line);
        out.push('\n');
    }
}

fn match_blank_at(chars: &[char]) -> Option<(usize, usize)> {
    if chars.is_empty() {
        return None;
    }

    let first = chars[0];
    if first == '（' || first == '(' {
        let closing = if first == '（' { '）' } else { ')' };
        let mut j = 1usize;
        while j < chars.len() && chars[j] != closing {
            j += 1;
        }
        if j < chars.len() {
            let inner: String = chars[1..j].iter().collect();
            let trimmed = inner.trim();
            let looks_blank = trimmed.is_empty()
                || trimmed
                    .chars()
                    .all(|c| c == '_' || c == '＿' || c.is_whitespace());
            if looks_blank {
                let blank_len = inner.chars().filter(|c| *c == '_' || *c == '＿').count();
                return Some((j + 1, blank_spaces(blank_len)));
            }
        }
    }

    None
}

fn blank_spaces(blank_len: usize) -> usize {
    match blank_len {
        0..=2 => 6,
        3..=4 => 8,
        5..=6 => 10,
        _ => 12,
    }
}

fn blank_markup(space_count: usize) -> String {
    format!("（{}）", "　".repeat(space_count))
}

fn is_emoji_like(ch: char) -> bool {
    let code = ch as u32;
    matches!(
        code,
        0x200D
            | 0x20E3
            | 0xFE0F
            | 0x2600..=0x27BF
            | 0x1F1E6..=0x1F1FF
            | 0x1F300..=0x1FAFF
    )
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PdfConfig;
    use crate::db::models::PracticeSet;
    use std::path::PathBuf;

    fn test_pdf_config() -> Option<PdfConfig> {
        let candidates = [
            "fonts/Alibaba-PuHuiTi-Regular.otf",
            "fonts/NotoSansSC-Regular.ttf",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        ];

        for candidate in candidates {
            let path = PathBuf::from(candidate);
            if path.is_file() && load_fonts(&path).is_ok() {
                return Some(PdfConfig { font_path: path });
            }
        }

        None
    }

    #[test]
    fn test_generate_pdf_with_sample_data() {
        let practice = PracticeSet {
            id: "test-practice-id".to_string(),
            summary_id: "test-summary-id".to_string(),
            subject: "语文".to_string(),
            requirements: Some("偏重阅读理解".to_string()),
            questions: serde_json::to_string(&vec![
                PracticeQuestion {
                    question: "😀 小明有3个苹果，给了小红1个，还剩几个？请列出算式。".to_string(),
                    answer: "✅ 3 - 1 = 2，还剩2个苹果。".to_string(),
                    knowledge_points: vec!["加减法".to_string(), "应用题".to_string()],
                    question_form: None,
                    material_type: None,
                    requires_image: false,
                    image_role: None,
                    image_type: None,
                    dependency_mode: None,
                    image_spec: None,
                    image_path: None,
                },
                PracticeQuestion {
                    question: "请写出下列词语的反义词：\n大 - （  ）\n多 - （  ）\n快 - （  ）"
                        .to_string(),
                    answer: "大 - 小，多 - 少，快 - 慢".to_string(),
                    knowledge_points: vec!["反义词".to_string()],
                    question_form: None,
                    material_type: None,
                    requires_image: false,
                    image_role: None,
                    image_type: None,
                    dependency_mode: None,
                    image_spec: None,
                    image_path: None,
                },
            ])
            .unwrap(),
            pdf_path: None,
            created_at: chrono::Utc::now().timestamp(),
            data_version: 1,
            generator_version: None,
        };

        let output_path = "data/test_practice.pdf";
        let Some(pdf_config) = test_pdf_config() else {
            eprintln!("⚠️  跳过 PDF 测试：未找到可用字体文件");
            return;
        };

        let result = generate_pdf(&practice, &pdf_config, None, output_path);
        assert!(result.is_ok(), "PDF generation failed: {:?}", result.err());

        let pdf_bytes = std::fs::read(output_path).expect("PDF file should exist");
        assert!(
            pdf_bytes.len() > 1000,
            "PDF too small: {} bytes",
            pdf_bytes.len()
        );
        assert_eq!(&pdf_bytes[0..5], b"%PDF-", "Should be a valid PDF file");

        // 用 pdftotext 验证内容
        let txt_output = std::process::Command::new("pdftotext")
            .args([output_path, "-"])
            .output()
            .expect("pdftotext should exist");
        let text = String::from_utf8_lossy(&txt_output.stdout);
        println!("PDF text content:\n{}", text);
        assert!(text.contains("巩固练习"), "Should contain title");
        assert!(
            text.contains("参考答案与知识点"),
            "Should contain answer section title"
        );
        assert!(text.contains("生成时间："), "Should contain generated date");
        assert!(
            text.contains("第1题") || text.contains("第 1 题"),
            "Should contain question heading"
        );
        assert!(
            text.contains("第 1 页 / 共 2 页")
                || text.contains("第1页/共2页")
                || text.contains("第 1 页/共 2 页")
                || text.contains("第1页 / 共2页"),
            "Should contain page number format"
        );
        assert!(
            !text.contains('\u{1F600}'),
            "Should not contain emoji from question"
        );
        assert!(
            !text.contains('\u{2705}'),
            "Should not contain emoji from answer label/content"
        );

        println!("PDF test passed - {} bytes", pdf_bytes.len());
    }

    #[test]
    fn test_load_fonts() {
        let Some(pdf_config) = test_pdf_config() else {
            eprintln!("⚠️  跳过字体测试：未找到可用字体文件");
            return;
        };

        let (fonts, family) = load_fonts(&pdf_config.font_path).expect("should find fonts");
        assert!(!fonts.is_empty(), "Should load at least one font");
        assert!(!family.is_empty(), "Font family name should not be empty");
        println!("Font family: {}, count: {}", family, fonts.len());
    }

    #[test]
    fn test_escape_typst() {
        assert_eq!(escape_typst("hello"), "hello");
        assert_eq!(escape_typst("a#b"), "a\\#b");
        assert_eq!(escape_typst("a\\b"), "a\\\\b");
        assert_eq!(escape_typst("$x$"), "\\$x\\$");
        assert_eq!(escape_typst("*bold*"), "\\*bold\\*");
        assert_eq!(escape_typst("x < y > z"), "x \\< y \\> z");
    }

    #[test]
    fn test_sanitize_text() {
        // 😀 U+1F600 is in the filtered range 0x1F300..=0x1FAFF
        assert_eq!(sanitize_text("😀 hello"), "hello");
        // ✅ U+2705 is in the filtered range; it should be removed.
        assert_eq!(sanitize_text("✅ done"), "done");
        assert_eq!(sanitize_text("  clean  "), "clean");
    }

    #[test]
    fn test_render_question_markup_expands_parenthesis_blank() {
        let rendered = render_question_markup("请填空：大 - （  ）\n多 - ( )");
        assert!(rendered.contains("（　　　"));
        assert!(!rendered.contains("（  ）"));
        assert!(!rendered.contains("( )"));
    }

    #[test]
    fn test_render_question_markup_expands_underscores() {
        let rendered = render_question_markup("请填写 ____ 和 ______");
        assert!(rendered.contains("\\_\\_\\_\\_"));
        assert!(rendered.contains("\\_\\_\\_\\_\\_\\_"));
    }

    #[test]
    fn test_push_multiline_typst_inserts_linebreaks() {
        let mut out = String::new();
        push_multiline_typst(&mut out, "第一行\n第二行\n第三行");
        assert!(out.contains("第一行\n#linebreak()\n第二行\n#linebreak()\n第三行"));
    }

    #[test]
    fn test_push_labeled_multiline_typst_preserves_answer_newlines() {
        let mut out = String::new();
        push_labeled_multiline_typst(&mut out, "答案", "步骤1\n步骤2");
        assert!(out.contains("答案: 步骤1\n#linebreak()\n步骤2\n"));
    }

    #[test]
    fn test_split_image_description_extracts_note() {
        let (question, note) = split_image_description("题干\n[图片说明：一个计数器，有2颗珠子。]\n后文");
        assert!(question.contains("题干"));
        assert!(question.contains("后文"));
        assert!(!question.contains("图片说明"));
        assert_eq!(note.as_deref(), Some("一个计数器，有2颗珠子。"));
    }

    #[test]
    fn test_parse_markdown_table_detects_basic_table() {
        let lines = vec!["| 姓名 | 分数 |", "| --- | --- |", "| 小明 | 95 |", "| 小红 | 88 |"]; 
        let parsed = parse_markdown_table(&lines).unwrap();
        assert_eq!(parsed.0, 4);
        assert!(parsed.1.contains("#table(columns: 2"));
        assert!(parsed.1.contains("小明"));
    }

    #[test]
    fn test_normalize_markdown_tables_repairs_two_row_table() {
        let raw = "| 2本 | 8本 | 32本 | 38本 |\n|│\n│( ) | 科技书 | 故事书 | ( ) |";
        let normalized = normalize_markdown_tables(raw);
        assert!(normalized.contains("| --- | --- | --- | --- |"), "{}", normalized);
        assert!(normalized.contains("| ( ) | 科技书 | 故事书 | ( ) |"), "{}", normalized);
    }

    #[test]
    fn test_image_width_for_precise_question_is_smaller() {
        let q = PracticeQuestion {
            question: "表格题".to_string(),
            answer: "略".to_string(),
            knowledge_points: vec!["表格读取".to_string()],
            question_form: None,
            material_type: None,
            requires_image: true,
            image_role: None,
            image_type: Some(crate::practice::planner::PracticeImageType::Table),
            dependency_mode: None,
            image_spec: None,
            image_path: Some("abc.png".to_string()),
        };
        assert_eq!(image_width_for_question(&q), "52%");
    }

    #[test]
    fn test_question_image_markup_renders_registered_asset() {
        let q = PracticeQuestion {
            question: "看图回答".to_string(),
            answer: "略".to_string(),
            knowledge_points: vec!["看图题".to_string()],
            question_form: None,
            material_type: None,
            requires_image: true,
            image_role: None,
            image_type: None,
            dependency_mode: None,
            image_spec: None,
            image_path: Some("abc.png".to_string()),
        };
        let mut assets = HashMap::new();
        assets.insert(
            "/practice-images/abc.png".to_string(),
            Bytes::new(Vec::new()),
        );

        let markup = question_image_markup(&q, &assets).unwrap();
        assert!(markup.contains("#image"));
        assert!(markup.contains("/practice-images/abc.png"));
    }
}
