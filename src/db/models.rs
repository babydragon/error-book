use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

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
}

/// 巩固练习
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PracticeSet {
    pub id: String,
    pub summary_id: String,
    pub subject: String,
    /// JSON 数组 [{question, answer, ...}]
    pub questions: String,
    pub pdf_path: Option<String>,
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
