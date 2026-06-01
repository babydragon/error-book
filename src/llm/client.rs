use anyhow::{Context, Result};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::time::sleep;

use crate::config::{AppConfig, ChatProvider, EmbeddingProvider, HttpServiceKind, LlmConfig, RoleKind, ThinkingConfig};

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
    /// Whether to send Connection: close and Accept-Encoding: identity per-request
    connection_close: bool,
    disable_compression: bool,
    /// Effective thinking/reasoning controls (from default `[llm.chat]` config).
    thinking: Option<ThinkingConfig>,
    /// Collapse `system` messages into `user` for OpenAI-compatible providers.
    openai_no_system_role: bool,
    /// When `true`, inject structured JSON output constraints into the request.
    /// For OpenAI: injects `response_format: { "type": "json_object" }`.
    /// For Google: injects `responseMimeType` and optionally `responseSchema`.
    structured_json_output: bool,
    /// Optional JSON schema for structured output (used by Google provider).
    structured_json_schema: Option<serde_json::Value>,
    /// Maximum output tokens for chat/generate requests.
    max_output_tokens: Option<u32>,
}

impl ChatClient {
    pub fn new(config: &AppConfig) -> Self {
        let profile = config.resolve_http_profile(HttpServiceKind::Chat);
        tracing::info!(
            request_timeout_secs = profile.request_timeout.as_secs(),
            connect_timeout_secs = profile.connect_timeout.as_secs(),
            http1_only = profile.http1_only,
            connection_close = profile.connection_close,
            disable_compression = profile.disable_compression,
            user_agent = %profile.user_agent,
            "Chat HTTP client 已构建 (统一 profile)"
        );
        Self {
            http: crate::config::build_client_from_profile(&profile),
            base_url: config.llm.chat.base_url.clone(),
            api_key: config.llm.chat.api_key.clone(),
            model: config.llm.chat.model.clone(),
            provider: config.llm.chat.provider,
            retry_config: config.llm.retry.clone(),
            connection_close: profile.connection_close,
            disable_compression: profile.disable_compression,
            thinking: config.llm.chat.effective_thinking(),
            openai_no_system_role: config.llm.chat.openai_no_system_role.unwrap_or(false),
            structured_json_output: config.llm.chat.structured_json_output.unwrap_or(false),
            structured_json_schema: None,
            max_output_tokens: config.llm.chat.max_output_tokens,
        }
    }

    /// 返回当前 chat 模型名称和 provider 的只读信息（用于 artifact 记录）
    pub fn model_info(&self) -> (&str, &str) {
        let provider_str = match self.provider {
            ChatProvider::Openai => "openai",
            ChatProvider::Google => "google",
        };
        (&self.model, provider_str)
    }

