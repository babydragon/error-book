use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::db::models::AnalysisResult;

/// LLM 响应的 JSON 部分（错题分析）
#[derive(Debug, Deserialize)]
pub struct AnalysisJson {
    pub subject: String,
    pub classification: Vec<String>,
    pub reason: String,
    pub suggestions: String,
}

/// 总结响应的 JSON 结构
#[derive(Debug, Deserialize)]
pub struct SummaryJson {
    pub common_reasons: String,
    pub common_suggestions: String,
    pub weak_points: Vec<String>,
    pub detail: String,
}

/// 练习题的 JSON 结构
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PracticeQuestion {
    pub question: String,
    pub answer: String,
    pub knowledge_points: Vec<String>,
}

/// 解析 LLM 返回的错题分析响应
/// 预期格式：先 markdown 原题，后跟 JSON 代码块
pub fn parse_analysis_response(raw: &str) -> Result<(String, AnalysisResult)> {
    // 尝试提取 JSON 代码块
    let (markdown_part, json_part) = extract_markdown_and_json(raw)?;

    // 解析 JSON
    let analysis_json: AnalysisJson = serde_json::from_str(&json_part).with_context(|| {
        format!(
            "JSON 解析失败，原始内容: {}",
            &json_part[..json_part.len().min(500)]
        )
    })?;

    // 尝试从 markdown 中提取 image_regions
    let image_regions = extract_image_regions(&markdown_part);

    // 构造结果
    let result = AnalysisResult {
        subject: analysis_json.subject,
        classification: analysis_json.classification,
        original_question: markdown_part.trim().to_string(),
        image_regions,
        error_reason: analysis_json.reason,
        suggestions: analysis_json.suggestions,
    };

    Ok((result.original_question.clone(), result))
}

/// 从混合文本中分离 markdown 和 JSON
fn extract_markdown_and_json(raw: &str) -> Result<(String, String)> {
    // 尝试找 ```json ... ``` 代码块
    if let Some(json_content) = extract_json_block(raw) {
        // markdown 部分是 JSON 块之前的内容
        let markdown_part = if let Some(pos) = raw.find("```json") {
            &raw[..pos]
        } else if let Some(pos) = raw.find("```") {
            &raw[..pos]
        } else {
            raw
        };
        return Ok((markdown_part.to_string(), json_content));
    }

    // 如果没有代码块，尝试找最后一个 JSON 对象
    if let Some(start) = raw.rfind('{') {
        if let Some(end) = raw.rfind('}') {
            if end > start {
                let json_part = raw[start..=end].to_string();
                let markdown_part = raw[..start].to_string();
                // 验证是否为有效 JSON
                if serde_json::from_str::<serde_json::Value>(&json_part).is_ok() {
                    return Ok((markdown_part, json_part));
                }
            }
        }
    }

    // 最后手段：把整个内容当作 JSON
    Err(anyhow::anyhow!(
        "无法从 LLM 埥应中提取 JSON 内容。原始响应前500字符: {}",
        &raw[..raw.len().min(500)]
    ))
}

/// 提取 ```json ... ``` 代码块中的内容
fn extract_json_block(raw: &str) -> Option<String> {
    let start_marker = "```json";
    let end_marker = "```";

    let start_idx = raw.find(start_marker)?;
    let json_start = start_idx + start_marker.len();
    // 找结束的 ```
    let json_end = raw[json_start..].find(end_marker)?;
    Some(raw[json_start..json_start + json_end].trim().to_string())
}

/// 从 markdown 中提取配图坐标 [[x1, y1, x2, y2], ...] 格式
fn extract_image_regions(markdown: &str) -> Vec<Vec<f64>> {
    let mut regions = Vec::new();

    // 匹配 [[数字, 数字, 数字, 数字], ...] 格式
    // 先找最外层的 [[ ... ]]
    if let Some(start) = markdown.find("[[") {
        if let Some(end) = markdown.rfind("]]") {
            let region_str = &markdown[start..=end + 1]; // 包含 ]]
                                                         // 尝试解析为 Vec<Vec<f64>>
            if let Ok(parsed) = serde_json::from_str::<Vec<Vec<f64>>>(region_str) {
                regions = parsed;
            }
        }
    }

    regions
}

