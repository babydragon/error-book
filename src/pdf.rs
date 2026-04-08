use crate::analysis::parser::PracticeQuestion;
use crate::config::PdfConfig;
use crate::db::models::PracticeSet;

use chrono::{Datelike, Timelike};
use typst::diag::FileError;
use typst::foundations::{Bytes, Datetime};
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{compile, Library, LibraryExt, World};
use typst_pdf::{pdf, PdfOptions};

/// PDF 文件信息
#[derive(Debug, Clone)]
pub struct PdfOutput {
    pub path: String,
}

/// 生成巩固练习 PDF（通过 Typst）
pub fn generate_pdf(
    practice: &PracticeSet,
    pdf_config: &PdfConfig,
    pdf_path: &str,
) -> anyhow::Result<PdfOutput> {
    let questions: Vec<PracticeQuestion> =
        serde_json::from_str(&practice.questions).unwrap_or_default();

    // 加载字体
    let (fonts, font_family) = load_fonts(&pdf_config.font_path)?;

    // 构建 Typst 标记源码
    let markup = build_typst_markup(&questions, &practice.subject, &font_family);

    // 创建 Typst World
    let mut world = TypstWorld::new(markup, fonts)?;

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
}

impl TypstWorld {
    fn new(markup: String, fonts: Vec<Font>) -> anyhow::Result<Self> {
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

        Ok(Self {
            library: LazyHash::new(library),
            book: LazyHash::new(book),
            fonts,
            source,
            main_id,
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

    fn file(&self, _id: FileId) -> Result<Bytes, FileError> {
        Err(FileError::NotFound(std::path::PathBuf::new()))
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
fn build_typst_markup(questions: &[PracticeQuestion], subject: &str, font_family: &str) -> String {
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

        let question_text = escape_typst(&sanitize_text(&q.question));
        for line in question_text.lines() {
            m.push_str(line);
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
        m.push_str(&format!("答案: {}\n", answer));

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
                },
                PracticeQuestion {
                    question: "请写出下列词语的反义词：\n大 - （  ）\n多 - （  ）\n快 - （  ）"
                        .to_string(),
                    answer: "大 - 小，多 - 少，快 - 慢".to_string(),
                    knowledge_points: vec!["反义词".to_string()],
                },
            ])
            .unwrap(),
            pdf_path: None,
            created_at: chrono::Utc::now().timestamp(),
        };

        let output_path = "data/test_practice.pdf";
        let Some(pdf_config) = test_pdf_config() else {
            eprintln!("⚠️  跳过 PDF 测试：未找到可用字体文件");
            return;
        };

        let result = generate_pdf(&practice, &pdf_config, output_path);
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
    }

    #[test]
    fn test_sanitize_text() {
        // 😀 U+1F600 is in the filtered range 0x1F300..=0x1FAFF
        assert_eq!(sanitize_text("😀 hello"), "hello");
        // ✅ U+2705 is in the filtered range; it should be removed.
        assert_eq!(sanitize_text("✅ done"), "done");
        assert_eq!(sanitize_text("  clean  "), "clean");
    }
}
