use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::db::models::AnalysisResult;
use crate::practice::planner::PracticePlan;
use crate::practice::planner::{
    PracticeDependencyMode, PracticeImageRole, PracticeImageSpec, PracticeImageType,
};

/// UTF-8 safe truncation for error/log messages.
/// Truncates to at most `max_chars` Unicode characters, appending "…" when truncated.
fn truncate_for_error(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}…", truncated)
    }
}

// ═══════════════════════════════════════════════════════════════
// Legacy combined structs (existing)
// ═══════════════════════════════════════════════════════════════

/// LLM 响应的 JSON 部分（错题分析）
#[derive(Debug, Deserialize)]
pub struct AnalysisJson {
    pub subject: String,
    pub classification: Vec<String>,
    pub reason: String,
    pub suggestions: String,
}

// ── 结构化解析产物（用于 canonicalize 阶段）──

/// 从 LLM 响应中提取的题目内容
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionExtraction {
    /// 原题 markdown（已去除批注/涂改等噪声）
    pub question_markdown: String,
    /// 配图坐标 [[x1, y1, x2, y2], ...]
    pub image_regions: Vec<Vec<f64>>,
}

/// 从 LLM 响应中提取的教学分析内容
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PedagogicalAnalysis {
    /// 科目（语文、数学等）
    pub subject: String,
    /// 知识点标签
    pub classification: Vec<String>,
    /// 错误原因
    pub error_reason: String,
    /// 改进建议
    pub suggestions: String,
}

/// 单次 legacy combined 调用解析后的结构化产物
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedLegacyAnalysis {
    pub question_extraction: QuestionExtraction,
    pub pedagogical_analysis: PedagogicalAnalysis,
}

// ═══════════════════════════════════════════════════════════════
// Multi-stage pipeline structs
// ═══════════════════════════════════════════════════════════════

// ── Stage 1: Vision Recognition ─────────────────────────────

/// Stage 1 output: visual recognition / OCR result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionRecognitionResult {
    /// All recognized text (print + handwriting)
    pub recognized_text: String,
    /// Clean question text (no annotations, no student answer)
    pub question_text_clean: String,
    /// Layout description
    pub layout_description: String,
    /// Student answer text
    pub student_answer_text: String,
    /// Teacher marks
    #[serde(default)]
    pub teacher_marks: Vec<TeacherMark>,
    /// Image regions from vision
    #[serde(default)]
    pub image_regions: Vec<Vec<f64>>,
    /// Overall handwriting recognition confidence
    pub handwriting_confidence: Option<f64>,
}

/// A single teacher mark on the paper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeacherMark {
    /// 对号|叉号|半对|分数|批注|圈画
    pub mark_type: String,
    /// Where in the question
    pub location_description: String,
    /// The actual mark content
    pub mark_content: String,
}

// ── Stage 2: Structured Extraction ──────────────────────────

/// Stage 2 output: structured question extraction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredExtractionResult {
    /// Clean question in markdown
    pub question_markdown_clean: String,
    /// Structured question breakdown
    pub question_structure: QuestionStructure,
    /// Image regions inherited from vision
    #[serde(default)]
    pub image_regions: Vec<Vec<f64>>,
    /// Difficulty level
    pub difficulty: String,
    /// Estimated grade
    pub estimated_grade: Option<String>,
}

/// Question structure breakdown
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionStructure {
    /// 选择题|填空题|判断题|计算题|应用题|阅读理解|写作题|其他
    pub question_type: String,
    /// Whether the question has sub-questions
    #[serde(default)]
    pub has_sub_questions: bool,
    /// Sub-questions (if any)
    #[serde(default)]
    pub sub_questions: Vec<SubQuestion>,
}

/// A sub-question
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubQuestion {
    pub index: u32,
    #[serde(rename = "type")]
    pub q_type: String,
    pub stem: String,
    #[serde(default)]
    pub options: Vec<String>,
}

// ── Stage 3: Pedagogical Analysis ───────────────────────────

/// Stage 3 output: pedagogical analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PedagogicalAnalysisResult {
    pub subject: String,
    pub classification: Vec<String>,
    pub error_type: String,
    pub error_subtype: String,
    pub error_reason: String,
    pub suggestions: String,
    #[serde(default)]
    pub root_cause_code: String,
    #[serde(default)]
    pub confidence: serde_json::Value,
}

// ═══════════════════════════════════════════════════════════════
// Multi-stage parse helpers
// ═══════════════════════════════════════════════════════════════

