use std::path::Path;

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::config::AppConfig;
use crate::db::models::{AnalysisRequest, ErrorRecord};
use crate::db::repository::Repository;
use crate::llm::client::{ChatClient, ChatMessage};
use crate::llm::embedding::EmbeddingClient;
use crate::llm::prompts;
use crate::storage::image::ImageStorage;

use super::parser;

/// 错题分析器
pub struct Analyzer {
    config: AppConfig,
    chat_client: ChatClient,
    embedding_client: EmbeddingClient,
    image_storage: ImageStorage,
    repository: Repository,
}

impl Analyzer {
    pub fn new(
        config: AppConfig,
        chat_client: ChatClient,
        embedding_client: EmbeddingClient,
        image_storage: ImageStorage,
        repository: Repository,
    ) -> Self {
        Self {
            config,
            chat_client,
            embedding_client,
            image_storage,
            repository,
        }
    }

    /// 分析错题图片
    pub async fn analyze(&self, request: AnalysisRequest) -> Result<ErrorRecord> {
        let image_path = Path::new(&request.image_path);
        if !image_path.exists() {
            anyhow::bail!("图片文件不存在: {}", request.image_path);
        }

        // 1. 保存图片到存储目录
        let stored_name = self.image_storage.save(image_path).await?;
        tracing::info!(stored_name = %stored_name, "图片已保存");

        // 2. 读取图片为 base64
        let image_base64 = self.image_storage.read_base64(&stored_name).await?;
        let media_type = detect_media_type(image_path);

        // 3. 构建 Prompt
        let mut messages = prompts::build_analysis_prompt(&self.config, &request);
        messages.push(ChatMessage::user_image_text(
            &image_base64,
            media_type,
            prompts::analysis_user_text(),
        ));

        // 4. 调用 LLM
        tracing::info!("开始调用 LLM 分析错题...");
        let raw_response = self.chat_client.chat(messages, Some(0.3)).await?;
        tracing::debug!(response_len = raw_response.len(), "LLM 原始响应");

        // 5. 解析响应
        let (_, analysis_result) = parser::parse_analysis_response(&raw_response)
            .context("解析 LLM 响应失败")?;

        // 6. 生成 embedding
        let embedding_text = format!(
            "科目: {}\n知识点: {}\n原题: {}\n原因: {}\n建议: {}",
            analysis_result.subject,
            analysis_result.classification.join("、"),
            analysis_result.original_question,
            analysis_result.error_reason,
            analysis_result.suggestions,
        );

        tracing::info!("开始生成文本 embedding...");
        let text_embedding = self.embedding_client
            .embed(&embedding_text)
            .await?;
        tracing::info!(dimensions = text_embedding.len(), "文本 Embedding 生成完成");

        if !self.embedding_client.supports_image_embedding() {
            anyhow::bail!(
                "当前 embedding provider 不支持图片 embedding；analyze 需要 llm.embedding.provider=google"
            );
        }

        tracing::info!("开始生成图片 embedding...");
        let image_embedding = self.embedding_client
            .embed_image_only(&image_base64, media_type)
            .await?;
        tracing::info!(dimensions = image_embedding.len(), "图片 Embedding 生成完成");

        // 7. 构造 ErrorRecord 并存储
        let now = chrono::Utc::now();
        let record = ErrorRecord {
            id: Uuid::new_v4().to_string(),
            image_path: stored_name,
            subject: analysis_result.subject.clone(),
            grade_level: request
                .grade_level
                .unwrap_or_else(|| self.config.defaults.grade_level.clone()),
            original_question: analysis_result.original_question.clone(),
            image_regions: if analysis_result.image_regions.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&analysis_result.image_regions)?)
            },
            classification: serde_json::to_string(&analysis_result.classification)?,
            error_reason: analysis_result.error_reason.clone(),
            suggestions: analysis_result.suggestions.clone(),
            text_embedding,
            image_embedding,
            created_at: now.timestamp(),
        };

        self.repository
            .insert_error_record(&record, &analysis_result.classification)
            .await?;

        tracing::info!(id = %record.id, "错题分析完成");
        Ok(record)
    }
}

fn detect_media_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png", // 默认
    }
}
