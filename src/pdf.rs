use crate::analysis::parser::PracticeQuestion;
use crate::db::models::PracticeSet;
use printpdf::*;

/// PDF 文件信息
#[derive(Debug, Clone)]
pub struct PdfOutput {
    pub path: String,
}

/// 生成巩固练习 PDF
pub fn generate_pdf(practice: &PracticeSet, pdf_path: &str) -> anyhow::Result<PdfOutput> {
    let questions: Vec<PracticeQuestion> =
        serde_json::from_str(&practice.questions).unwrap_or_default();

    // 读取字体文件
    let font_bytes = read_font_bytes()?;

    let mut doc = PdfDocument::new(&format!("巩固练习 - {}", practice.subject));

    // 解析并注册字体
    let parsed_font = ParsedFont::from_bytes(&font_bytes, 0, &mut vec![])
        .ok_or_else(|| anyhow::anyhow!("字体解析失败"))?;
    let font_id = doc.add_font(&parsed_font);

    // 页面尺寸 (A4)
    let page_w = Mm(210.0);
    let page_h = Mm(297.0);

    // 内容区域边距 (mm)
    let margin_left = 20.0;
    let margin_top = 20.0;
    let margin_right = 20.0;
    let content_width_mm = 210.0 - margin_left - margin_right;

    let font_size_title: Pt = Pt(18.0);
    let font_size_heading: Pt = Pt(14.0);
    let font_size_body: Pt = Pt(11.0);
    let line_height: Pt = Pt(20.0);

    let mut question_lines: Vec<(Pt, String)> = Vec::new();
    let mut answer_lines: Vec<(Pt, String)> = Vec::new();
    let subject = sanitize_pdf_text(&practice.subject);

    question_lines.push((font_size_title, format!("巩固练习 - {}", subject)));
    question_lines.push((Pt(8.0), String::new()));

    answer_lines.push((font_size_title, format!("参考答案与知识点 - {}", subject)));
    answer_lines.push((Pt(8.0), String::new()));

    for (i, q) in questions.iter().enumerate() {
        question_lines.push((font_size_heading, format!("第 {} 题", i + 1)));
        question_lines.push((Pt(4.0), String::new()));

        // 题目文本（按换行拆分）
        let sanitized_question = sanitize_pdf_text(&q.question);
        for line in sanitized_question.lines() {
            for wrapped in wrap_line(line, content_width_mm, font_size_body.0) {
                question_lines.push((font_size_body, wrapped));
            }
        }
        question_lines.push((Pt(10.0), String::new()));

        answer_lines.push((font_size_heading, format!("第 {} 题", i + 1)));
        answer_lines.push((Pt(4.0), String::new()));

        let sanitized_answer = sanitize_pdf_text(&q.answer);
        for wrapped in wrap_line(
            &format!("答案: {}", sanitized_answer),
            content_width_mm,
            font_size_body.0,
        ) {
            answer_lines.push((font_size_body, wrapped));
        }
        answer_lines.push((Pt(2.0), String::new()));

        let kp = sanitize_pdf_text(&q.knowledge_points.join("、"));
        for wrapped in wrap_line(
            &format!("知识点: {}", kp),
            content_width_mm,
            font_size_body.0,
        ) {
            answer_lines.push((font_size_body, wrapped));
        }
        answer_lines.push((Pt(10.0), String::new()));
    }

    let mut pages: Vec<PdfPage> = Vec::new();
    pages.extend(paginate_lines(
        &question_lines,
        page_w,
        page_h,
        margin_left,
        margin_top,
        line_height,
        &font_id,
    ));

    if !questions.is_empty() {
        pages.extend(paginate_lines(
            &answer_lines,
            page_w,
            page_h,
            margin_left,
            margin_top,
            line_height,
            &font_id,
        ));
    }

    // 无内容时至少放一页标题
    if pages.is_empty() {
        pages.push(build_page(
            &[(font_size_title, format!("巩固练习 - {}", subject))].to_vec(),
            page_w,
            page_h,
            margin_left,
            margin_top,
            line_height,
            &font_id,
        ));
    }

    doc.with_pages(pages);

    let mut warnings = vec![];
    let bytes = doc.save(
        &PdfSaveOptions {
            subset_fonts: true,
            optimize: true,
            ..Default::default()
        },
        &mut warnings,
    );

    for w in &warnings {
        if w.severity != printpdf::PdfParseErrorSeverity::Info {
            tracing::warn!("PDF warning: {:?}", w);
        }
    }

    std::fs::write(pdf_path, &bytes)
        .map_err(|e| anyhow::anyhow!("写入 PDF 失败 {}: {}", pdf_path, e))?;

    tracing::info!(path = pdf_path, "PDF 已生成");
    Ok(PdfOutput {
        path: pdf_path.to_string(),
    })
}

