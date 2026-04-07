use anyhow::{Context, Result};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::sleep;

use crate::config::{AppConfig, ChatProvider, EmbeddingProvider};

/// OpenAI Chat Completions API 请求
#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<OpenAiChatMessage>,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub parts: Vec<ChatPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChatPart {
    Text(String),
    Image { mime_type: String, data: String },
}

#[derive(Debug, Clone, Serialize)]
struct OpenAiChatMessage {
    role: String,
    content: serde_json::Value,
}

impl ChatMessage {
    pub fn system(content: &str) -> Self {
        Self {
            role: "system".to_string(),
            parts: vec![ChatPart::Text(content.to_string())],
        }
    }

    pub fn user_text(content: &str) -> Self {
        Self {
            role: "user".to_string(),
            parts: vec![ChatPart::Text(content.to_string())],
        }
    }

    /// 构造包含图片的用户消息
    /// image_base64: base64 编码的图片数据
    /// media_type: image/png | image/jpeg
    /// text: 文本内容
    pub fn user_image_text(image_base64: &str, media_type: &str, text: &str) -> Self {
        Self {
            role: "user".to_string(),
            parts: vec![
                ChatPart::Image {
                    mime_type: media_type.to_string(),
                    data: image_base64.to_string(),
                },
                ChatPart::Text(text.to_string()),
            ],
        }
    }
}

/// Chat Completions API 响应
#[derive(Debug, Serialize, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

/// LLM Chat 客户端
#[derive(Clone)]
pub struct ChatClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    provider: ChatProvider,
    retry_config: crate::config::RetryConfig,
}

