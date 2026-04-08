use std::sync::Arc;

use anyhow::Result;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router,
    service::serve_server,
    transport::io::stdio,
};
use tokio::sync::Mutex;

use crate::analysis::analyzer::Analyzer;
use crate::analysis::parser::PracticeQuestion;
use crate::config::AppConfig;
use crate::db::models::AnalysisRequest;
use crate::db::repository::Repository;
use crate::llm::client::{ChatClient, EmbeddingClient};
use crate::practice::generator::PracticeGenerator;
use crate::storage::image::ImageStorage;
use crate::summary::generator::SummaryGenerator;

fn json_ok(data: serde_json::Value) -> String {
    serde_json::json!({
        "ok": true,
        "data": data,
    })
    .to_string()
}

fn json_err(message: impl Into<String>) -> String {
    serde_json::json!({
        "ok": false,
        "error": message.into(),
    })
    .to_string()
}

/// MCP Server handler
pub struct McpHandler {
    config: Arc<AppConfig>,
    db: Arc<libsql::Database>,
    chat_client: Arc<ChatClient>,
    embedding_client: Arc<EmbeddingClient>,
    image_storage: Arc<ImageStorage>,
    concurrency: Arc<Mutex<()>>,
    tool_router: ToolRouter<Self>,
}

impl McpHandler {
    pub fn new(
        config: AppConfig,
        db: Arc<libsql::Database>,
        chat_client: ChatClient,
        embedding_client: EmbeddingClient,
        image_storage: ImageStorage,
    ) -> Self {
        Self {
            config: Arc::new(config),
            db,
            chat_client: Arc::new(chat_client),
            embedding_client: Arc::new(embedding_client),
            image_storage: Arc::new(image_storage),
            concurrency: Arc::new(Mutex::new(())),
            tool_router: Self::tool_router(),
        }
    }

    fn repository(&self) -> Repository {
        Repository::new(Arc::clone(&self.db))
    }
}

// ============ Tool parameter structs ============

#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct AnalyzeParams {
    /// 错题图片文件路径
    pub image_path: String,
    /// 科目（可选，不指定则由AI判断）
    pub subject: Option<String>,
    /// 年级（可选）
    pub grade_level: Option<String>,
    /// 老师批改颜色（默认红色）
    pub color_teacher: Option<String>,
    /// 订正颜色（默认蓝色）
    pub color_correction: Option<String>,
}

#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct ShowParams {
    /// 错题记录 ID
    pub id: String,
}

#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct ShowSummaryParams {
    /// 总结记录 ID
    pub summary_id: String,
}

#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct ShowPracticeParams {
    /// 练习集 ID
    pub practice_id: String,
}

#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct ListParams {
    /// 按科目筛选（可选）
    pub subject: Option<String>,
    /// 起始日期 YYYY-MM-DD（可选）
    pub from: Option<String>,
    /// 结束日期 YYYY-MM-DD（可选）
    pub to: Option<String>,
    /// 返回条数限制（默认20）
    pub limit: Option<u32>,
}

#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct SearchParams {
    /// 搜索文本
    pub query: String,
    /// 按科目筛选（可选）
    pub subject: Option<String>,
    /// 返回条数限制（默认10）
    pub limit: Option<u32>,
}

#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct SummaryParams {
    /// 科目
    pub subject: String,
    /// 起始日期 YYYY-MM-DD
    pub from: String,
    /// 结束日期 YYYY-MM-DD
    pub to: String,
    /// 总结类型（默认 week）
    pub period_type: Option<String>,
}

#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct PracticeParams {
    /// 总结记录 ID
    pub summary_id: String,
    /// 题目数量（默认10）
    pub count: Option<u32>,
    /// 额外要求（如题型、难度、特殊限制等）
    pub requirements: Option<String>,
    /// PDF 输出路径（可选，不指定则仅返回文本）
    pub output_path: Option<String>,
}

#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct ListSummariesParams {
    /// 按科目筛选（可选）
    pub subject: Option<String>,
    /// 返回条数限制（默认20）
    pub limit: Option<u32>,
}