fn paginate_lines(
    all_lines: &[(Pt, String)],
    page_w: Mm,
    page_h: Mm,
    margin_left: f32,
    margin_top: f32,
    line_height: Pt,
    font_id: &FontId,
) -> Vec<PdfPage> {
    let page_content_height_mm = 297.0 - margin_top - 20.0;
    let mm_per_pt = 25.4 / 72.0;

    let mut pages: Vec<PdfPage> = Vec::new();
    let mut current_lines: Vec<(Pt, String)> = Vec::new();
    let mut current_height_mm: f32 = 0.0;

    for (size, text) in all_lines.iter().cloned() {
        let line_mm = if text.is_empty() {
            size.0 * mm_per_pt
        } else {
            line_height.0 * mm_per_pt
        };

        if current_height_mm + line_mm > page_content_height_mm && !current_lines.is_empty() {
            pages.push(build_page(
                &current_lines,
                page_w,
                page_h,
                margin_left,
                margin_top,
                line_height,
                font_id,
            ));
            current_lines.clear();
            current_height_mm = 0.0;
        }

        current_height_mm += line_mm;
        current_lines.push((size, text));
    }

    if !current_lines.is_empty() {
        pages.push(build_page(
            &current_lines,
            page_w,
            page_h,
            margin_left,
            margin_top,
            line_height,
            font_id,
        ));
    }

    pages
}

/// 构建单个 PDF 页面
fn build_page(
    lines: &[(Pt, String)],
    page_w: Mm,
    page_h: Mm,
    margin_left: f32,
    margin_top: f32,
    default_line_height: Pt,
    font_id: &FontId,
) -> PdfPage {
    let mut ops = vec![
        Op::StartTextSection,
        Op::SetTextCursor {
            pos: Point {
                x: Mm(margin_left).into(),
                y: Mm(297.0 - margin_top).into(),
            },
        },
    ];

    let mut first_line = true;

    for (font_size, text) in lines {
        let line_height = if text.is_empty() {
            *font_size
        } else {
            default_line_height
        };

        if !first_line {
            ops.push(Op::SetLineHeight { lh: line_height });
            ops.push(Op::AddLineBreak);
        }
        first_line = false;

        if text.is_empty() {
            continue;
        }

        ops.push(Op::SetFont {
            font: PdfFontHandle::External(font_id.clone()),
            size: *font_size,
        });
        ops.push(Op::ShowText {
            items: vec![TextItem::Text(text.clone())],
        });
    }

    ops.push(Op::EndTextSection);
    PdfPage::new(page_w, page_h, ops)
}

/// 读取字体文件
fn read_font_bytes() -> anyhow::Result<Vec<u8>> {
    // 优先 Alibaba PuHuiTi，备选 NotoSansSC
    let candidates = [
        "fonts/Alibaba-PuHuiTi-Regular.otf",
        "fonts/NotoSansSC-Regular.ttf",
    ];
    for path in &candidates {
        if std::path::Path::new(path).exists() {
            let bytes = std::fs::read(path)
                .map_err(|e| anyhow::anyhow!("读取字体文件失败 {}: {}", path, e))?;
            if bytes.len() > 100_000 {
                return Ok(bytes);
            }
            tracing::warn!("字体文件 {} 过小 ({} bytes)，可能无效", path, bytes.len());
        }
    }
    anyhow::bail!("未找到有效的字体文件，请检查 fonts/ 目录")
}

/// 简单的自动换行：按字符数估算（CJK 字符宽度约为英文 2 倍）
fn wrap_line(line: &str, content_width_mm: f32, font_size_pt: f32) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }

    // 估算每行能放多少字符
    // 11pt 字体，大约每字符 3mm (CJK) 或 1.5mm (ASCII)
    let avg_char_width_mm = 3.0 * (font_size_pt / 11.0);

    let mut result = Vec::new();
    let mut current = String::new();
    let mut current_width: f32 = 0.0;

    for ch in line.chars() {
        let char_w = if ch.is_ascii() {
            avg_char_width_mm * 0.5
        } else {
            avg_char_width_mm
        };
        if current_width + char_w > content_width_mm && !current.is_empty() {
            result.push(current.clone());
            current.clear();
            current_width = 0.0;
        }
        current.push(ch);
        current_width += char_w;
    }

    if !current.is_empty() {
        result.push(current);
    }

    if result.is_empty() {
        result.push(line.to_string());
    }

    result
}

fn sanitize_pdf_text(s: &str) -> String {
    s.chars()
        .filter(|&ch| !is_emoji_like(ch))
        .collect::<String>()
        .trim()
        .to_string()
}

fn is_emoji_like(ch: char) -> bool {
    let code = ch as u32;
    matches!(
        code,
        0x200D
            | 0x20E3
            | 0xFE0F
            | 0x1F1E6..=0x1F1FF
            | 0x1F300..=0x1FAFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::PracticeSet;

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
        let result = generate_pdf(&practice, output_path);
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
        assert!(text.contains("第1题"), "Should contain question heading");
        assert!(
            !text.contains('😀'),
            "Should not contain emoji from question"
        );
        assert!(
            !text.contains('✅'),
            "Should not contain emoji from answer label/content"
        );

        println!("✅ PDF test passed - {} bytes", pdf_bytes.len());
    }

    #[test]
    fn test_read_font_bytes() {
        let bytes = read_font_bytes().expect("should find a font");
        assert!(
            bytes.len() > 100_000,
            "Font should be large enough, got {} bytes",
            bytes.len()
        );
        println!("Font file: {} bytes", bytes.len());
    }
}
