use anyhow::{Context, Result};
use uuid::Uuid;

use crate::config::AppConfig;
use crate::db::models::PracticeSet;
use crate::db::repository::Repository;
use crate::llm::client::ChatClient;

/// 巩固练习生成器
pub struct PracticeGenerator {
    config: AppConfig,
    chat_client: ChatClient,
    repository: Repository,
}

impl PracticeGenerator {
    pub fn new(config: AppConfig, chat_client: ChatClient, repository: Repository) -> Self {
        Self { config, chat_client, repository }
    }

    /// 生成巩固练习题
    /// 1. 读取总结（获取薄弱知识点 + 原题参考）
    /// 2. 调用 LLM 生成新题目
    /// 3. 解析响应
    /// 4. 存储（可选生成 PDF）
    pub async fn generate(
        &self,
        summary_id: &str,
        count: u32,
        requirements: Option<&str>,
        pdf_path: Option<&str>,
    ) -> Result<PracticeSet> {
        // 1. 读取总结
        let summary = self.repository.get_summary(summary_id).await?
            .ok_or_else(|| anyhow::anyhow!("总结记录不存在: {}", summary_id))?;

        tracing::info!(id = %summary.id, subject = %summary.subject, "读取总结，开始生成练习题");

        // 2. 获取参考题目
        let related_ids: Vec<String> = serde_json::from_str(&summary.related_error_ids)
            .unwrap_or_default();
        let mut reference_questions = Vec::new();
        for rid in related_ids.iter().take(5) {
            if let Some(record) = self.repository.get_error_record(rid).await? {
                let classification: Vec<String> = serde_json::from_str(&record.classification)
                    .unwrap_or_default();
                let question_preview = truncate_chars(&record.original_question, 400);
                reference_questions.push(format!(
                    "知识点: {}\n原题: {}",
                    classification.join("、"),
                    question_preview,
                ));
            }
        }
        let reference_text = reference_questions.join("\n\n");

        // 3. 解析薄弱知识点
        let weak_points: Vec<String> = serde_json::from_str(&summary.weak_points)
            .unwrap_or_default();
        if weak_points.is_empty() {
            anyhow::bail!("总结中没有薄弱知识点，无法生成练习题");
        }

        // 4. 调用 LLM
        let grade_level = &self.config.defaults.grade_level;
        let requirements = normalize_requirements(requirements);
        let messages = crate::llm::prompts::build_practice_prompt(
            &summary.subject,
            grade_level,
            &weak_points,
            &reference_text,
            count,
            requirements.as_deref(),
        );
        tracing::info!("调用 LLM 生成练习题...");
        let raw_response = self.chat_client.chat(messages, Some(0.2)).await?;
        tracing::debug!(response_len = raw_response.len(), "LLM 练习题响应");

        // 5. 解析响应
        let mut questions = crate::analysis::parser::parse_practice_response(&raw_response)
            .context("解析练习题响应失败")?;

        if questions.len() < count as usize {
            let missing = count as usize - questions.len();
            tracing::warn!(expected = count, actual = questions.len(), missing, "练习题数量不足，尝试补生成");

            let retry_messages = crate::llm::prompts::build_practice_fill_prompt(
                &summary.subject,
                grade_level,
                &weak_points,
                &questions,
                missing as u32,
                requirements.as_deref(),
            );
            let retry_raw = self.chat_client.chat(retry_messages, Some(0.2)).await?;
            tracing::debug!(response_len = retry_raw.len(), missing, "LLM 补生成练习题响应");

            let mut extra_questions = crate::analysis::parser::parse_practice_response(&retry_raw)
                .context("解析补生成练习题响应失败")?;
            questions.append(&mut extra_questions);
        }

        if questions.len() != count as usize {
            anyhow::bail!(
                "练习题数量不符合预期：期望 {} 道，实际 {} 道",
                count,
                questions.len()
            );
        }

        // 6. 存储练习集
        let now = chrono::Utc::now();
        let questions_json = serde_json::to_string(&questions)?;
        let practice = PracticeSet {
            id: Uuid::new_v4().to_string(),
            summary_id: summary_id.to_string(),
            subject: summary.subject.clone(),
            requirements: requirements.clone(),
            questions: questions_json,
            pdf_path: pdf_path.map(|p| p.to_string()),
            created_at: now.timestamp(),
        };

        self.repository.insert_practice_set(&practice).await?;
        tracing::info!(id = %practice.id, count = questions.len(), "练习题生成完成");

        Ok(practice)
    }
}

fn normalize_requirements(requirements: Option<&str>) -> Option<String> {
    let trimmed = requirements?.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(trimmed.chars().take(500).collect())
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for ch in s.chars() {
        if count >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
        count += 1;
    }
    out
}