/// Parse Stage 1 vision recognition raw LLM output into VisionRecognitionResult.
pub fn parse_vision_recognition(raw: &str) -> Result<VisionRecognitionResult> {
    let json_str = extract_json_from_llm_output(raw)?;
    let result: VisionRecognitionResult = serde_json::from_str(&json_str).with_context(|| {
        format!(
            "Vision recognition JSON 解析失败，原始内容: {}",
            truncate_for_error(&json_str, 500)
        )
    })?;
    Ok(result)
}

/// Parse Stage 2 structured extraction raw LLM output into StructuredExtractionResult.
pub fn parse_structured_extraction(raw: &str) -> Result<StructuredExtractionResult> {
    let json_str = extract_json_from_llm_output(raw)?;
    let result: StructuredExtractionResult =
        serde_json::from_str(&json_str).with_context(|| {
            format!(
                "Structured extraction JSON 解析失败，原始内容: {}",
                truncate_for_error(&json_str, 500)
            )
        })?;
    Ok(result)
}

/// Parse Stage 3 pedagogical analysis raw LLM output into PedagogicalAnalysisResult.
pub fn parse_pedagogical_analysis(raw: &str) -> Result<PedagogicalAnalysisResult> {
    let json_str = extract_json_from_llm_output(raw)?;
    let result: PedagogicalAnalysisResult = serde_json::from_str(&json_str).with_context(|| {
        format!(
            "Pedagogical analysis JSON 解析失败，原始内容: {}",
            truncate_for_error(&json_str, 500)
        )
    })?;
    Ok(result)
}

/// Extract JSON from LLM output (handles ```json blocks and bare JSON)
fn extract_json_from_llm_output(raw: &str) -> Result<String> {
    // Try ```json ... ``` block first
    if let Some(json) = extract_json_block(raw) {
        return Ok(json);
    }
    // Try bare JSON object
    let trimmed = raw.trim();
    if trimmed.starts_with('{') {
        if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
            return Ok(trimmed.to_string());
        }
    }
    // Try finding first { to last }
    if let Some(start) = raw.find('{') {
        if let Some(end) = raw.rfind('}') {
            if end > start {
                let candidate = &raw[start..=end];
                if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                    return Ok(candidate.to_string());
                }
            }
        }
    }
    Err(anyhow::anyhow!(
        "无法从 LLM 输出中提取 JSON。原始内容前500字符: {}",
        truncate_for_error(raw, 500)
    ))
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
    #[serde(default)]
    pub question_form: Option<String>,
    #[serde(default)]
    pub material_type: Option<String>,
    #[serde(default)]
    pub requires_image: bool,
    #[serde(default)]
    pub image_role: Option<PracticeImageRole>,
    #[serde(default)]
    pub image_type: Option<PracticeImageType>,
    #[serde(default)]
    pub dependency_mode: Option<PracticeDependencyMode>,
    #[serde(default)]
    pub image_spec: Option<PracticeImageSpec>,
    #[serde(default)]
    pub image_path: Option<String>,
}

pub fn parse_practice_plan_response(raw: &str) -> Result<PracticePlan> {
    let json_str = match extract_json_block(raw) {
        Some(json) => json,
        None => extract_bare_json(raw),
    };

    match serde_json::from_str::<PracticePlan>(&json_str) {
        Ok(plan) => Ok(plan),
        Err(primary_err) => {
            if let Ok(items) = serde_json::from_str::<
                Vec<crate::practice::planner::PracticeQuestionPlanItem>,
            >(&json_str)
            {
                return Ok(PracticePlan {
                    subject: "未指定".to_string(),
                    grade_level: "未指定".to_string(),
                    count: items.len() as u32,
                    rationale: "root_array_fallback".to_string(),
                    items,
                });
            }

            let diagnosis = diagnose_practice_plan_json(&json_str, &primary_err);
            Err(anyhow::anyhow!(
                "练习题规划 JSON 解析失败。{}。原始内容: {}",
                diagnosis,
                truncate_for_error(&json_str, 500)
            ))
        }
    }
}