impl ChatClient {
    pub fn new(config: &AppConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: config.llm.chat.base_url.clone(),
            api_key: config.llm.chat.api_key.clone(),
            model: config.llm.chat.model.clone(),
            provider: config.llm.chat.provider,
            retry_config: config.llm.retry.clone(),
        }
    }

    /// 发送 Chat 请求（含自动重试）
    pub async fn chat(&self, messages: Vec<ChatMessage>, temperature: Option<f64>) -> Result<String> {
        let mut last_error = None;

        for attempt in 0..self.retry_config.max_attempts {
            let response = match self.provider {
                ChatProvider::Openai => self.send_openai_request(&messages, temperature).await,
                ChatProvider::Google => self.send_google_request(&messages, temperature).await,
            };

            match response {
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

    async fn send_openai_request(&self, messages: &[ChatMessage], temperature: Option<f64>) -> Result<String> {
        let api_url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let request = ChatRequest {
            model: self.model.clone(),
            messages: messages.iter().cloned().map(to_openai_message).collect(),
            max_tokens: Some(4096),
            temperature,
        };

        tracing::debug!(
            url = %api_url,
            body = %serde_json::to_string(&request).unwrap_or_default(),
            provider = "openai",
            "发送 Chat API 请求"
        );

        let response = self
            .http
            .post(&api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Chat API 网络请求失败")?;

        let status = response.status();
        if status.is_success() {
            let chat_resp: ChatResponse = response
                .json()
                .await
                .context("解析 Chat API 响应失败")?;
            tracing::debug!(
                status = %status,
                body = %serde_json::to_string(&chat_resp).unwrap_or_default(),
                provider = "openai",
                "收到 Chat API 响应"
            );
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
    api_key: String,
    base_url: String,
    model: String,
    dimensions: u32,
    provider: EmbeddingProvider,
    retry_config: crate::config::RetryConfig,
}

/// Google AI Studio embedContent 响应
#[derive(Debug, Serialize, Deserialize)]
struct GoogleEmbedResponse {
    embedding: GoogleEmbedding,
}

#[derive(Debug, Serialize, Deserialize)]
struct GoogleEmbedding {
    values: Vec<f32>,
}

/// Google AI Studio embedContent 请求体中的 content parts 构建辅助
enum ContentPart {
    Text { text: String },
    Image { mime_type: String, data: String },
}

impl ChatClient {
    async fn send_google_request(&self, messages: &[ChatMessage], temperature: Option<f64>) -> Result<String> {
        let api_url = format!(
            "{}/v1beta/models/{}:generateContent",
            self.base_url.trim_end_matches('/'),
            self.model
        );
        let body = build_google_chat_request(messages, temperature);

        tracing::debug!(
            url = %api_url,
            body = %serde_json::to_string(&body).unwrap_or_default(),
            provider = "google",
            "发送 Chat API 请求"
        );

        let response = self
            .http
            .post(&api_url)
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("Google Chat API 网络请求失败")?;

        let status = response.status();
        if status.is_success() {
            let chat_resp: GoogleChatResponse = response
                .json()
                .await
                .context("解析 Google Chat API 响应失败")?;
            tracing::debug!(
                status = %status,
                body = %serde_json::to_string(&chat_resp).unwrap_or_default(),
                provider = "google",
                "收到 Chat API 响应"
            );
            extract_google_chat_text(chat_resp)
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
            api_key: config.llm.embedding.api_key.clone(),
            base_url: config.llm.embedding.base_url.clone(),
            model: config.llm.embedding.model.clone(),
            dimensions: config.llm.embedding.dimensions,
            provider: config.llm.embedding.provider,
            retry_config: config.llm.retry.clone(),
        }
    }

    pub fn provider(&self) -> EmbeddingProvider {
        self.provider
    }

    pub fn supports_image_embedding(&self) -> bool {
        matches!(self.provider, EmbeddingProvider::Google)
    }

    /// 生成纯文本 embedding
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        match self.provider {
            EmbeddingProvider::Google => {
                let parts = vec![ContentPart::Text { text: text.to_string() }];
                self.send_google_embed(parts).await
            }
            EmbeddingProvider::Openai => anyhow::bail!(
                "llm.embedding.provider=openai 暂未实现；当前仅支持 provider=google"
            ),
        }
    }

    /// 多模态 embedding：图片 + 文本
    pub async fn embed_with_image(
        &self,
        image_base64: &str,
        media_type: &str,
        text: &str,
    ) -> Result<Vec<f32>> {
        match self.provider {
            EmbeddingProvider::Google => {
                let mut parts = vec![ContentPart::Image {
                    mime_type: media_type.to_string(),
                    data: image_base64.to_string(),
                }];
                if !text.is_empty() {
                    parts.push(ContentPart::Text { text: text.to_string() });
                }
                self.send_google_embed(parts).await
            }
            EmbeddingProvider::Openai => anyhow::bail!(
                "llm.embedding.provider=openai 目前不支持图片/多模态 embedding；请使用 provider=google"
            ),
        }
    }

    /// 纯图片 embedding（无文本）
    pub async fn embed_image_only(
        &self,
        image_base64: &str,
        media_type: &str,
    ) -> Result<Vec<f32>> {
        match self.provider {
            EmbeddingProvider::Google => {
                let parts = vec![ContentPart::Image {
                    mime_type: media_type.to_string(),
                    data: image_base64.to_string(),
                }];
                self.send_google_embed(parts).await
            }
            EmbeddingProvider::Openai => anyhow::bail!(
                "llm.embedding.provider=openai 目前不支持图片 embedding；请使用 provider=google"
            ),
        }
    }

    /// 构建 Google AI Studio embedContent 请求并发送
    /// POST {api_url}
    /// {
    ///   "model": "models/{model}",
    ///   "content": { "parts": [...] },
    ///   "config": { "output_dimensionality": N }
    /// }
    async fn send_google_embed(&self, parts: Vec<ContentPart>) -> Result<Vec<f32>> {
        let api_url = format!(
            "{}/v1beta/models/{}:embedContent",
            self.base_url.trim_end_matches('/'),
            self.model
        );
        let body = serde_json::json!({
            "model": format!("models/{}", self.model),
            "content": {
                "parts": parts.iter().map(|p| p.to_json()).collect::<Vec<_>>()
            },
            "outputDimensionality": self.dimensions
        });

        let mut last_error = None;

        for attempt in 0..self.retry_config.max_attempts {
            match self.send_google_request(&api_url, &body).await {
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

    async fn send_google_request(&self, api_url: &str, body: &serde_json::Value) -> Result<Vec<f32>> {
        tracing::debug!(
            url = %api_url,
            body = %serde_json::to_string(body).unwrap_or_default(),
            provider = "google",
            "发送 Embedding API 请求"
        );

        let response = self
            .http
            .post(api_url)
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
            tracing::debug!(
                status = %status,
                body = %serde_json::to_string(&resp).unwrap_or_default(),
                provider = "google",
                "收到 Embedding API 响应"
            );
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

fn to_openai_message(message: ChatMessage) -> OpenAiChatMessage {
    let content = if message.parts.len() == 1 {
        match &message.parts[0] {
            ChatPart::Text(text) => serde_json::Value::String(text.clone()),
            ChatPart::Image { mime_type, data } => serde_json::json!([
                {
                    "type": "image_url",
                    "image_url": { "url": format!("data:{};base64,{}", mime_type, data) }
                }
            ]),
        }
    } else {
        serde_json::Value::Array(
            message
                .parts
                .iter()
                .map(|part| match part {
                    ChatPart::Text(text) => serde_json::json!({ "type": "text", "text": text }),
                    ChatPart::Image { mime_type, data } => serde_json::json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:{};base64,{}", mime_type, data) }
                    }),
                })
                .collect(),
        )
    };

    OpenAiChatMessage {
        role: message.role,
        content,
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct GoogleChatResponse {
    #[serde(default)]
    candidates: Vec<GoogleCandidate>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GoogleCandidate {
    #[serde(default)]
    content: Option<GoogleContent>,
    #[serde(rename = "finishReason")]
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GoogleContent {
    #[serde(default)]
    parts: Vec<GooglePart>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GooglePart {
    #[serde(default)]
    text: Option<String>,
}

fn build_google_chat_request(messages: &[ChatMessage], temperature: Option<f64>) -> serde_json::Value {
    let mut system_parts = Vec::new();
    let mut contents = Vec::new();

    for message in messages {
        if message.role == "system" {
            system_parts.extend(message.parts.iter().map(chat_part_to_google_json));
        } else {
            contents.push(serde_json::json!({
                "role": map_google_role(&message.role),
                "parts": message.parts.iter().map(chat_part_to_google_json).collect::<Vec<_>>()
            }));
        }
    }

    let mut body = serde_json::json!({
        "contents": contents,
        "generationConfig": {
            "temperature": temperature.unwrap_or(0.3),
            "maxOutputTokens": 4096
        }
    });

    if !system_parts.is_empty() {
        body["systemInstruction"] = serde_json::json!({ "parts": system_parts });
    }

    body
}

fn chat_part_to_google_json(part: &ChatPart) -> serde_json::Value {
    match part {
        ChatPart::Text(text) => serde_json::json!({ "text": text }),
        ChatPart::Image { mime_type, data } => serde_json::json!({
            "inlineData": {
                "mimeType": mime_type,
                "data": data,
            }
        }),
    }
}

fn map_google_role(role: &str) -> &'static str {
    match role {
        "assistant" => "model",
        _ => "user",
    }
}

fn extract_google_chat_text(resp: GoogleChatResponse) -> Result<String> {
    let candidate = resp
        .candidates
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Google Chat API 返回空响应"))?;

    let text = candidate
        .content
        .map(|content| {
            content
                .parts
                .into_iter()
                .filter_map(|p| p.text)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    if text.trim().is_empty() {
        let reason = candidate.finish_reason.unwrap_or_else(|| "unknown".to_string());
        anyhow::bail!("Google Chat API 未返回文本内容，finish_reason={}", reason);
    }

    Ok(text)
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