/// 解析 LLM 返回的总结响应
/// 预期格式：纯 JSON 或 ```json ... ``` 代码块
pub fn parse_summary_response(raw: &str) -> Result<SummaryJson> {
    let json_str = match extract_json_block(raw) {
        Some(json) => json,
        None => extract_bare_json(raw),
    };

    let summary: SummaryJson = serde_json::from_str(&json_str).with_context(|| {
        format!(
            "总结 JSON 解析失败，原始内容: {}",
            &json_str[..json_str.len().min(500)]
        )
    })?;

    Ok(summary)
}

/// 解析 LLM 返回的练习题响应
/// 预期格式：纯 JSON 数组 或 ```json ... ``` 代码块
/// 支持截断 JSON 的自动修复
pub fn parse_practice_response(raw: &str) -> Result<Vec<PracticeQuestion>> {
    let json_str = match extract_json_block(raw) {
        Some(json) => json,
        None => extract_bare_json(raw),
    };

    // 先尝试直接解析
    if let Ok(questions) = serde_json::from_str::<Vec<PracticeQuestion>>(&json_str) {
        return Ok(questions);
    }

    // 直接解析失败，尝试修复截断的 JSON
    let repaired = repair_truncated_json_array(&json_str);
    if let Ok(questions) = serde_json::from_str::<Vec<PracticeQuestion>>(&repaired) {
        tracing::warn!(
            "练习题 JSON 曾被截断，已自动修复（原始 {} 字节 → 修复后 {} 字节）",
            json_str.len(),
            repaired.len()
        );
        return Ok(questions);
    }

    Err(anyhow::anyhow!(
        "练习题 JSON 解析失败（含修复尝试），原始内容: {}",
        &json_str[..json_str.len().min(500)]
    ))
}

/// 从没有代码块的原始文本中提取裸 JSON
fn extract_bare_json(raw: &str) -> String {
    let trimmed = raw.trim();

    // 如果整个字符串以 [ 或 { 开头，直接返回
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        return trimmed.to_string();
    }

    // 尝试找 [ ] (用于数组)
    if let Some(start) = raw.find('[') {
        if let Some(end) = raw.rfind(']') {
            if end > start {
                return raw[start..=end].to_string();
            }
        }
    }

    // 然后尝试 { }
    if let Some(start) = raw.find('{') {
        if let Some(end) = raw.rfind('}') {
            if end > start {
                return raw[start..=end].to_string();
            }
        }
    }

    raw.to_string()
}