fn diagnose_practice_plan_json(json_str: &str, err: &serde_json::Error) -> String {
    let mut hints = Vec::new();
    let value = serde_json::from_str::<serde_json::Value>(json_str);

    match value {
        Ok(serde_json::Value::Array(_)) => {
            hints.push("顶层是 root_array，非预期 object_wrapper".to_string());
        }
        Ok(serde_json::Value::Object(map)) => {
            if !map.contains_key("items") {
                hints.push("缺少 items 字段".to_string());
            }
            if let Some(items) = map.get("items").and_then(|v| v.as_array()) {
                if let Some((idx, item)) = items.iter().enumerate().find(|(_, item)| {
                    item.get("image_spec").is_some_and(|v| v.is_string())
                        || item
                            .get("dependency_mode")
                            .and_then(|v| v.as_str())
                            .is_some_and(|s| s == "essential")
                        || item
                            .get("image_role")
                            .and_then(|v| v.as_str())
                            .is_some_and(|s| s.contains('-'))
                }) {
                    if item.get("image_spec").is_some_and(|v| v.is_string()) {
                        hints.push(format!(
                            "items[{}].image_spec 是 string，已支持兼容；若仍失败请检查其它字段",
                            idx
                        ));
                    }
                    if let Some(mode) = item.get("dependency_mode").and_then(|v| v.as_str()) {
                        hints.push(format!(
                            "items[{}].dependency_mode={}（可能是不兼容枚举值）",
                            idx, mode
                        ));
                    }
                    if let Some(role) = item.get("image_role").and_then(|v| v.as_str()) {
                        hints.push(format!(
                            "items[{}].image_role={}（已尝试兼容连字符）",
                            idx, role
                        ));
                    }
                    if let Some(image_type) = item.get("image_type").and_then(|v| v.as_str()) {
                        hints.push(format!("items[{}].image_type={}", idx, image_type));
                    }
                }
            }
        }
        Ok(other) => hints.push(format!("顶层 JSON 类型异常: {}", other)),
        Err(_) => {
            hints.push(format!(
                "serde 错误位置 line={} column={} reason={}",
                err.line(),
                err.column(),
                err
            ));
            hints.push(format!(
                "尾部上下文={}",
                truncate_for_error(&tail_chars(json_str, 160), 180)
            ));
            return hints.join("；");
        }
    }

    hints.push(format!(
        "serde 错误位置 line={} column={} reason={}",
        err.line(),
        err.column(),
        err
    ));
    hints.join("；")
}

/// 解析 LLM 返回的错题分析响应（结构化版本）
/// 返回拆分后的 `ParsedLegacyAnalysis`，包含 `QuestionExtraction` 和 `PedagogicalAnalysis`
pub fn parse_analysis_response_structured(raw: &str) -> Result<ParsedLegacyAnalysis> {
    // 尝试提取 JSON 代码块
    let (markdown_part, json_part) = extract_markdown_and_json(raw)?;

    // 解析 JSON
    let analysis_json: AnalysisJson = serde_json::from_str(&json_part).with_context(|| {
        format!(
            "JSON 解析失败，原始内容: {}",
            truncate_for_error(&json_part, 500)
        )
    })?;

    // 尝试从 markdown 中提取 image_regions
    let image_regions = extract_image_regions(&markdown_part);

    Ok(ParsedLegacyAnalysis {
        question_extraction: QuestionExtraction {
            question_markdown: markdown_part.trim().to_string(),
            image_regions,
        },
        pedagogical_analysis: PedagogicalAnalysis {
            subject: analysis_json.subject,
            classification: analysis_json.classification,
            error_reason: analysis_json.reason,
            suggestions: analysis_json.suggestions,
        },
    })
}

/// 解析 LLM 返回的错题分析响应（兼容旧接口）
/// 预期格式：先 markdown 原题，后跟 JSON 代码块
pub fn parse_analysis_response(raw: &str) -> Result<(String, AnalysisResult)> {
    let parsed = parse_analysis_response_structured(raw)?;

    let result = AnalysisResult {
        subject: parsed.pedagogical_analysis.subject,
        classification: parsed.pedagogical_analysis.classification,
        original_question: parsed.question_extraction.question_markdown,
        image_regions: parsed.question_extraction.image_regions,
        error_reason: parsed.pedagogical_analysis.error_reason,
        suggestions: parsed.pedagogical_analysis.suggestions,
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
        "无法从 LLM 查应中提取 JSON 内容。原始响应前500字符: {}",
        truncate_for_error(raw, 500)
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
            truncate_for_error(&json_str, 500)
        )
    })?;

    Ok(summary)
}

/// 对象包装格式的练习题响应
/// 形如 `{ "questions": [ {"question":"...","answer":"...","knowledge_points":[...]} ] }`
#[derive(Debug, Deserialize)]
struct PracticeResponseWrapper {
    questions: Vec<PracticeQuestion>,
}

