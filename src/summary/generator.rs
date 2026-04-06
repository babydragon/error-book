use anyhow::{Context, Result};
use uuid::Uuid;

use crate::config::AppConfig;
use crate::db::models::Summary;
use crate::db::repository::Repository;
use crate::llm::client::ChatClient;

/// 阶段性总结生成器
pub struct SummaryGenerator {
    config: AppConfig,
    chat_client: ChatClient,
    repository: Repository,
}

impl SummaryGenerator {
    pub fn new(config: AppConfig, chat_client: ChatClient, repository: Repository) -> Self {
        Self { config, chat_client, repository }
    }

    /// 生成阶段性总结
    /// 1. 查询时间段内的错题记录
    /// 2. 拼接文本发给 LLM
    /// 3. 解析响应
    /// 4. 存储总结
    pub async fn generate(
        &self,
        subject: &str,
        from: chrono::NaiveDateTime,
        to: chrono::NaiveDateTime,
        period_type: &str,
    ) -> Result<Summary> {
        // 1. 查询错题
        let records = self.repository
            .list_error_records(Some(subject), Some(from), Some(to), None)
            .await?;

        if records.is_empty() {
            anyhow::bail!(
                "在 {} 到 {} 期间，科目 {} 没有错题记录",
                from.format("%Y-%m-%d"),
                to.format("%Y-%m-%d"),
                subject,
            );
        }

        tracing::info!(count = records.len(), "查询到错题记录，开始生成总结");

        // 2. 拼接错题文本
        let records_text = records.iter().enumerate().map(|(i, r)| {
            format!(
                "=== 错题 {} ===\n\
                 知识点: {}\n\
                 原题: {}\n\
                 错误原因: {}\n\
                 改进建议: {}",
                i + 1,
                r.classification,
                r.original_question,
                r.error_reason,
                r.suggestions,
            )
        }).collect::<Vec<_>>().join("\n\n");

        let related_ids: Vec<String> = records.iter().map(|r| r.id.clone()).collect();

        // 3. 调用 LLM
        let grade_level = &self.config.defaults.grade_level;
        let messages = crate::llm::prompts::build_summary_prompt(subject, grade_level, &records_text);
        tracing::info!("调用 LLM 生成总结...");
        let raw_response = self.chat_client.chat(messages, Some(0.3)).await?;
        tracing::debug!(response_len = raw_response.len(), "LLM 总结响应");

        // 4. 解析响应
        let summary_json = crate::analysis::parser::parse_summary_response(&raw_response)
            .context("解析总结响应失败")?;

        // 5. 构造并存储
        let now = chrono::Utc::now();
        let summary = Summary {
            id: Uuid::new_v4().to_string(),
            subject: subject.to_string(),
            period_type: period_type.to_string(),
            period_start: from.and_utc().timestamp(),
            period_end: to.and_utc().timestamp(),
            common_reasons: summary_json.common_reasons,
            common_suggestions: summary_json.common_suggestions,
            weak_points: serde_json::to_string(&summary_json.weak_points)?,
            detail: summary_json.detail,
            related_error_ids: serde_json::to_string(&related_ids)?,
            created_at: now.timestamp(),
        };

        self.repository.insert_summary(&summary).await?;
        tracing::info!(id = %summary.id, "总结生成完成");

        Ok(summary)
    }
}
