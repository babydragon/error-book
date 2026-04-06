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
        "无法从 LLM 响应中提取 JSON 内容。原始响应前500字符: {}",
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

/// 从 markdown 中提取配图坐标 [[x1, y1, x2, y2], ...]
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
        None => extract_json_and_json(raw),
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
pub fn parse_practice_response(raw: &str) -> Result<Vec<PracticeQuestion>> {
    let json_str = match extract_json_block(raw) {
        Some(json) => json,
        None => extract_json_and_json(raw),
    };

    let questions: Vec<PracticeQuestion> = serde_json::from_str(&json_str).with_context(|| {
        format!(
            "练习题 JSON 解析失败，原始内容: {}",
            &json_str[..json_str.len().min(500)]
        )
    })?;

    Ok(questions)
}

/// 从没有代码块的原始文本中提取 JSON
fn extract_json_and_json(raw: &str) -> String {
    // Try to find the best JSON boundary — prefer [ ] for arrays, { } for objects
    // Check for array first since practice responses are arrays
    let trimmed = raw.trim();

    // If the whole trimmed string starts with [ or {, return it directly
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        return trimmed.to_string();
    }

    // Try to find [ ] first (for array JSON like practice questions)
    if let Some(start) = raw.find('[') {
        if let Some(end) = raw.rfind(']') {
            if end > start {
                return raw[start..=end].to_string();
            }
        }
    }
    // Then try { }
    if let Some(start) = raw.find('{') {
        if let Some(end) = raw.rfind('}') {
            if end > start {
                return raw[start..=end].to_string();
            }
        }
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_full_response() {
        let raw = r#"## 原题

小明有 23 颗糖，平均分给 4 个小朋友，每人几颗？还剩几颗？

```json
{
    "subject": "数学",
    "classification": ["带余数除法", "应用题"],
    "reason": "学生未理解余数的概念",
    "suggestions": "通过实物操作帮助理解余数"
}
```"#;

        let (_, result) = parse_analysis_response(raw).unwrap();
        assert_eq!(result.subject, "数学");
        assert_eq!(result.classification, vec!["带余数除法", "应用题"]);
        assert!(result.original_question.contains("小明有"));
    }

    #[test]
    fn test_parse_with_image_regions() {
        let raw = r#"## 原题

看图回答问题 [[10, 20, 300, 200]]

```json
{
    "subject": "数学",
    "classification": ["钟表"],
    "reason": "不会读钟表",
    "suggestions": "练习读钟表"
}
```"#;

        let (_, result) = parse_analysis_response(raw).unwrap();
        let expected: Vec<Vec<f64>> = vec![vec![10.0, 20.0, 300.0, 200.0]];
        assert_eq!(result.image_regions, expected);
    }

    #[test]
    fn test_parse_summary_response_with_block() {
        let raw = r#"根据分析，总结如下：

```json
{
    "common_reasons": "计算粗心",
    "common_suggestions": "加强口算练习",
    "weak_points": ["进位加法", "退位减法"],
    "detail": "学生在进位和退位方面容易出错"
}
```"#;

        let summary = parse_summary_response(raw).unwrap();
        assert_eq!(summary.common_reasons, "计算粗心");
        assert_eq!(summary.weak_points, vec!["进位加法", "退位减法"]);
    }

    #[test]
    fn test_parse_summary_response_plain_json() {
        let raw = r#"{"common_reasons":"概念不清","common_suggestions":"多做练习","weak_points":["除法"],"detail":"需要加强"}"#;

        let summary = parse_summary_response(raw).unwrap();
        assert_eq!(summary.common_reasons, "概念不清");
    }

    #[test]
    fn test_parse_practice_response_with_block() {
        let raw = r#"
```json
[
    {
        "question": "25 ÷ 4 = ?",
        "answer": "6 余 1",
        "knowledge_points": ["带余数除法"]
    },
    {
        "question": "37 ÷ 5 = ?",
        "answer": "7 余 2",
        "knowledge_points": ["带余数除法", "除法"]
    }
]
```"#;

        let questions = parse_practice_response(raw).unwrap();
        assert_eq!(questions.len(), 2);
        assert_eq!(questions[0].question, "25 ÷ 4 = ?");
        assert_eq!(questions[1].knowledge_points, vec!["带余数除法", "除法"]);
    }

    #[test]
    fn test_parse_practice_response_plain_json() {
        let raw = r#"[{"question":"1+1","answer":"2","knowledge_points":["加法"]}]"#;

        let questions = parse_practice_response(raw).unwrap();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].answer, "2");
    }
}