/// 解析 LLM 返回的练习题响应
/// 支持两种格式：
/// - 新格式（对象包装）：`{ "questions": [ ... ] }`
/// - 旧格式（根数组）：`[ ... ]`
/// 支持截断 JSON 的自动修复
pub fn parse_practice_response(raw: &str) -> Result<Vec<PracticeQuestion>> {
    let json_str = match extract_json_block(raw) {
        Some(json) => json,
        None => extract_bare_json(raw),
    };

    // 优先尝试新格式：对象包装 { "questions": [...] }
    if let Ok(wrapper) = serde_json::from_str::<PracticeResponseWrapper>(&json_str) {
        return Ok(wrapper.questions);
    }

    // 兼容旧格式：根数组 [...]
    if let Ok(questions) = serde_json::from_str::<Vec<PracticeQuestion>>(&json_str) {
        return Ok(questions);
    }

    // 直接解析失败，尝试修复截断的 JSON
    // 先尝试从对象包装中提取 questions 数组进行修复
    if let Some(questions_str) = extract_questions_array_from_wrapper(&json_str) {
        let repaired = repair_truncated_json_array(&questions_str);
        if let Ok(questions) = serde_json::from_str::<Vec<PracticeQuestion>>(&repaired) {
            tracing::warn!(
                "练习题 JSON（对象包装内数组）曾被截断，已自动修复（原始 {} 字节 → 修复后 {} 字节）",
                json_str.len(),
                repaired.len()
            );
            return Ok(questions);
        }
    }

    // 回退：对整体做截断修复（兼容旧根数组格式）
    let repaired = repair_truncated_json_array(&json_str);
    // 修复后的内容可能是对象包装
    if let Ok(wrapper) = serde_json::from_str::<PracticeResponseWrapper>(&repaired) {
        tracing::warn!(
            "练习题 JSON（对象包装）曾被截断，已自动修复（原始 {} 字节 → 修复后 {} 字节）",
            json_str.len(),
            repaired.len()
        );
        return Ok(wrapper.questions);
    }
    if let Ok(questions) = serde_json::from_str::<Vec<PracticeQuestion>>(&repaired) {
        tracing::warn!(
            "练习题 JSON 曾被截断，已自动修复（原始 {} 字节 → 修复后 {} 字节）",
            json_str.len(),
            repaired.len()
        );
        return Ok(questions);
    }

    Err(anyhow::anyhow!("{}", format_diagnosis(&json_str)))
}

/// 尝试从对象包装的 JSON 字符串中提取 `questions` 数组的内容（不含外层 `[]`）。
/// 仅在 json_str 以 `{` 开头且包含 `"questions"` 键时生效。
fn extract_questions_array_from_wrapper(json_str: &str) -> Option<String> {
    let trimmed = json_str.trim();
    if !trimmed.starts_with('{') {
        return None;
    }

    // Fast path: full JSON is valid, direct extraction.
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(questions_val) = val.get("questions") {
            if questions_val.is_array() {
                return Some(questions_val.to_string());
            }
        }
    }

    // Salvage path: wrapper may be truncated. Try to scan out `questions: [ ... ]`
    // even when outer object is incomplete.
    extract_questions_array_from_wrapper_scanning(trimmed)
}

/// 扫描式提取对象包装中的 `questions` 数组。
/// 即使外层对象未闭合，也尽量返回从 `[` 开始的数组片段（可能不完整）。
fn extract_questions_array_from_wrapper_scanning(trimmed: &str) -> Option<String> {
    let key_pos = trimmed.find("\"questions\"")?;
    let after_key = &trimmed[key_pos + "\"questions\"".len()..];

    // Find ':' after key
    let colon_rel = after_key.find(':')?;
    let after_colon = &after_key[colon_rel + 1..];

    // Find the first '[' after ':'
    let arr_start_rel = after_colon.find('[')?;
    let arr_start_abs = key_pos + "\"questions\"".len() + colon_rel + 1 + arr_start_rel;
    let arr_slice = &trimmed[arr_start_abs..];

    // Try to find matching ']' with string/escape awareness.
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in arr_slice.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }

        if in_string {
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        // include closing ']'
                        return Some(arr_slice[..=idx].to_string());
                    }
                }
            }
            _ => {}
        }
    }

    // Array not closed; return trailing fragment for repair_truncated_json_array.
    Some(arr_slice.to_string())
}

// ═══════════════════════════════════════════════════════════════
// Practice-specific JSON diagnostics
// ═══════════════════════════════════════════════════════════════

/// Diagnostic labels produced by `diagnose_practice_json_error`.
#[derive(Debug, Clone, Default)]
struct PracticeJsonDiagnosis {
    /// "object_wrapper" | "root_array" | "unknown"
    format_guess: &'static str,
    /// Diagnostic tags, e.g. "truncated_in_string", "trailing_backslash", etc.
    tags: Vec<&'static str>,
    /// Number of complete question objects that could be salvaged.
    complete_questions: usize,
    /// serde_json line/column info (if available).
    serde_location: Option<String>,
    /// UTF-8 safe context snippet near the end of the input.
    tail_context: String,
}

