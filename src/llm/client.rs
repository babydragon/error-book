use anyhow::{Context, Result};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::sleep;

use crate::config::AppConfig;

/// OpenAI Chat Completions API 请求
#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: serde_json::Value,
}

impl ChatMessage {
    pub fn system(content: &str) -> Self {
        Self {
            role: "system".to_string(),
            content: serde_json::Value::String(content.to_string()),
        }
    }

    pub fn user_text(content: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: serde_json::Value::String(content.to_string()),
        }
    }

    /// 构造包含图片的用户消息
    /// image_base64: base64 编码的图片数据
    /// media_type: image/png | image/jpeg
    /// text: 文本内容
    pub fn user_image_text(image_base64: &str, media_type: &str, text: &str) -> Self {
        let content = serde_json::json!([
            {
                "type": "image_url",
                "image_url": {
                    "url": format!("data:{};base64,{}", media_type, image_base64)
                }
            },
            {
                "type": "text",
                "text": text
            }
        ]);
        Self {
            role: "user".to_string(),
            content,
        }
    }
}

/// Chat Completions API 响应
#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

/// LLM Chat 客户端
#[derive(Clone)]
pub struct ChatClient {
    http: reqwest::Client,
    api_url: String,
    api_key: String,
    model: String,
    retry_config: crate::config::RetryConfig,
}

impl ChatClient {
    pub fn new(config: &AppConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_url: config.chat_api_url(),
            api_key: config.llm.api_key.clone(),
            model: config.llm.chat_model.clone(),
            retry_config: config.llm.retry.clone(),
        }
    }

    /// 发送 Chat 请求（含自动重试）
    pub async fn chat(&self, messages: Vec<ChatMessage>, temperature: Option<f64>) -> Result<String> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages,
            max_tokens: Some(4096),
            temperature,
        };

        let mut last_error = None;

        for attempt in 0..self.retry_config.max_attempts {
            match self.send_request(&request).await {
                Ok(content) => return Ok(content),
                Err(e) => {
                    let should_retry = self.should_retry(&e);
                    tracing::warn!(
                        attempt = attempt + 1,
                        max = self.retry_config.max_attempts,
                        retryable = should_retry,
                        error = %e,
                        "Chat API 请求失败"
                    );

                    if !should_retry || attempt + 1 >= self.retry_config.max_attempts {
                        return Err(e);
                    }

                    let delay = self.calculate_delay(attempt);
                    tracing::info!(?delay, "等待重试...");
                    sleep(delay).await;
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("未知错误")))
    }

    async fn send_request(&self, request: &ChatRequest) -> Result<String> {
        let response = self
            .http
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(request)
            .send()
            .await
            .context("Chat API 网络请求失败")?;

        let status = response.status();
        if status.is_success() {
            let chat_resp: ChatResponse = response
                .json()
                .await
                .context("解析 Chat API 响应失败")?;
            chat_resp
                .choices
                .into_iter()
                .next()
                .map(|c| c.message.content)
                .ok_or_else(|| anyhow::anyhow!("Chat API 返回空响应"))
        } else {
            let body = response.text().await.unwrap_or_default();
            let retryable = self.retry_config.is_retryable(status.as_u16());
            Err(LlmError::ApiError {
                status: status.as_u16(),
                body,
                retryable,
            }
            .into())
        }
    }

    fn should_retry(&self, error: &anyhow::Error) -> bool {
        if let Some(llm_err) = error.downcast_ref::<LlmError>() {
            llm_err.is_retryable()
        } else {
            // 网络错误可重试
            true
        }
    }

    fn calculate_delay(&self, attempt: u32) -> Duration {
        let base = self.retry_config.base_delay();
        let max = self.retry_config.max_delay();
        let exp_delay = base * 2u32.saturating_pow(attempt);
        let jitter = Duration::from_millis(rand::rng().random_range(0..base.as_millis() as u64));
        let delay = exp_delay + jitter;
        delay.min(max)
    }
}

/// Embedding 客户端（Google AI Studio embedContent 格式）
#[derive(Clone)]
pub struct EmbeddingClient {
    http: reqwest::Client,
    api_url: String,
    api_key: String,
    model: String,
    dimensions: u32,
    retry_config: crate::config::RetryConfig,
}

/// Google AI Studio embedContent 响应
#[derive(Debug, Deserialize)]
struct GoogleEmbedResponse {
    embedding: GoogleEmbedding,
}

#[derive(Debug, Deserialize)]
struct GoogleEmbedding {
    values: Vec<f32>,
}

/// Google AI Studio embedContent 请求体中的 content parts 构建辅助
enum ContentPart {
    Text { text: String },
    Image { mime_type: String, data: String },
}