    /// Return (model, provider_str) for a given role resolved from `llm_config`.
    /// Falls back to `self.model_info()` when the role maps to the default chat provider.
    pub fn model_info_for_role(&self, llm_config: &LlmConfig, role: RoleKind) -> (String, String) {
        let resolved = llm_config.resolve_role_provider(role);
        let same_as_default = resolved.base_url == self.base_url
            && resolved.api_key == self.api_key
            && resolved.model == self.model;
        if same_as_default {
            let (m, p) = self.model_info();
            (m.to_string(), p.to_string())
        } else {
            let provider_str = match resolved.provider {
                ChatProvider::Openai => "openai",
                ChatProvider::Google => "google",
            };
            (resolved.model.clone(), provider_str.to_string())
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

    /// Role-aware chat: resolves model/api_key/base_url from `llm_config.resolve_role_provider(role)`,
    /// then reuses the same HTTP client, retry logic, and provider dispatch as `chat()`.
    ///
    /// If the resolved provider uses the same base_url/api_key/model as the default chat config,
    /// this is equivalent to `chat()` — just with per-role temperature/logging.
    pub async fn chat_with_role(
        &self,
        llm_config: &LlmConfig,
        role: RoleKind,
        messages: Vec<ChatMessage>,
        temperature: Option<f64>,
    ) -> Result<String> {
        let resolved = llm_config.resolve_role_provider(role);
        let structured = resolved.structured_json_output;

        // Check if the resolved provider matches the default chat config.
        // If yes, we can just use the existing `chat()` path (same URL/auth).
        let same_as_default = resolved.base_url == self.base_url
            && resolved.api_key == self.api_key
            && resolved.model == self.model;

        if same_as_default {
            tracing::debug!(role = ?role, "Role resolves to default chat provider");

            let mut role_client = self.clone();
            role_client.thinking = resolved.thinking;
            role_client.openai_no_system_role = resolved.openai_no_system_role;
            role_client.structured_json_output = structured;
            role_client.structured_json_schema = None;
            role_client.max_output_tokens = resolved.max_output_tokens;

            if structured && matches!(role, RoleKind::PracticeGeneration | RoleKind::PracticePlanning) {
                role_client.structured_json_schema = Some(match role {
                    RoleKind::PracticePlanning => practice_planning_schema(),
                    _ => practice_generation_schema(),
                });
            }

            return role_client.chat(messages, temperature).await;
        }

        // Different provider: build a temporary client clone with overridden fields
        tracing::info!(
            role = ?role,
            model = %resolved.model,
            base_url = %resolved.base_url,
            provider = ?resolved.provider,
            "Role resolves to dedicated provider"
        );

        let mut role_client = self.clone();
        role_client.base_url = resolved.base_url;
        role_client.api_key = resolved.api_key;
        role_client.model = resolved.model;
        role_client.provider = resolved.provider;
        // Carry the resolved provider's thinking config (or clear it).
        role_client.thinking = resolved.thinking;
        // Carry the resolved provider's compatibility flags.
        role_client.openai_no_system_role = resolved.openai_no_system_role;
        role_client.max_output_tokens = resolved.max_output_tokens;

        // Enable structured JSON output when the resolved provider or role requests it.
        if structured {
            role_client.structured_json_output = true;
            // For practice generation, inject the practice-specific schema.
            if matches!(role, RoleKind::PracticeGeneration | RoleKind::PracticePlanning) {
                role_client.structured_json_schema = Some(match role {
                    RoleKind::PracticePlanning => practice_planning_schema(),
                    _ => practice_generation_schema(),
                });
            }
        }

        role_client.chat(messages, temperature).await
    }

    async fn send_openai_request(&self, messages: &[ChatMessage], temperature: Option<f64>) -> Result<String> {
        let api_url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        // Optionally collapse system messages into the first user message.
        let transformed = if self.openai_no_system_role {
            collapse_system_messages(messages)
        } else {
            messages.to_vec()
        };

        let request = ChatRequest {
            model: self.model.clone(),
            messages: transformed.iter().cloned().map(to_openai_message).collect(),
            max_tokens: Some(self.max_output_tokens.unwrap_or(4096)),
            temperature,
        };

        // Build the JSON body, then optionally inject thinking/reasoning fields.
        let mut body = serde_json::to_value(&request)
            .context("serialising OpenAI chat request")?;

        if let Some(ref tc) = self.thinking {
            if let Some(enabled) = tc.thinking_enabled {
                body["thinking"] = serde_json::json!({
                    "type": if enabled { "enabled" } else { "disabled" }
                });
            }
            if let Some(effort) = tc.normalised_reasoning_effort() {
                body["reasoning_effort"] = serde_json::Value::String(effort);
            }
        }

        // Inject response_format when structured output is enabled.
        if self.structured_json_output {
            body["response_format"] = serde_json::json!({ "type": "json_object" });
        }

        tracing::debug!(
            url = %api_url,
            max_output_tokens = self.max_output_tokens.unwrap_or(4096),
            body = %serde_json::to_string(&body).unwrap_or_default(),
            provider = "openai",
            "发送 Chat API 请求"
        );

        let mut req = self
            .http
            .post(&api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json");

        if self.connection_close {
            req = req.header("Connection", "close");
        }
        if self.disable_compression {
            req = req.header("Accept-Encoding", "identity");
        }

        let start = Instant::now();
        let response = match req.json(&body).send().await {
            Ok(resp) => resp,
            Err(err) => {
                let elapsed = start.elapsed();
                let phase = classify_reqwest_error(&err);
                let chain = format_error_chain(&err);
                tracing::error!(
                    elapsed_ms = elapsed.as_millis() as u64,
                    phase,
                    url = %api_url,
                    model = %self.model,
                    base_url = %self.base_url,
                    provider = "openai",
                    error_chain = %chain,
                    "Chat API 网络请求失败 (send)"
                );
                return Err(err).context("Chat API 网络请求失败");
            }
        };

        let status = response.status();
        if status.is_success() {
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let raw_body = response.text().await.map_err(|err| {
                let elapsed = start.elapsed();
                let phase = classify_reqwest_error(&err);
                let chain = format_error_chain(&err);
                tracing::error!(
                    elapsed_ms = elapsed.as_millis() as u64,
                    phase,
                    url = %api_url,
                    model = %self.model,
                    base_url = %self.base_url,
                    provider = "openai",
                    error_chain = %chain,
                    "Chat API 读取响应 body 失败"
                );
                err
            }).context("读取 Chat API 响应 body 失败")?;
            let body_len = raw_body.len();

            let chat_resp: ChatResponse = match serde_json::from_str(&raw_body) {
                Ok(r) => r,
                Err(parse_err) => {
                    let snippet = truncate_body(&raw_body, BODY_SNIPPET_MAX_LEN);
                    tracing::error!(
                        status = %status,
                        content_type = ?content_type,
                        body_len,
                        body_snippet = %snippet,
                        parse_error = %parse_err,
                        provider = "openai",
                        "解析 Chat API 响应失败: 2xx 但 body 无法反序列化为 ChatResponse"
                    );
                    return Err(LlmError::ParseError {
                        status: status.as_u16(),
                        content_type,
                        body_len,
                        body_snippet: snippet,
                        parse_error: parse_err.to_string(),
                    }
                    .into());
                }
            };

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
    connection_close: bool,
    disable_compression: bool,
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
        let body = build_google_chat_request(
            messages,
            temperature,
            self.structured_json_output,
            self.structured_json_schema.as_ref(),
            self.max_output_tokens,
        );

        tracing::debug!(
            url = %api_url,
            max_output_tokens = self.max_output_tokens.unwrap_or(4096),
            body = %serde_json::to_string(&body).unwrap_or_default(),
            provider = "google",
            "发送 Chat API 请求"
        );

        let mut req = self
            .http
            .post(&api_url)
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json");

        if self.connection_close {
            req = req.header("Connection", "close");
        }
        if self.disable_compression {
            req = req.header("Accept-Encoding", "identity");
        }

        let start = Instant::now();
        let response = match req.json(&body).send().await {
            Ok(resp) => resp,
            Err(err) => {
                let elapsed = start.elapsed();
                let phase = classify_reqwest_error(&err);
                let chain = format_error_chain(&err);
                tracing::error!(
                    elapsed_ms = elapsed.as_millis() as u64,
                    phase,
                    url = %api_url,
                    model = %self.model,
                    base_url = %self.base_url,
                    provider = "google",
                    error_chain = %chain,
                    "Google Chat API 网络请求失败 (send)"
                );
                return Err(err).context("Google Chat API 网络请求失败");
            }
        };

        let status = response.status();
        if status.is_success() {
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let raw_body = response.text().await.map_err(|err| {
                let elapsed = start.elapsed();
                let phase = classify_reqwest_error(&err);
                let chain = format_error_chain(&err);
                tracing::error!(
                    elapsed_ms = elapsed.as_millis() as u64,
                    phase,
                    url = %api_url,
                    model = %self.model,
                    base_url = %self.base_url,
                    provider = "google",
                    error_chain = %chain,
                    "Google Chat API 读取响应 body 失败"
                );
                err
            }).context("读取 Google Chat API 响应 body 失败")?;
            let body_len = raw_body.len();

            let chat_resp: GoogleChatResponse = match serde_json::from_str(&raw_body) {
                Ok(r) => r,
                Err(parse_err) => {
                    let snippet = truncate_body(&raw_body, BODY_SNIPPET_MAX_LEN);
                    tracing::error!(
                        status = %status,
                        content_type = ?content_type,
                        body_len,
                        body_snippet = %snippet,
                        parse_error = %parse_err,
                        provider = "google",
                        "解析 Google Chat API 响应失败: 2xx 但 body 无法反序列化为 GoogleChatResponse"
                    );
                    return Err(LlmError::ParseError {
                        status: status.as_u16(),
                        content_type,
                        body_len,
                        body_snippet: snippet,
                        parse_error: parse_err.to_string(),
                    }
                    .into());
                }
            };

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
        let profile = config.resolve_http_profile(HttpServiceKind::Embedding);
        tracing::info!(
            request_timeout_secs = profile.request_timeout.as_secs(),
            connect_timeout_secs = profile.connect_timeout.as_secs(),
            http1_only = profile.http1_only,
            connection_close = profile.connection_close,
            user_agent = %profile.user_agent,
            "Embedding HTTP client 已构建 (统一 profile)"
        );

        // Resolve via role/provider system so that llm.roles.embedding → named provider
        // takes precedence; otherwise falls back to legacy [llm.embedding] config.
        let resolved = config.llm.resolve_role_provider(RoleKind::Embedding);

        // Detect whether the resolution came from a dedicated role binding or legacy fallback.
        let is_role_provider = config.llm.roles.embedding.as_ref()
            .and_then(|name| config.llm.providers.get(name))
            .is_some();

        if is_role_provider {
            tracing::info!(
                model = %resolved.model,
                base_url = %resolved.base_url,
                provider = ?resolved.provider,
                "Embedding resolved to dedicated role provider (llm.roles.embedding)"
            );
        } else {
            tracing::info!(
                model = %resolved.model,
                base_url = %resolved.base_url,
                provider = ?resolved.provider,
                "Embedding using legacy [llm.embedding] config"
            );
        }

        // Map ChatProvider (from ResolvedProvider) → EmbeddingProvider
        let embedding_provider = match resolved.provider {
            ChatProvider::Openai => EmbeddingProvider::Openai,
            ChatProvider::Google => EmbeddingProvider::Google,
        };

        Self {
            http: crate::config::build_client_from_profile(&profile),
            api_key: resolved.api_key,
            base_url: resolved.base_url,
            model: resolved.model,
            dimensions: config.llm.embedding.dimensions,
            provider: embedding_provider,
            retry_config: config.llm.retry.clone(),
            connection_close: profile.connection_close,
            disable_compression: profile.disable_compression,
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

        let mut req = self
            .http
            .post(api_url)
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json");

        if self.connection_close {
            req = req.header("Connection", "close");
        }
        if self.disable_compression {
            req = req.header("Accept-Encoding", "identity");
        }

        let response = req
            .json(body)
            .send()
            .await
            .context("Google Embedding API 网络请求失败")?;

        let status = response.status();
        if status.is_success() {
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let raw_body = response.text().await.context("读取 Google Embedding 响应 body 失败")?;
            let body_len = raw_body.len();

            let resp: GoogleEmbedResponse = match serde_json::from_str(&raw_body) {
                Ok(r) => r,
                Err(parse_err) => {
                    let snippet = truncate_body(&raw_body, BODY_SNIPPET_MAX_LEN);
                    tracing::error!(
                        status = %status,
                        content_type = ?content_type,
                        body_len,
                        body_snippet = %snippet,
                        parse_error = %parse_err,
                        provider = "google",
                        "解析 Google Embedding 响应失败: 2xx 但 body 无法反序列化为 GoogleEmbedResponse"
                    );
                    return Err(LlmError::ParseError {
                        status: status.as_u16(),
                        content_type,
                        body_len,
                        body_snippet: snippet,
                        parse_error: parse_err.to_string(),
                    }
                    .into());
                }
            };

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

/// Collapse all `system` messages into the first `user` message for providers
/// that do not support the `system` role.
///
/// Algorithm:
/// 1. Collect the text content of all system messages.
/// 2. If system text exists, prepend it as an instruction block to the first
///    user message. If there is no user message, create a new one.
/// 3. All non-system messages keep their original order.
fn collapse_system_messages(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    // Collect system text
    let system_text: String = messages
        .iter()
        .filter(|m| m.role == "system")
        .map(|m| {
            m.parts.iter().filter_map(|p| match p {
                ChatPart::Text(t) => Some(t.as_str()),
                _ => None,
            }).collect::<Vec<_>>().join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n");

    if system_text.is_empty() {
        return messages.to_vec();
    }

    let prefix = format!("[System Instructions]\n{}\n[End System Instructions]", system_text);

    let mut result = Vec::new();
    let mut first_user_patched = false;

    for msg in messages {
        if msg.role == "system" {
            // Skip system messages; their content is folded into prefix.
            continue;
        }
        if msg.role == "user" && !first_user_patched {
            // Prepend system text to the first user message.
            let mut new_parts = Vec::new();
            new_parts.push(ChatPart::Text(prefix.clone()));
            new_parts.extend(msg.parts.iter().cloned());
            result.push(ChatMessage {
                role: "user".to_string(),
                parts: new_parts,
            });
            first_user_patched = true;
        } else {
            result.push(msg.clone());
        }
    }

    // If there was no user message at all, create one from system text.
    if !first_user_patched {
        result.insert(0, ChatMessage::user_text(&prefix));
    }

    result
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

fn build_google_chat_request(
    messages: &[ChatMessage],
    temperature: Option<f64>,
    structured_output: bool,
    structured_schema: Option<&serde_json::Value>,
    max_output_tokens: Option<u32>,
) -> serde_json::Value {
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

    let mut generation_config = serde_json::json!({
        "temperature": temperature.unwrap_or(0.3),
        "maxOutputTokens": max_output_tokens.unwrap_or(4096)
    });

    if structured_output {
        generation_config["responseMimeType"] = serde_json::json!("application/json");
        if let Some(schema) = structured_schema {
            generation_config["responseSchema"] = schema.clone();
        }
    }

    let mut body = serde_json::json!({
        "contents": contents,
        "generationConfig": generation_config
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

    let finish_reason = candidate.finish_reason.clone();

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

    if let Some(reason) = finish_reason.as_deref() {
        // Useful for diagnosing partial/truncated outputs from provider-side cutoffs.
        if matches!(reason, "MAX_TOKENS" | "RECITATION" | "SAFETY") {
            tracing::warn!(
                finish_reason = %reason,
                text_len = text.chars().count(),
                "Google Chat API finishReason indicates output may be incomplete"
            );
        } else {
            tracing::debug!(
                finish_reason = %reason,
                text_len = text.chars().count(),
                "Google Chat API finishReason"
            );
        }
    }

    if text.trim().is_empty() {
        let reason = finish_reason.unwrap_or_else(|| "unknown".to_string());
        anyhow::bail!("Google Chat API 未返回文本内容，finish_reason={}", reason);
    }

    Ok(text)
}

/// Maximum number of characters to include in a logged/emitted body snippet.
const BODY_SNIPPET_MAX_LEN: usize = 500;

/// Truncate a string to at most `max_len` Unicode characters, appending "…" when truncated.
/// UTF-8 safe — never panics on multi-byte characters.
fn truncate_body(body: &str, max_len: usize) -> String {
    if body.chars().count() <= max_len {
        body.to_string()
    } else {
        let truncated: String = body.chars().take(max_len).collect();
        format!("{}…", truncated)
    }
}

/// Classify a [`reqwest::Error`] into a human-readable phase string for log fields.
fn classify_reqwest_error(err: &reqwest::Error) -> &'static str {
    if err.is_timeout() {
        "timeout"
    } else if err.is_connect() {
        "connect"
    } else if err.is_body() {
        "body"
    } else if err.is_decode() {
        "decode"
    } else if err.is_redirect() {
        "redirect"
    } else if err.is_request() {
        "request"
    } else {
        "unknown"
    }
}

/// Walk the `std::error::Error::source()` chain and format as `"msg → cause → cause …"`.
fn format_error_chain(err: &dyn std::error::Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(s) = source {
        parts.push(s.to_string());
        source = s.source();
    }
    parts.join(" → ")
}

/// JSON schema for practice generation structured output.
///
/// Constraints:
/// - Top-level object with a `questions` array
/// - Each element must have `question` (string), `answer` (string), `knowledge_points` (string array)
pub fn practice_generation_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "questions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "question": {
                            "type": "string"
                        },
                        "answer": {
                            "type": "string"
                        },
                        "knowledge_points": {
                            "type": "array",
                            "items": {
                                "type": "string"
                            }
                        }
                    },
                    "required": ["question", "answer", "knowledge_points"]
                }
            }
        },
        "required": ["questions"]
    })
}

pub fn practice_planning_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "subject": { "type": "string" },
            "grade_level": { "type": "string" },
            "count": { "type": "integer" },
            "rationale": { "type": "string" },
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "index": { "type": "integer" },
                        "primary_point": { "type": "string" },
                        "secondary_points": { "type": "array", "items": { "type": "string" } },
                        "question_form": { "type": "string" },
                        "material_type": { "type": "string" },
                        "target_reference_ids": { "type": "array", "items": { "type": "string" } },
                        "avoid_topics": { "type": "array", "items": { "type": "string" } },
                        "requires_image": { "type": "boolean" },
                        "image_role": { "type": "string" },
                        "image_type": { "type": "string" },
                        "dependency_mode": { "type": "string" },
                        "image_spec": { "type": ["object", "null"] }
                    },
                    "required": ["index", "primary_point", "secondary_points", "question_form", "material_type", "target_reference_ids", "avoid_topics", "requires_image", "image_role", "image_type", "dependency_mode"]
                }
            }
        },
        "required": ["subject", "grade_level", "count", "items"]
    })
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
    #[error(
        "响应解析失败 (status={status}, content_type={content_type:?}, body_len={body_len}): {parse_error} — body 片段: {body_snippet}"
    )]
    ParseError {
        status: u16,
        content_type: Option<String>,
        body_len: usize,
        body_snippet: String,
        parse_error: String,
    },
}

impl LlmError {
    fn is_retryable(&self) -> bool {
        match self {
            LlmError::ApiError { retryable, .. } => *retryable,
            // Parse failures on 2xx are unlikely to be transient; don't retry.
            LlmError::ParseError { .. } => false,
        }
    }
}