/// Build a detailed diagnostic message for practice JSON parse failure.
fn diagnose_practice_json_error(json_str: &str) -> PracticeJsonDiagnosis {
    let trimmed = json_str.trim();
    let mut d = PracticeJsonDiagnosis::default();

    // 1) Format guess
    if trimmed.starts_with('{') {
        d.format_guess = "object_wrapper";
    } else if trimmed.starts_with('[') {
        d.format_guess = "root_array";
    } else {
        d.format_guess = "unknown";
    }

    // 2) Serde error location — try parsing to get line/column
    let serde_err = if d.format_guess == "object_wrapper" {
        serde_json::from_str::<serde_json::Value>(trimmed).err()
    } else {
        serde_json::from_str::<Vec<serde_json::Value>>(trimmed).err()
    };
    if let Some(err) = &serde_err {
        d.serde_location = Some(format!("{}", err));
    }

    // 3) Structural analysis — scan for open brackets/strings
    let mut depth_brace = 0i32; // { }
    let mut depth_bracket = 0i32; // [ ]
    let mut in_string = false;
    let mut escaped = false;
    let mut trailing_backslash = false;
    // Count all complete { } pairs regardless of nesting depth.
    let mut complete_brace_pairs = 0usize;

    for ch in trimmed.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if in_string {
            match ch {
                '\\' => {
                    escaped = true;
                    trailing_backslash = true;
                }
                '"' => {
                    in_string = false;
                    trailing_backslash = false;
                }
                _ => {
                    trailing_backslash = false;
                }
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                trailing_backslash = false;
            }
            '{' => {
                trailing_backslash = false;
                depth_brace += 1;
            }
            '}' => {
                trailing_backslash = false;
                if depth_brace > 0 {
                    depth_brace -= 1;
                    complete_brace_pairs += 1;
                }
            }
            '[' => {
                depth_bracket += 1;
                trailing_backslash = false;
            }
            ']' => {
                depth_bracket -= 1;
                trailing_backslash = false;
            }
            _ => {
                trailing_backslash = false;
            }
        }
    }

    // For object_wrapper format, if the outer {} pair completed (depth_brace==0),
    // one of the brace pairs is the wrapper itself — subtract it.
    d.complete_questions = if d.format_guess == "object_wrapper" && depth_brace == 0 {
        complete_brace_pairs.saturating_sub(1)
    } else {
        complete_brace_pairs
    };

    // 4) Tags
    if in_string {
        d.tags.push("truncated_in_string");
    }
    if trailing_backslash {
        d.tags.push("trailing_backslash");
    }
    if depth_brace > 0 {
        d.tags.push("unclosed_object");
    }
    if depth_bracket > 0 {
        d.tags.push("unclosed_array");
    }

    // Check for object-wrapper with partial questions array
    if d.format_guess == "object_wrapper" {
        // Try to see if "questions" key exists in the raw text
        if trimmed.contains("\"questions\"") {
            if depth_brace == 0 && depth_bracket > 0 {
                d.tags.push("wrapper_ok_array_incomplete");
            } else if depth_brace > 0 {
                d.tags.push("wrapper_incomplete");
            }
        }
    }

    // Check if there are zero complete objects (likely total truncation)
    if d.complete_questions == 0 && trimmed.len() > 10 {
        d.tags.push("no_complete_objects");
    }

    // 5) Tail context — last 120 chars, UTF-8 safe
    d.tail_context = tail_chars(trimmed, 120);

    d
}

/// Return the last `max_chars` Unicode characters of `s`, UTF-8 safe.
fn tail_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let skip = s.chars().count() - max_chars;
        format!("…{}", s.chars().skip(skip).collect::<String>())
    }
}