impl ContentPart {
    fn to_json(&self) -> serde_json::Value {
        match self {
            ContentPart::Text { text } => serde_json::json!({ "text": text }),
            ContentPart::Image { mime_type, data } => serde_json::json!({
                "inline_data": {
                    "mimeType": mime_type,
                    "data": data,
                }
            }),
        }
    }
}

impl EmbeddingClient {
    pub fn new(config: &AppConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_url: config.google_embed_url(),
            api_key: config.llm.api_key.clone(),
            model: config.llm.embedding_model.clone(),
            dimensions: config.llm.embedding_dimensions,
            retry_config: config.llm.retry.clone(),
        }
    }

    /// 生成纯文本 embedding
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let parts = vec![ContentPart::Text { text: text.to_string() }];
        self.send_embed(parts).await
    }

    /// 多模态 embedding：图片 + 文本
    pub async fn embed_with_image(
        &self,
        image_base64: &str,
        media_type: &str,
        text: &str,
    ) -> Result<Vec<f32>> {
        let mut parts = vec![ContentPart::Image {
            mime_type: media_type.to_string(),
            data: image_base64.to_string(),
        }];
        if !text.is_empty() {
            parts.push(ContentPart::Text { text: text.to_string() });
        }
        self.send_embed(parts).await
    }

    /// 纯图片 embedding（无文本）
    pub async fn embed_image_only(
        &self,
        image_base64: &str,
        media_type: &str,
    ) -> Result<Vec<f32>> {
        let parts = vec![ContentPart::Image {
            mime_type: media_type.to_string(),
            data: image_base64.to_string(),
        }];
        self.send_embed(parts).await
    }

    /// 构建 Google AI Studio embedContent 请求并发送
    /// POST {api_url}
    /// {
    ///   "model": "models/{model}",
    ///   "content": { "parts": [...] },
    ///   "config": { "output_dimensionality": N }
    /// }
    async fn send_embed(&self, parts: Vec<ContentPart>) -> Result<Vec<f32>> {
        let body = serde_json::json!({
            "model": format!("models/{}", self.model),
            "content": {
                "parts": parts.iter().map(|p| p.to_json()).collect::<Vec<_>>()
            },
            "outputDimensionality": self.dimensions
        });
        tracing::debug!("Google Embedding 请求: URL={}, body={}", self.api_url, serde_json::to_string(&body).unwrap_or_default());

        let mut last_error = None;

        for attempt in 0..self.retry_config.max_attempts {
            match self.send_request(&body).await {
                Ok(embedding) => return Ok(embedding),
                Err(e) => {
                    let should_retry = self.should_retry(&e);
                    tracing::warn!(
                        attempt = attempt + 1,
                        retryable = should_retry,
                        error = %e,
                        "Google Embedding API 请求失败"
                    );
                    if !should_retry || attempt + 1 >= self.retry_config.max_attempts {
                        return Err(e);
                    }
                    let delay = self.calculate_delay(attempt);
                    sleep(delay).await;
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("未知错误")))
    }

    async fn send_request(&self, body: &serde_json::Value) -> Result<Vec<f32>> {
        let response = self
            .http
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await
            .context("Google Embedding API 网络请求失败")?;

        let status = response.status();
        if status.is_success() {
            let resp: GoogleEmbedResponse = response
                .json()
                .await
                .context("解析 Google Embedding 响应失败")?;
            Ok(resp.embedding.values)
        } else {
            let body = response.text().await.unwrap_or_default();
            let retryable = self.retry_config.is_retryable(status.as_u16());
            Err(LlmError::ApiError {
                status: status.as_u16(),
                body,
                retryable,
            }
            .into())
        }
    }

    fn should_retry(&self, error: &anyhow::Error) -> bool {
        if let Some(llm_err) = error.downcast_ref::<LlmError>() {
            llm_err.is_retryable()
        } else {
            true
        }
    }

    fn calculate_delay(&self, attempt: u32) -> Duration {
        let base = self.retry_config.base_delay();
        let max = self.retry_config.max_delay();
        let exp_delay = base * 2u32.saturating_pow(attempt);
        let jitter = Duration::from_millis(rand::rng().random_range(0..base.as_millis() as u64));
        let delay = exp_delay + jitter;
        delay.min(max)
    }
}

/// LLM 错误类型
#[derive(Debug, thiserror::Error)]
enum LlmError {
    #[error("API 错误 (status={status}): {body}")]
    ApiError {
        status: u16,
        body: String,
        retryable: bool,
    },
}

impl LlmError {
    fn is_retryable(&self) -> bool {
        match self {
            LlmError::ApiError { retryable, .. } => *retryable,
        }
    }
}