/// 修复截断的 JSON 数组
///
/// LLM 输出可能因为 token 限制导致 JSON 被截断，例如：
/// `[{"question": "...", "answer": "...", "knowledge_points": ["..."}`
/// 被截断为 `[{"question": "...", "answer": "...`
///
/// 皴略策略：从后往前扫描，找到最后一个完整的 JSON 对象，提取出来，
/// 独立解析每个完整对象，跳过不完整的。
fn repair_truncated_json_array(raw: &str) -> String {
    let trimmed = raw.trim();
    let inner = if trimmed.starts_with('[') {
        &trimmed[1..]
    } else {
        trimmed
    };

    // 扫描并提取所有完整的顶层 JSON 对象，忽略最后可能截断的不完整对象。
    let mut valid_objects = Vec::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut object_start: Option<usize> = None;

    for (idx, c) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }

        if in_string {
            match c {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match c {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    object_start = Some(idx);
                }
                depth += 1;
            }
            '}' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        if let Some(start) = object_start.take() {
                            let candidate = &inner[start..=idx];
                            if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                                valid_objects.push(candidate.to_string());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if !valid_objects.is_empty() {
        return format!("[{}]", valid_objects.join(","));
    }

    // 如果一个完整对象都没有找到，尝试对原始字符串做最小补全。
    // 使用栈记录开括号的顺序，以便按正确的逆序关闭。
    let mut result = trimmed.to_string();
    let mut bracket_stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;

    for c in result.chars() {
        if escaped {
            escaped = false;
            continue;
        }

        if in_string {
            match c {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match c {
            '"' => in_string = true,
            '{' => bracket_stack.push('{'),
            '}' => {
                if bracket_stack.last() == Some(&'{') {
                    bracket_stack.pop();
                }
            }
            '[' => bracket_stack.push('['),
            ']' => {
                if bracket_stack.last() == Some(&'[') {
                    bracket_stack.pop();
                }
            }
            _ => {}
        }
    }

    if in_string {
        result.push('"');
    }
    // 按逆嵌套顺序关闭所有未闭合的括号
    while let Some(opening) = bracket_stack.pop() {
        result.push(match opening {
            '{' => '}',
            '[' => ']',
            _ => continue,
        });
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (a) 截断的练习题 JSON：嵌套 knowledge_points 数组在截断时被正确关闭
    #[test]
    fn repair_truncated_practice_with_nested_knowledge_points() {
        // knowledge_points 数组在第二个元素后被截断
        let truncated = r#"[{"question":"What is 2+2?","answer":"4","knowledge_points":["arithmetic","addition"#;

        let repaired = repair_truncated_json_array(truncated);
        let parsed: Vec<PracticeQuestion> = serde_json::from_str(&repaired).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].question, "What is 2+2?");
        assert_eq!(parsed[0].answer, "4");
        assert_eq!(parsed[0].knowledge_points, vec!["arithmetic", "addition"]);
    }

    /// (b) 多个对象时，最后一个被截断，前面的完整对象被保留
    #[test]
    fn repair_keeps_complete_objects_and_drops_truncated_last() {
        let truncated = r#"[
            {"question":"Q1","answer":"A1","knowledge_points":["k1"]},
            {"question":"Q2","answer":"A2","knowledge_points":["k2"]},
            {"question":"Q3","answer":"A3","knowledge_points":["k3"#;

        let repaired = repair_truncated_json_array(truncated);
        let parsed: Vec<PracticeQuestion> = serde_json::from_str(&repaired).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].question, "Q1");
        assert_eq!(parsed[1].question, "Q2");
    }

    /// (c) 字符串中的转义引号不会破坏扫描逻辑
    #[test]
    fn escaped_quotes_in_strings_do_not_break_scanning() {
        let truncated = r#"[{"question":"He said \"hello\"","answer":"greeting","knowledge_points":["quotes"]}, {"question":"She replied \"world\"","answer":"response","knowledge_points":["more quotes"#;

        let repaired = repair_truncated_json_array(truncated);
        let parsed: Vec<PracticeQuestion> = serde_json::from_str(&repaired).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].question, r#"He said "hello""#);
        assert_eq!(parsed[0].knowledge_points, vec!["quotes"]);
    }

    /// 补充：验证逆序关闭逻辑——{ 在 [ 之上时先关 ] 再关 }
    #[test]
    fn fallback_closes_nested_brackets_in_reverse_order() {
        // 栈顺序: [ → { → [  → 关闭顺序: ] } ]
        let truncated = r#"[{"question":"Q","answer":"A","knowledge_points":["k1"#;

        let repaired = repair_truncated_json_array(truncated);
        let parsed: Vec<PracticeQuestion> = serde_json::from_str(&repaired).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].knowledge_points, vec!["k1"]);
    }

    /// 补充：完整 JSON（无截断）通过正常路径不被修改
    #[test]
    fn complete_json_passes_through_unchanged() {
        let complete = r#"[{"question":"Q1","answer":"A1","knowledge_points":["k1"]}]"#;
        let result = repair_truncated_json_array(complete);
        let parsed: Vec<PracticeQuestion> = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.len(), 1);
    }
}