#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct ListPracticesParams {
    /// 按科目筛选（可选）
    pub subject: Option<String>,
    /// 按总结记录筛选（可选）
    pub summary_id: Option<String>,
    /// 返回条数限制（默认20）
    pub limit: Option<u32>,
}

#[derive(Debug, rmcp::serde::Deserialize, rmcp::schemars::JsonSchema)]
pub struct PracticePdfParams {
    /// 练习集 ID
    pub practice_id: String,
    /// PDF 输出路径
    pub output_path: String,
}

// ============ Tool implementations ============

#[tool_router]
impl McpHandler {
    #[tool(description = "分析错题图片：识别题目内容、分析错误原因、给出改进建议")]
    async fn analyze_error(&self, params: Parameters<AnalyzeParams>) -> String {
        let params = params.0;
        let request = AnalysisRequest {
            image_path: params.image_path,
            subject: params.subject,
            grade_level: params.grade_level,
            color_teacher: params.color_teacher,
            color_correction: params.color_correction,
        };

        let _guard = self.concurrency.lock().await;
        let analyzer = Analyzer::new(
            (*self.config).clone(),
            (*self.chat_client).clone(),
            (*self.embedding_client).clone(),
            (*self.image_storage).clone(),
            self.repository(),
        );

        match analyzer.analyze(request).await {
            Ok(record) => json_ok(serde_json::json!({
                "type": "error_record",
                "id": record.id,
                "subject": record.subject,
                "grade_level": record.grade_level,
                "classification": serde_json::from_str::<Vec<String>>(&record.classification).unwrap_or_default(),
                "original_question": record.original_question,
                "error_reason": record.error_reason,
                "suggestions": record.suggestions,
                "created_at": record.created_at,
            })),
            Err(e) => json_err(format!("分析失败: {}", e)),
        }
    }

    #[tool(description = "查看错题详情：根据 ID 获取完整的错题记录")]
    async fn show_error(&self, params: Parameters<ShowParams>) -> String {
        let params = params.0;
        let repo = self.repository();
        match repo.get_error_record(&params.id).await {
            Ok(Some(r)) => json_ok(serde_json::json!({
                "type": "error_record",
                "id": r.id,
                "subject": r.subject,
                "grade_level": r.grade_level,
                "classification": serde_json::from_str::<Vec<String>>(&r.classification).unwrap_or_default(),
                "original_question": r.original_question,
                "error_reason": r.error_reason,
                "suggestions": r.suggestions,
                "created_at": r.created_at,
                "created_at_text": chrono::DateTime::from_timestamp(r.created_at, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| r.created_at.to_string()),
            })),
            Ok(None) => json_err(format!("未找到记录: {}", params.id)),
            Err(e) => json_err(format!("查询失败: {}", e)),
        }
    }

    #[tool(description = "查看总结详情：根据 summary_id 获取完整的阶段性总结")]
    async fn show_summary(&self, params: Parameters<ShowSummaryParams>) -> String {
        let params = params.0;
        let repo = self.repository();
        match repo.get_summary(&params.summary_id).await {
            Ok(Some(s)) => json_ok(serde_json::json!({
                "type": "summary",
                "id": s.id,
                "subject": s.subject,
                "period_type": s.period_type,
                "period_start": s.period_start,
                "period_end": s.period_end,
                "period_start_text": chrono::DateTime::from_timestamp(s.period_start, 0)
                    .map(|dt| dt.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| s.period_start.to_string()),
                "period_end_text": chrono::DateTime::from_timestamp(s.period_end, 0)
                    .map(|dt| dt.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| s.period_end.to_string()),
                "common_reasons": s.common_reasons,
                "common_suggestions": s.common_suggestions,
                "weak_points": serde_json::from_str::<Vec<String>>(&s.weak_points).unwrap_or_default(),
                "related_error_ids": serde_json::from_str::<Vec<String>>(&s.related_error_ids).unwrap_or_default(),
                "detail": s.detail,
                "created_at": s.created_at,
                "created_at_text": chrono::DateTime::from_timestamp(s.created_at, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| s.created_at.to_string()),
            })),
            Ok(None) => json_err(format!("未找到总结记录: {}", params.summary_id)),
            Err(e) => json_err(format!("查询失败: {}", e)),
        }
    }

