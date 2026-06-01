use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

/// 当前最新 data_version 常量
pub const DATA_VERSION_CURRENT: i64 = 2;

/// 错题记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorRecord {
    pub id: String,
    pub image_path: String,
    pub subject: String,
    pub grade_level: String,
    pub original_question: String,
    /// 配图坐标，JSON 数组 [[x1, y1, x2, y2], ...]
    pub image_regions: Option<String>,
    /// 知识点标签，JSON 数组 ["知识点1", "知识点2"]
    pub classification: String,
    pub error_reason: String,
    pub suggestions: String,
    /// 文本 embedding 向量（科目+知识点+原题+原因+建议）
    #[serde(skip)]
    pub text_embedding: Vec<f32>,
    /// 图片 embedding 向量（原图）
    #[serde(skip)]
    pub image_embedding: Vec<f32>,
    /// Unix timestamp
    pub created_at: i64,

    // ── 结构化预留字段（Phase 1: 全部 Option，兼容旧数据）──
    /// 清洗后的题目 Markdown（去除批注/涂改等噪声）
    pub question_markdown_clean: Option<String>,
    /// 题目结构化 JSON（题型、子题拆分等）
    pub question_structure_json: Option<String>,
    /// 学生作答文本（从图片中识别）
    pub student_answer_text: Option<String>,
    /// 老师批注 JSON（红笔标注、对号/叉号等）
    pub teacher_marks_json: Option<String>,
    /// 题目分类（选择题/填空题/应用题/计算题 等）
    pub question_type: Option<String>,
    /// 难度等级
    pub difficulty: Option<String>,
    /// 错误大类（概念错误/计算错误/审题错误/表达错误 等）
    pub error_type: Option<String>,
    /// 错误子类
    pub error_subtype: Option<String>,
    /// 根因编码（可枚举的错误根因标识）
    pub root_cause_code: Option<String>,
    /// 置信度 JSON（各字段的模型置信度）
    pub confidence_json: Option<String>,
    /// 分析管线版本
    pub pipeline_version: Option<String>,
    /// 模型调用追踪 JSON（记录每步使用的模型/参数/耗时）
    pub model_trace_json: Option<String>,

    /// 数据版本号（用于 backfill 升级追踪）
    pub data_version: i64,
}

impl Default for ErrorRecord {
    fn default() -> Self {
        Self {
            id: String::new(),
            image_path: String::new(),
            subject: String::new(),
            grade_level: String::new(),
            original_question: String::new(),
            image_regions: None,
            classification: String::new(),
            error_reason: String::new(),
            suggestions: String::new(),
            text_embedding: Vec::new(),
            image_embedding: Vec::new(),
            created_at: 0,
            question_markdown_clean: None,
            question_structure_json: None,
            student_answer_text: None,
            teacher_marks_json: None,
            question_type: None,
            difficulty: None,
            error_type: None,
            error_subtype: None,
            root_cause_code: None,
            confidence_json: None,
            pipeline_version: None,
            model_trace_json: None,
            data_version: DATA_VERSION_CURRENT,
        }
    }
}

/// 用于搜索结果的轻量记录（含距离分数）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorRecordWithScore {
    pub record: ErrorRecord,
    /// 余弦距离（0=完全相同，2=完全相反）
    pub distance: f64,
}

impl ErrorRecordWithScore {
    /// 余弦相似度（1=完全相同，-1=完全相反）
    pub fn similarity(&self) -> f64 {
        1.0 - self.distance
    }
}

/// 分类标签（子表）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationTag {
    pub error_id: String,
    pub tag: String,
}

/// 阶段性总结
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub subject: String,
    /// week | month | semester
    pub period_type: String,
    pub period_start: i64,
    pub period_end: i64,
    pub common_reasons: String,
    pub common_suggestions: String,
    /// JSON 数组 ["知识点1", ...]
    pub weak_points: String,
    pub detail: String,
    /// JSON 数组 ["id1", "id2", ...]
    pub related_error_ids: String,
    pub created_at: i64,
    /// 数据版本号
    pub data_version: i64,
    /// 生成器版本标识
    pub generator_version: Option<String>,
}