/// Format the diagnosis into a human-readable error message.
fn format_diagnosis(json_str: &str) -> String {
    let d = diagnose_practice_json_error(json_str);
    let len_bytes = json_str.len();
    let len_chars = json_str.chars().count();

    let mut parts = Vec::new();
    parts.push(format!("格式={}", d.format_guess));
    parts.push(format!("长度={}字节/{}字符", len_bytes, len_chars));
    parts.push(format!("已完成题目数={}", d.complete_questions));

    if !d.tags.is_empty() {
        parts.push(format!("诊断=[{}]", d.tags.join(", ")));
    }

    if let Some(ref loc) = d.serde_location {
        // Extract just the key info, keep it short
        let loc_short = if loc.len() > 200 {
            truncate_for_error(loc, 200)
        } else {
            loc.clone()
        };
        parts.push(format!("serde错误={}", loc_short));
    }

    parts.push(format!("尾部上下文=\"{}\"", d.tail_context));

    format!(
        "练习题 JSON 解析失败（含修复尝试）。{}。原始内容前500字符: {}",
        parts.join("；"),
        truncate_for_error(json_str, 500)
    )
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

    // ── truncate_for_error UTF-8 safety ──────────────────────────

    #[test]
    fn truncate_for_error_short_ascii() {
        assert_eq!(truncate_for_error("hello", 10), "hello");
    }

    #[test]
    fn truncate_for_error_truncates_ascii() {
        assert_eq!(truncate_for_error("hello world", 5), "hello…");
    }

    #[test]
    fn truncate_for_error_chinese_no_panic() {
        // 3-byte UTF-8 characters — byte-based slicing would panic
        let chinese = "这是一段中文内容用来测试截断";
        let result = truncate_for_error(chinese, 5);
        assert_eq!(result, "这是一段中…");
    }

    #[test]
    fn truncate_for_error_mixed_chars() {
        let mixed = "你好abc世界def";
        let result = truncate_for_error(mixed, 5);
        assert_eq!(result, "你好abc…");
    }

    #[test]
    fn truncate_for_error_exact_length() {
        let s = "测试";
        assert_eq!(truncate_for_error(s, 2), "测试");
    }

    #[test]
    fn truncate_for_error_empty() {
        assert_eq!(truncate_for_error("", 5), "");
    }

    // ── parse_practice_response: object wrapper ──────────────────

    #[test]
    fn parse_practice_object_wrapper() {
        let json =
            r#"{"questions":[{"question":"1+1=?","answer":"2","knowledge_points":["加法"]}]}"#;
        let questions = parse_practice_response(json).unwrap();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].question, "1+1=?");
        assert_eq!(questions[0].answer, "2");
        assert_eq!(questions[0].knowledge_points, vec!["加法"]);
    }

    #[test]
    fn parse_practice_object_wrapper_with_code_block() {
        let raw = "```json\n{\"questions\":[{\"question\":\"Q1\",\"answer\":\"A1\",\"knowledge_points\":[\"k1\"]}]}\n```";
        let questions = parse_practice_response(raw).unwrap();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].question, "Q1");
    }

    #[test]
    fn parse_practice_object_wrapper_multiple() {
        let json = r#"{"questions":[
            {"question":"Q1","answer":"A1","knowledge_points":["k1"]},
            {"question":"Q2","answer":"A2","knowledge_points":["k2"]},
            {"question":"Q3","answer":"A3","knowledge_points":["k3"]}
        ]}"#;
        let questions = parse_practice_response(json).unwrap();
        assert_eq!(questions.len(), 3);
    }

    // ── parse_practice_response: backward compat root array ──────

    #[test]
    fn parse_practice_root_array_compat() {
        let json = r#"[{"question":"Q1","answer":"A1","knowledge_points":["k1"]}]"#;
        let questions = parse_practice_response(json).unwrap();
        assert_eq!(questions.len(), 1);
    }

    #[test]
    fn parse_practice_root_array_with_code_block() {
        let raw =
            "```json\n[{\"question\":\"Q\",\"answer\":\"A\",\"knowledge_points\":[\"k\"]}]\n```";
        let questions = parse_practice_response(raw).unwrap();
        assert_eq!(questions.len(), 1);
    }

    // ── Chinese content in parse error does not panic ────────────

    #[test]
    fn parse_vision_recognition_chinese_error_no_panic() {
        let bad_json = "这是一段很长的中文内容，完全不是JSON格式，所以应该返回解析错误而不是panic，字节截断可能在多字节UTF8字符处出问题";
        let result = parse_vision_recognition(bad_json);
        assert!(result.is_err());
        // Verify the error message can be displayed without panic
        let _ = format!("{:?}", result);
    }

    #[test]
    fn parse_practice_response_chinese_error_no_panic() {
        // Garbage Chinese content — should error, not panic
        let bad = "这不是JSON内容，测试一下截断日志中包含中文时会不会因为字节截断而panic。多一些字符确保触发截断。";
        let result = parse_practice_response(bad);
        assert!(result.is_err());
        let _ = format!("{:?}", result);
    }

    #[test]
    fn parse_summary_response_chinese_error_no_panic() {
        let bad = "中文内容不是JSON，截断测试。更多的中文字符来确保超过500字符截断阈值。";
        let result = parse_summary_response(bad);
        assert!(result.is_err());
        let _ = format!("{:?}", result);
    }

    // ── extract_questions_array_from_wrapper helper ──────────────

    #[test]
    fn extract_questions_array_from_valid_wrapper() {
        let json = r#"{"questions":[{"question":"Q","answer":"A","knowledge_points":["k"]}]}"#;
        let extracted = extract_questions_array_from_wrapper(json).unwrap();
        let parsed: Vec<PracticeQuestion> = serde_json::from_str(&extracted).unwrap();
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn extract_questions_array_from_non_wrapper() {
        let json = r#"[{"question":"Q","answer":"A","knowledge_points":["k"]}]"#;
        assert!(extract_questions_array_from_wrapper(json).is_none());
    }

    #[test]
    fn extract_questions_array_from_invalid_json() {
        assert!(extract_questions_array_from_wrapper("not json").is_none());
    }

    #[test]
    fn extract_questions_array_from_incomplete_wrapper_salvage() {
        let json = r#"{"questions":[{"question":"Q1","answer":"A1","knowledge_points":["k1"]},{"question":"Q2 long"#;
        let extracted = extract_questions_array_from_wrapper(json)
            .expect("should salvage questions array fragment");
        let repaired = repair_truncated_json_array(&extracted);
        let parsed: Vec<PracticeQuestion> = serde_json::from_str(&repaired).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].question, "Q1");
    }

    #[test]
    fn parse_practice_incomplete_wrapper_salvages_complete_first_question() {
        let raw = r#"{"questions":[{"question":"长题干1","answer":"A1","knowledge_points":["k1"]},{"question":"长题干2被截断"#;
        let parsed = parse_practice_response(raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].question, "长题干1");
    }

    // ═══════════════════════════════════════════════════════════
    // diagnose_practice_json_error tests
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn diagnose_object_wrapper_truncated_in_string() {
        // Object wrapper, truncated mid-string in question text
        let json = r#"{"questions":[{"question":"一道很长的题目内容被截断到这里就"#;
        let d = diagnose_practice_json_error(json);
        assert_eq!(d.format_guess, "object_wrapper");
        assert!(d.tags.contains(&"truncated_in_string"));
        assert!(d.tags.contains(&"unclosed_object"));
        assert!(d.tags.contains(&"unclosed_array"));
        assert_eq!(d.complete_questions, 0);
        // Verify the error message contains the tag
        let msg = format_diagnosis(json);
        assert!(msg.contains("truncated_in_string"), "msg: {}", msg);
        assert!(msg.contains("object_wrapper"), "msg: {}", msg);
    }

    #[test]
    fn diagnose_root_array_trailing_backslash() {
        // Root array, ends with trailing backslash inside a string
        let json = r#"[{"question":"trail\"#;
        let d = diagnose_practice_json_error(json);
        assert_eq!(d.format_guess, "root_array");
        assert!(d.tags.contains(&"trailing_backslash"), "tags: {:?}", d.tags);
        assert!(
            d.tags.contains(&"truncated_in_string"),
            "tags: {:?}",
            d.tags
        );
        let msg = format_diagnosis(json);
        assert!(msg.contains("trailing_backslash"), "msg: {}", msg);
    }

    #[test]
    fn diagnose_wrapper_with_complete_and_partial() {
        // Wrapper with one complete question and one truncated
        let json = r#"{"questions":[{"question":"Q1","answer":"A1","knowledge_points":["k1"]},{"question":"Q2 is long an"#;
        let d = diagnose_practice_json_error(json);
        assert_eq!(d.format_guess, "object_wrapper");
        // Q1 is complete (its {} pair fully closed). Outer wrapper {} never closed,
        // so no subtraction needed: 1 brace pair = 1 complete question.
        assert_eq!(d.complete_questions, 1);
        assert!(d.tags.contains(&"truncated_in_string"));
        assert!(d.tags.contains(&"wrapper_incomplete"));
        let msg = format_diagnosis(json);
        assert!(msg.contains("已完成题目数=1"), "msg: {}", msg);
    }

    #[test]
    fn diagnose_wrapper_ok_array_incomplete() {
        // Wrapper where outer object closes but inner array doesn't
        // This is a bit contrived; in practice the repair path may handle it.
        // Let's test a case where "questions" exists but the value is truncated.
        let json = r#"{"questions": [{"question":"Q1","answer":"A1","knowledge_points":["k1"]},{"question":"Q2"#;
        let d = diagnose_practice_json_error(json);
        assert_eq!(d.format_guess, "object_wrapper");
        // The outer { is not closed
        assert!(d.tags.contains(&"unclosed_object") || d.tags.contains(&"truncated_in_string"));
    }

    #[test]
    fn diagnose_root_array_with_complete_objects() {
        // Root array: Q1 and Q2 are both complete (their {} close), but root ] never closes.
        let json = r#"[{"question":"Q1","answer":"A1","knowledge_points":["k1"]},{"question":"Q2","answer":"A2","knowledge_points":["k2"]}"#;
        let d = diagnose_practice_json_error(json);
        assert_eq!(d.format_guess, "root_array");
        assert_eq!(d.complete_questions, 2);
        assert!(d.tags.contains(&"unclosed_array"));
        let msg = format_diagnosis(json);
        assert!(msg.contains("已完成题目数=2"), "msg: {}", msg);
    }

    #[test]
    fn diagnose_tail_context_shows_end() {
        let long = format!(
            "{{\"questions\":[{}]}}",
            r#"{"question":"x","answer":"y","knowledge_points":["z"]},"#.repeat(50)
        );
        let json = &long[..long.len() - 20]; // truncate
        let d = diagnose_practice_json_error(json);
        // tail_context should show the end
        assert!(d.tail_context.len() <= 130); // 120 chars + "…"
    }

    #[test]
    fn diagnose_serde_location_present() {
        // Invalid JSON that serde can report on
        let json = r#"{"questions": [BAD]}"#;
        let d = diagnose_practice_json_error(json);
        assert!(d.serde_location.is_some(), "should have serde error info");
        let msg = format_diagnosis(json);
        assert!(msg.contains("serde错误"), "msg: {}", msg);
    }

    #[test]
    fn parse_practice_error_contains_diagnostic_tags() {
        // Ensure failure path still exposes diagnostic tags when nothing salvageable exists
        let bad = r#"{"questions":[{"question":"截断"#;
        let result = parse_practice_response(bad);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("truncated_in_string"),
            "err_msg: {}",
            err_msg
        );
        assert!(err_msg.contains("object_wrapper"), "err_msg: {}", err_msg);
    }

    #[test]
    fn parse_practice_salvages_complete_question_from_incomplete_wrapper() {
        let bad = r#"{
          "questions": [
            {
              "question": "Q1 long",
              "answer": "A1",
              "knowledge_points": ["k1"]
            },
            {
              "question": "Q2 gets trunc"#;

        let questions = parse_practice_response(bad).unwrap();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].question, "Q1 long");
        assert_eq!(questions[0].answer, "A1");
    }

    #[test]
    fn parse_practice_plan_accepts_alias_enums_and_string_image_spec() {
        let raw = r#"{
          "subject": "数学",
          "grade_level": "二年级",
          "count": 1,
          "rationale": "test",
          "items": [
            {
              "index": 1,
              "primary_point": "万以内数的认识与读写",
              "secondary_points": [],
              "question_form": "multiple_choice",
              "material_type": "text_with_image",
              "target_reference_ids": ["R1"],
              "avoid_topics": [],
              "requires_image": true,
              "image_role": "source-material",
              "image_type": "diagram",
              "dependency_mode": "essential",
              "image_spec": "一个算盘图"
            }
          ]
        }"#;

        let plan = parse_practice_plan_response(raw).unwrap();
        assert_eq!(plan.items.len(), 1);
        assert!(plan.items[0].requires_image);
        assert_eq!(
            plan.items[0].image_spec.as_ref().unwrap().prompt,
            "一个算盘图"
        );
    }

    #[test]
    fn parse_practice_plan_accepts_math_diagram_and_standalone() {
        let raw = r#"{
          "subject": "数学",
          "grade_level": "二年级",
          "count": 1,
          "items": [
            {
              "index": 1,
              "primary_point": "万以内数的认识与读写",
              "secondary_points": [],
              "question_form": "选择题",
              "material_type": "图文结合",
              "target_reference_ids": ["R4"],
              "avoid_topics": [],
              "requires_image": true,
              "image_role": "source-material",
              "image_type": "math_diagram",
              "dependency_mode": "standalone",
              "image_spec": "一个算盘图"
            }
          ]
        }"#;

        let plan = parse_practice_plan_response(raw).unwrap();
        assert_eq!(
            plan.items[0].image_type,
            crate::practice::planner::PracticeImageType::GeometryDiagram
        );
        assert_eq!(
            plan.items[0].dependency_mode,
            crate::practice::planner::PracticeDependencyMode::TextOnly
        );
    }

    #[test]
    fn parse_practice_plan_error_reports_field_reason() {
        let raw = r#"{
          "subject": "数学",
          "grade_level": "二年级",
          "count": 1,
          "items": [
            {
              "index": 1,
              "primary_point": "时间",
              "secondary_points": [],
              "question_form": "word_problem",
              "material_type": "text",
              "target_reference_ids": [],
              "avoid_topics": [],
              "requires_image": true,
              "image_role": "weird-role",
              "image_type": "mystery-diagram",
              "dependency_mode": "ultra-mode"
            }
          ]
        }"#;

        let err = format!("{}", parse_practice_plan_response(raw).unwrap_err());
        assert!(
            err.contains("dependency_mode")
                || err.contains("image_role")
                || err.contains("image_type"),
            "err={}",
            err
        );
        assert!(
            err.contains("line=") || err.contains("reason="),
            "err={}",
            err
        );
    }

    #[test]
    fn tail_chars_basic() {
        assert_eq!(tail_chars("hello", 3), "…llo");
        assert_eq!(tail_chars("hi", 5), "hi");
        assert_eq!(tail_chars("你好世界", 2), "…世界");
    }
}