    #[tool(description = "查看练习详情：根据 practice_id 获取完整的练习题、答案、知识点和 PDF 路径")]
    async fn show_practice(&self, params: Parameters<ShowPracticeParams>) -> String {
        let params = params.0;
        let repo = self.repository();
        match repo.get_practice_set(&params.practice_id).await {
            Ok(Some(p)) => {
                let questions: Vec<PracticeQuestion> = serde_json::from_str(&p.questions).unwrap_or_default();
                json_ok(serde_json::json!({
                    "type": "practice",
                    "id": p.id,
                    "summary_id": p.summary_id,
                    "subject": p.subject,
                    "requirements": p.requirements,
                    "questions": questions,
                    "question_count": questions.len(),
                    "pdf_path": p.pdf_path,
                    "created_at": p.created_at,
                    "created_at_text": chrono::DateTime::from_timestamp(p.created_at, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                        .unwrap_or_else(|| p.created_at.to_string()),
                }))
            }
            Ok(None) => json_err(format!("未找到练习集: {}", params.practice_id)),
            Err(e) => json_err(format!("查询失败: {}", e)),
        }
    }

    #[tool(description = "列出错题记录：按科目、时间范围筛选")]
    async fn list_errors(&self, params: Parameters<ListParams>) -> String {
        let params = params.0;
        let repo = self.repository();
        let limit = params.limit.unwrap_or(20);

        let from_dt = params.from.as_deref()
            .map(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d"))
            .transpose()
            .ok()
            .flatten()
            .and_then(|d| d.and_hms_opt(0, 0, 0));

        let to_dt = params.to.as_deref()
            .map(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d"))
            .transpose()
            .ok()
            .flatten()
            .and_then(|d| d.and_hms_opt(23, 59, 59));

        match repo.list_error_records(params.subject.as_deref(), from_dt, to_dt, Some(limit)).await {
            Ok(records) => {
                if records.is_empty() {
                    json_ok(serde_json::json!({"items": []}))
                } else {
                    json_ok(serde_json::json!({
                        "items": records.into_iter().map(|r| serde_json::json!({
                            "id": r.id,
                            "subject": r.subject,
                            "grade_level": r.grade_level,
                            "classification": serde_json::from_str::<Vec<String>>(&r.classification).unwrap_or_default(),
                            "original_question_preview": r.original_question.chars().take(80).collect::<String>(),
                            "created_at": r.created_at,
                            "created_at_text": chrono::DateTime::from_timestamp(r.created_at, 0)
                                .map(|dt| dt.format("%Y-%m-%d").to_string())
                                .unwrap_or_else(|| r.created_at.to_string()),
                        })).collect::<Vec<_>>()
                    }))
                }
            }
            Err(e) => json_err(format!("查询失败: {}", e)),
        }
    }

    #[tool(description = "列出阶段性总结：按科目筛选已生成的总结记录")]
    async fn list_summaries(&self, params: Parameters<ListSummariesParams>) -> String {
        let params = params.0;
        let repo = self.repository();
        let limit = params.limit.unwrap_or(20);

        match repo.list_summaries(params.subject.as_deref(), Some(limit)).await {
            Ok(summaries) => {
                if summaries.is_empty() {
                    json_ok(serde_json::json!({"items": []}))
                } else {
                    json_ok(serde_json::json!({
                        "items": summaries.into_iter().map(|s| serde_json::json!({
                            "id": s.id,
                            "subject": s.subject,
                            "period_type": s.period_type,
                            "period_start": s.period_start,
                            "period_end": s.period_end,
                            "period_start_text": chrono::DateTime::from_timestamp(s.period_start, 0)
                                .map(|dt| dt.format("%Y-%m-%d").to_string())
                                .unwrap_or_else(|| s.period_start.to_string()),
                            "period_end_text": chrono::DateTime::from_timestamp(s.period_end, 0)
                                .map(|dt| dt.format("%Y-%m-%d").to_string())
                                .unwrap_or_else(|| s.period_end.to_string()),
                            "created_at": s.created_at,
                            "created_at_text": chrono::DateTime::from_timestamp(s.created_at, 0)
                                .map(|dt| dt.format("%Y-%m-%d").to_string())
                                .unwrap_or_else(|| s.created_at.to_string()),
                        })).collect::<Vec<_>>()
                    }))
                }
            }
            Err(e) => json_err(format!("查询失败: {}", e)),
        }
    }

    #[tool(description = "列出已生成的练习题：按科目或总结 ID 筛选练习记录")]
    async fn list_practices(&self, params: Parameters<ListPracticesParams>) -> String {
        let params = params.0;
        let repo = self.repository();
        let limit = params.limit.unwrap_or(20);

        match repo
            .list_practice_sets(params.subject.as_deref(), params.summary_id.as_deref(), Some(limit))
            .await
        {
            Ok(practices) => {
                if practices.is_empty() {
                    json_ok(serde_json::json!({"items": []}))
                } else {
                    json_ok(serde_json::json!({
                        "items": practices.into_iter().map(|p| {
                            let questions: Vec<PracticeQuestion> = serde_json::from_str(&p.questions).unwrap_or_default();
                            serde_json::json!({
                                "id": p.id,
                                "summary_id": p.summary_id,
                                "subject": p.subject,
                                "requirements": p.requirements,
                                "question_count": questions.len(),
                                "pdf_path": p.pdf_path,
                                "created_at": p.created_at,
                                "created_at_text": chrono::DateTime::from_timestamp(p.created_at, 0)
                                    .map(|dt| dt.format("%Y-%m-%d").to_string())
                                    .unwrap_or_else(|| p.created_at.to_string()),
                            })
                        }).collect::<Vec<_>>()
                    }))
                }
            }
            Err(e) => json_err(format!("查询失败: {}", e)),
        }
    }

    #[tool(description = "语义搜索错题：通过自然语言描述搜索相关错题记录")]
    async fn search_errors(&self, params: Parameters<SearchParams>) -> String {
        let params = params.0;
        let limit = params.limit.unwrap_or(10);

        let text_emb = match self.embedding_client.embed(&params.query).await {
            Ok(emb) => emb,
            Err(e) => return json_err(format!("向量生成失败: {}", e)),
        };

        let repo = self.repository();
        match repo.search_by_text_vector(&text_emb, limit, params.subject.as_deref()).await {
            Ok(results) => {
                if results.is_empty() {
                    json_ok(serde_json::json!({"items": []}))
                } else {
                    json_ok(serde_json::json!({
                        "items": results.into_iter().map(|r| {
                            let similarity = r.similarity();
                            let record = r.record;
                            serde_json::json!({
                                "id": record.id,
                                "similarity": similarity,
                                "subject": record.subject,
                                "classification": serde_json::from_str::<Vec<String>>(&record.classification).unwrap_or_default(),
                                "original_question_preview": record.original_question.chars().take(80).collect::<String>(),
                                "error_reason_preview": record.error_reason.chars().take(80).collect::<String>(),
                                "created_at": record.created_at,
                            })
                        }).collect::<Vec<_>>()
                    }))
                }
            }
            Err(e) => json_err(format!("搜索失败: {}", e)),
        }
    }

    #[tool(description = "生成阶段性总结：分析指定时间段内的错题，总结共性错误原因和改进建议")]
    async fn generate_summary(&self, params: Parameters<SummaryParams>) -> String {
        let params = params.0;
        let from_date = match chrono::NaiveDate::parse_from_str(&params.from, "%Y-%m-%d") {
            Ok(d) => d,
            Err(e) => return json_err(format!("起始日期格式错误: {}", e)),
        };
        let to_date = match chrono::NaiveDate::parse_from_str(&params.to, "%Y-%m-%d") {
            Ok(d) => d,
            Err(e) => return json_err(format!("结束日期格式错误: {}", e)),
        };
        let period_type = params.period_type.unwrap_or_else(|| "week".to_string());

        let _guard = self.concurrency.lock().await;
        let generator = SummaryGenerator::new(
            (*self.config).clone(),
            (*self.chat_client).clone(),
            self.repository(),
        );

        match generator.generate(
            &params.subject,
            from_date.and_hms_opt(0, 0, 0).unwrap(),
            to_date.and_hms_opt(23, 59, 59).unwrap(),
            &period_type,
        ).await {
            Ok(summary) => json_ok(serde_json::json!({
                "type": "summary",
                "id": summary.id,
                "subject": summary.subject,
                "period_type": summary.period_type,
                "period_start_text": params.from,
                "period_end_text": params.to,
                "common_reasons": summary.common_reasons,
                "common_suggestions": summary.common_suggestions,
                "weak_points": serde_json::from_str::<Vec<String>>(&summary.weak_points).unwrap_or_default(),
                "detail": summary.detail,
                "created_at": summary.created_at,
            })),
            Err(e) => json_err(format!("总结生成失败: {}", e)),
        }
    }

    #[tool(description = "生成巩固练习题：基于阶段性总结生成新的练习题目，可选输出 PDF")]
    async fn generate_practice(&self, params: Parameters<PracticeParams>) -> String {
        let params = params.0;
        let count = params.count.unwrap_or(10);

        let _guard = self.concurrency.lock().await;
        let generator = PracticeGenerator::new(
            (*self.config).clone(),
            (*self.chat_client).clone(),
            self.repository(),
        );

        match generator
            .generate(&params.summary_id, count, params.requirements.as_deref(), None)
            .await
        {
            Ok(practice) => {
                let questions: Vec<PracticeQuestion> = serde_json::from_str(&practice.questions).unwrap_or_default();
                let mut pdf_path = practice.pdf_path.clone();
                if let Some(ref path) = params.output_path {
                    match crate::pdf::generate_pdf(&practice, &self.config.pdf, path) {
                        Ok(pdf_out) => pdf_path = Some(pdf_out.path),
                        Err(e) => return json_err(format!("PDF 生成失败: {}", e)),
                    }
                }
                json_ok(serde_json::json!({
                    "type": "practice",
                    "id": practice.id,
                    "summary_id": practice.summary_id,
                    "subject": practice.subject,
                    "requirements": practice.requirements,
                    "questions": questions,
                    "question_count": questions.len(),
                    "pdf_path": pdf_path,
                    "created_at": practice.created_at,
                }))
            }
            Err(e) => json_err(format!("练习生成失败: {}", e)),
        }
    }

    #[tool(description = "为已生成的练习集导出 PDF：根据 practice_id 重新生成 PDF，不调用 LLM")]
    async fn generate_practice_pdf(&self, params: Parameters<PracticePdfParams>) -> String {
        let params = params.0;
        let repo = self.repository();

        match repo.get_practice_set(&params.practice_id).await {
            Ok(Some(practice)) => match crate::pdf::generate_pdf(&practice, &self.config.pdf, &params.output_path) {
                Ok(pdf_out) => {
                    if let Err(e) = repo
                        .update_practice_set_pdf_path(&params.practice_id, &pdf_out.path)
                        .await
                    {
                        return json_err(format!(
                            "PDF 已生成: {}，但更新数据库中的 pdf_path 失败: {}",
                            pdf_out.path, e
                        ));
                    }
                    json_ok(serde_json::json!({
                        "type": "practice_pdf",
                        "practice_id": practice.id,
                        "subject": practice.subject,
                        "output_path": pdf_out.path,
                    }))
                }
                Err(e) => json_err(format!("PDF 生成失败: {}", e)),
            },
            Ok(None) => json_err(format!("未找到练习集: {}", params.practice_id)),
            Err(e) => json_err(format!("查询失败: {}", e)),
        }
    }
}

#[tool_handler]
impl ServerHandler for McpHandler {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::default()
            .with_instructions("错题本 AI 助手：支持 analyze -> summary -> practice -> pdf 工作流。优先用 list_* 查找 ID，再用 show_* 获取完整内容；用户已有 summary_id 或 practice_id 时应直接复用，避免重复调用 LLM。")
    }
}

/// 启动 MCP Server（stdio 模式）
pub async fn run_mcp_server(handler: McpHandler) -> Result<()> {
    tracing::info!("启动 MCP Server (stdio)...");
    let server = serve_server(handler, stdio()).await?;
    server.waiting().await?;
    Ok(())
}