/// 总结信息图
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryImage {
    pub id: String,
    pub summary_id: String,
    pub prompt: String,
    pub image_path: String,
    pub mime_type: String,
    pub created_at: i64,
    /// 数据版本号
    pub data_version: i64,
    /// 生成器版本标识
    pub generator_version: Option<String>,
}

/// 巩固练习
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PracticeSet {
    pub id: String,
    pub summary_id: String,
    pub subject: String,
    pub requirements: Option<String>,
    /// JSON 数组 [{question, answer, ...}]
    pub questions: String,
    pub pdf_path: Option<String>,
    pub created_at: i64,
    /// 数据版本号
    pub data_version: i64,
    /// 生成器版本标识
    pub generator_version: Option<String>,
}

/// MCP 后台任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpJob {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub input_json: String,
    pub result_json: Option<String>,
    pub error_message: Option<String>,
    pub progress_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

/// 分析产物（结构化管线各阶段的中间/最终输出）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisArtifact {
    pub id: String,
    pub error_id: String,
    /// 管线阶段名称（如 "ocr", "structure_extraction", "pedagogical_analysis"）
    pub stage: String,
    /// 该阶段产物的 schema 版本
    pub schema_version: String,
    /// 使用的模型名称
    pub model_name: Option<String>,
    /// 该阶段的 JSON 产物
    pub payload_json: String,
    /// Unix timestamp
    pub created_at: i64,
}

/// ============ 业务请求/响应模型 ============

/// 错题分析请求
#[derive(Debug, Clone)]
pub struct AnalysisRequest {
    pub image_path: String,
    pub subject: Option<String>,
    pub grade_level: Option<String>,
    pub color_teacher: Option<String>,
    pub color_correction: Option<String>,
}

/// 错题分析结果（LLM 返回解析后）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub subject: String,
    pub classification: Vec<String>,
    pub original_question: String,
    pub image_regions: Vec<Vec<f64>>,
    pub error_reason: String,
    pub suggestions: String,
}

/// 总结请求
#[derive(Debug, Clone)]
pub struct SummaryRequest {
    pub subject: String,
    pub from_date: NaiveDateTime,
    pub to_date: NaiveDateTime,
    pub period_type: PeriodType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PeriodType {
    Week,
    Month,
    Semester,
}

/// 巩固练习请求
#[derive(Debug, Clone)]
pub struct PracticeRequest {
    pub summary_id: String,
    pub count: Option<u32>,
}

/// ============ Backfill 相关模型 ============

/// Backfill 运行状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BackfillRunStatus {
    Running,
    Completed,
    Failed,
}

impl BackfillRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

/// Backfill 运行记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackfillRun {
    pub id: String,
    pub scope: String,
    pub status: String,
    pub dry_run: bool,
    pub started_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
    pub stats_json: Option<String>,
    pub error_message: Option<String>,
    pub last_table: Option<String>,
    pub last_row_id: Option<String>,
}

/// Backfill 运行统计
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackfillStats {
    pub total_scanned: u64,
    pub upgraded: u64,
    pub skipped: u64,
    pub failed: u64,
}

/// Analysis backfill 更新参数（仅结构化字段，不含 legacy 核心字段）
#[derive(Debug, Clone)]
pub struct AnalysisBackfillFields {
    pub question_markdown_clean: Option<String>,
    pub question_structure_json: Option<String>,
    pub student_answer_text: Option<String>,
    pub teacher_marks_json: Option<String>,
    pub question_type: Option<String>,
    pub difficulty: Option<String>,
    pub error_type: Option<String>,
    pub error_subtype: Option<String>,
    pub root_cause_code: Option<String>,
    pub confidence_json: Option<String>,
    pub pipeline_version: String,
    pub model_trace_json: Option<String>,
}
