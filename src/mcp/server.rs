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
    /// PDF 输出路径（可选，不指定则仅返回文本）
    pub output_path: Option<String>,
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
            Ok(record) => {
                format!(
                    "错题分析完成\nID: {}\n科目: {}\n年级: {}\n知识点: {}\n\n原题:\n{}\n\n错误原因:\n{}\n\n改进建议:\n{}",
                    record.id, record.subject, record.grade_level, record.classification,
                    record.original_question, record.error_reason, record.suggestions
                )
            }
            Err(e) => format!("分析失败: {}", e),
        }
    }

    #[tool(description = "查看错题详情：根据 ID 获取完整的错题记录")]
    async fn show_error(&self, params: Parameters<ShowParams>) -> String {
        let params = params.0;
        let repo = self.repository();
        match repo.get_error_record(&params.id).await {
            Ok(Some(r)) => {
                let date = chrono::DateTime::from_timestamp(r.created_at, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| r.created_at.to_string());
                format!(
                    "ID: {}\n科目: {}\n年级: {}\n知识点: {}\n时间: {}\n\n原题:\n{}\n\n错误原因:\n{}\n\n改进建议:\n{}",
                    r.id, r.subject, r.grade_level, r.classification, date,
                    r.original_question, r.error_reason, r.suggestions
                )
            }
            Ok(None) => format!("未找到记录: {}", params.id),
            Err(e) => format!("查询失败: {}", e),
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
                    "没有找到错题记录".to_string()
                } else {
                    let mut result = format!("共 {} 条错题记录：\n\n", records.len());
                    for r in &records {
                        let date = chrono::DateTime::from_timestamp(r.created_at, 0)
                            .map(|dt| dt.format("%Y-%m-%d").to_string())
                            .unwrap_or_default();
                        let short_id = &r.id[..8.min(r.id.len())];
                        result.push_str(&format!(
                            "{} | {} | {} | {}\n  原题: {}\n\n",
                            short_id, r.subject, r.classification, date,
                            r.original_question.chars().take(80).collect::<String>()
                        ));
                    }
                    result
                }
            }
            Err(e) => format!("查询失败: {}", e),
        }
    }

    #[tool(description = "语义搜索错题：通过自然语言描述搜索相关错题记录")]
    async fn search_errors(&self, params: Parameters<SearchParams>) -> String {
        let params = params.0;
        let limit = params.limit.unwrap_or(10);

        let text_emb = match self.embedding_client.embed(&params.query).await {
            Ok(emb) => emb,
            Err(e) => return format!("向量生成失败: {}", e),
        };

        let repo = self.repository();
        match repo.search_by_text_vector(&text_emb, limit, params.subject.as_deref()).await {
            Ok(results) => {
                if results.is_empty() {
                    "没有找到匹配的错题".to_string()
                } else {
                    let mut result = format!("共 {} 条匹配结果：\n\n", results.len());
                    for r in &results {
                        let record = &r.record;
                        let short_id = &record.id[..8.min(record.id.len())];
                        result.push_str(&format!(
                            "{} | 相似度: {:.4} | {} | {}\n  原题: {}\n  原因: {}\n\n",
                            short_id,
                            r.similarity(),
                            record.subject,
                            record.classification,
                            record.original_question.chars().take(80).collect::<String>(),
                            record.error_reason.chars().take(80).collect::<String>(),
                        ));
                    }
                    result
                }
            }
            Err(e) => format!("搜索失败: {}", e),
        }
    }

    #[tool(description = "生成阶段性总结：分析指定时间段内的错题，总结共性错误原因和改进建议")]
    async fn generate_summary(&self, params: Parameters<SummaryParams>) -> String {
        let params = params.0;
        let from_date = match chrono::NaiveDate::parse_from_str(&params.from, "%Y-%m-%d") {
            Ok(d) => d,
            Err(e) => return format!("起始日期格式错误: {}", e),
        };
        let to_date = match chrono::NaiveDate::parse_from_str(&params.to, "%Y-%m-%d") {
            Ok(d) => d,
            Err(e) => return format!("结束日期格式错误: {}", e),
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
            Ok(summary) => {
                let weak_points: Vec<String> = serde_json::from_str(&summary.weak_points).unwrap_or_default();
                format!(
                    "总结生成完成\nID: {}\n科目: {}\n时间段: {} ~ {}\n类型: {}\n\n共性错误原因:\n{}\n\n共性改进建议:\n{}\n\n薄弱知识点:\n{}\n\n详细分析:\n{}",
                    summary.id, summary.subject, params.from, params.to, summary.period_type,
                    summary.common_reasons, summary.common_suggestions,
                    weak_points.iter().enumerate().map(|(i, wp)| format!("{}. {}", i + 1, wp)).collect::<Vec<_>>().join("\n"),
                    summary.detail
                )
            }
            Err(e) => format!("总结生成失败: {}", e),
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

        match generator.generate(&params.summary_id, count, None).await {
            Ok(practice) => {
                let questions: Vec<PracticeQuestion> =
                    serde_json::from_str(&practice.questions).unwrap_or_default();
                let mut result = format!(
                    "练习生成完成\nID: {}\n总结 ID: {}\n科目: {}\n题目数: {}\n\n",
                    practice.id, practice.summary_id, practice.subject, questions.len()
                );
                for (i, q) in questions.iter().enumerate() {
                    result.push_str(&format!(
                        "第 {} 题\n{}\n答案: {}\n知识点: {}\n\n",
                        i + 1, q.question, q.answer, q.knowledge_points.join("、")
                    ));
                }

                if let Some(ref path) = params.output_path {
                    match crate::pdf::generate_pdf(&practice, path) {
                        Ok(pdf_out) => result.push_str(&format!("PDF 已生成: {}\n", pdf_out.path)),
                        Err(e) => result.push_str(&format!("PDF 生成失败: {}\n", e)),
                    }
                }

                result
            }
            Err(e) => format!("练习生成失败: {}", e),
        }
    }
}

#[tool_handler]
impl ServerHandler for McpHandler {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::default()
            .with_instructions("错题本 AI 助手：可以分析错题图片、查看/搜索错题记录、生成阶段性总结、生成巩固练习题（支持 PDF 输出）。")
    }
}

/// 启动 MCP Server（stdio 模式）
pub async fn run_mcp_server(handler: McpHandler) -> Result<()> {
    tracing::info!("启动 MCP Server (stdio)...");
    let server = serve_server(handler, stdio()).await?;
    server.waiting().await?;
    Ok(())
}
