use anyhow::{Context, Result};
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

// ── Thinking / reasoning controls for OpenAI-compatible providers ────

/// Optional thinking/reasoning controls for providers that support them
/// (e.g. DeepSeek V4 Pro `thinking` / `reasoning_effort`).
///
/// These fields are only injected into OpenAI-compatible request bodies
/// when the provider protocol is `openai`; Google-native requests ignore them.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct ThinkingConfig {
    /// When `Some(true)`, sends `thinking: { type: "enabled" }` in the request.
    /// When `Some(false)`, sends `thinking: { type: "disabled" }`.
    /// When `None` (default), omits the field entirely.
    #[serde(default)]
    pub thinking_enabled: Option<bool>,

    /// When set, sends `reasoning_effort: "<value>"` in the request.
    /// Accepted values: `low`, `medium`, `high`, `xhigh`, `max`.
    /// Aliases: `low`/`medium` → normalised to `high`; `xhigh` → `max`.
    /// When `None` (default), omits the field entirely.
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

impl ThinkingConfig {
    /// Normalise `reasoning_effort` aliases to the canonical values (`high` / `max`).
    pub fn normalised_reasoning_effort(&self) -> Option<String> {
        self.reasoning_effort
            .as_ref()
            .map(|v| match v.to_ascii_lowercase().as_str() {
                "low" | "medium" => "high".to_string(),
                "xhigh" => "max".to_string(),
                other => other.to_string(),
            })
    }

    /// Returns true if either field is actually set (non-trivial config).
    pub fn is_active(&self) -> bool {
        self.thinking_enabled.is_some() || self.reasoning_effort.is_some()
    }
}

// ── Unified HTTP profile types ──────────────────────────────────────

/// Identifies which service is requesting an HTTP client profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpServiceKind {
    Chat,
    Embedding,
    Image,
    ImageDownload,
}

/// Resolved runtime HTTP profile (defaults + override + legacy fallback).
#[derive(Debug, Clone)]
pub struct HttpProfile {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub http1_only: bool,
    pub connection_close: bool,
    pub disable_compression: bool,
    pub tcp_keepalive: Duration,
    pub tcp_nodelay: bool,
    pub user_agent: String,
}

// ── Unified HTTP config model ───────────────────────────────────────

/// Shared transport defaults for all HTTP clients.
#[derive(Debug, Deserialize, Clone)]
pub struct HttpDefaultsConfig {
    #[serde(default = "default_http_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_http_http1_only")]
    pub http1_only: bool,
    #[serde(default = "default_http_connection_close")]
    pub connection_close: bool,
    #[serde(default = "default_http_disable_compression")]
    pub disable_compression: bool,
    #[serde(default = "default_http_tcp_keepalive_secs")]
    pub tcp_keepalive_secs: u64,
    #[serde(default = "default_http_tcp_nodelay")]
    pub tcp_nodelay: bool,
    #[serde(default = "default_http_user_agent")]
    pub user_agent: String,
}

impl Default for HttpDefaultsConfig {
    fn default() -> Self {
        Self {
            connect_timeout_secs: default_http_connect_timeout_secs(),
            http1_only: default_http_http1_only(),
            connection_close: default_http_connection_close(),
            disable_compression: default_http_disable_compression(),
            tcp_keepalive_secs: default_http_tcp_keepalive_secs(),
            tcp_nodelay: default_http_tcp_nodelay(),
            user_agent: default_http_user_agent(),
        }
    }
}

/// Per-service HTTP override (only request timeout for simplicity).
#[derive(Debug, Deserialize, Clone, Default)]
pub struct HttpServiceOverride {
    #[serde(default)]
    pub request_timeout_secs: Option<u64>,
}

/// Top-level `[llm.http]` section.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct HttpConfig {
    #[serde(default)]
    pub defaults: HttpDefaultsConfig,
    #[serde(default)]
    pub chat: HttpServiceOverride,
    #[serde(default)]
    pub embedding: HttpServiceOverride,
    #[serde(default)]
    pub image: HttpServiceOverride,
    #[serde(default)]
    pub image_download: HttpServiceOverride,
}

// ── HTTP default helpers ────────────────────────────────────────────

fn default_http_connect_timeout_secs() -> u64 {
    30
}
fn default_http_http1_only() -> bool {
    true
}
fn default_http_connection_close() -> bool {
    true
}
fn default_http_disable_compression() -> bool {
    true
}
fn default_http_tcp_keepalive_secs() -> u64 {
    30
}
fn default_http_tcp_nodelay() -> bool {
    true
}
fn default_http_user_agent() -> String {
    "error-book/0.1".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub llm: LlmConfig,
    pub database: DatabaseConfig,
    pub storage: StorageConfig,
    #[serde(default)]
    pub defaults: DefaultsConfig,
    pub pdf: PdfConfig,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

/// Chat LLM provider configuration (independent from embedding).
///
/// All fields have serde defaults, so `[llm.chat]` can be omitted entirely
/// when chat-capable roles are configured via `[llm.providers.*]` + `[llm.roles]`.
/// In that case it serves only as a fallback.
#[derive(Deserialize, Clone)]
pub struct ChatProviderConfig {
    #[serde(default)]
    pub provider: ChatProvider,
    #[serde(default)]
    pub base_url: String,
    #[serde(skip_serializing)]
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    /// Optional nested thinking/reasoning controls (DeepSeek V4 Pro, etc.).
    #[serde(default)]
    pub thinking: Option<ThinkingConfig>,
    /// Flat thinking fields – alternative to nested `[llm.chat.thinking]`.
    /// Used as fallback when `thinking` is not configured.
    #[serde(default)]
    pub thinking_enabled: Option<bool>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// When `true`, collapse all `system` role messages into the first `user`
    /// message before sending to an OpenAI-compatible provider. Useful for
    /// relays that reject the `system` role (e.g. some DeepSeek V4 Pro proxies).
    #[serde(default)]
    pub openai_no_system_role: Option<bool>,
    /// When `true`, enable structured JSON output for both OpenAI and Google providers.
    /// For OpenAI: injects `response_format: { "type": "json_object" }`.
    /// For Google: injects `responseMimeType: "application/json"` and optional schema.
    #[serde(default)]
    pub structured_json_output: Option<bool>,
    /// Maximum output tokens for chat/generate requests.
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
}

impl Default for ChatProviderConfig {
    fn default() -> Self {
        Self {
            provider: ChatProvider::default(),
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            thinking: None,
            thinking_enabled: None,
            reasoning_effort: None,
            openai_no_system_role: None,
            structured_json_output: None,
            max_output_tokens: None,
        }
    }
}

impl ChatProviderConfig {
    /// Return the effective thinking config, merging flat fields with nested style.
    /// Nested `thinking` takes precedence; flat fields are used as fallback.
    pub fn effective_thinking(&self) -> Option<ThinkingConfig> {
        if let Some(ref nested) = self.thinking {
            return Some(nested.clone());
        }
        if self.thinking_enabled.is_some() || self.reasoning_effort.is_some() {
            return Some(ThinkingConfig {
                thinking_enabled: self.thinking_enabled,
                reasoning_effort: self.reasoning_effort.clone(),
            });
        }
        None
    }
}

impl fmt::Debug for ChatProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatProviderConfig")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("thinking", &self.thinking)
            .field("thinking_enabled", &self.thinking_enabled)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("openai_no_system_role", &self.openai_no_system_role)
            .field("structured_json_output", &self.structured_json_output)
            .field("max_output_tokens", &self.max_output_tokens)
            .finish()
    }
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChatProvider {
    #[default]
    Openai,
    Google,
}

/// Embedding LLM provider configuration (independent from chat).
///
/// All fields have serde defaults, so `[llm.embedding]` can be omitted or
/// contain only `dimensions` when the embedding role is bound to a named
/// provider via `[llm.roles]`.
#[derive(Deserialize, Clone)]
pub struct EmbeddingProviderConfig {
    #[serde(default)]
    pub provider: EmbeddingProvider,
    #[serde(default)]
    pub base_url: String,
    #[serde(skip_serializing)]
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_embedding_dimensions")]
    pub dimensions: u32,
}

impl Default for EmbeddingProviderConfig {
    fn default() -> Self {
        Self {
            provider: EmbeddingProvider::default(),
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            dimensions: default_embedding_dimensions(),
        }
    }
}

impl fmt::Debug for EmbeddingProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddingProviderConfig")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("dimensions", &self.dimensions)
            .finish()
    }
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingProvider {
    Openai,
    #[default]
    Google,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LlmConfig {
    /// Legacy chat config – fallback-only when all chat-capable roles are
    /// bound to named providers.  Can be omitted entirely.
    #[serde(default)]
    pub chat: ChatProviderConfig,
    /// Legacy embedding config – may contain only `dimensions` when the
    /// embedding role is bound to a named provider.  Provider fields
    /// (`base_url` / `api_key` / `model`) are fallback-only.
    #[serde(default)]
    pub embedding: EmbeddingProviderConfig,
    /// Legacy image config – may contain only `mime_type` / `aspect_ratio`
    /// / timeout extras when image_generation role is bound to a named
    /// provider.  Provider fields are fallback-only.
    #[serde(default)]
    pub image: Option<ImageProviderConfig>,
    #[serde(default)]
    pub retry: RetryConfig,
    /// Unified HTTP transport + per-service timeout config.
    #[serde(default)]
    pub http: HttpConfig,
    /// Named providers for role-based resolution.
    #[serde(default)]
    pub providers: std::collections::HashMap<String, NamedProviderConfig>,
    /// Role → provider-name bindings.
    #[serde(default)]
    pub roles: RoleBindings,
}

/// Image generation provider configuration.
///
/// All fields have serde defaults, so `[llm.image]` can be omitted or
/// contain only image-specific extras (`mime_type`, `aspect_ratio`, timeouts)
/// when image_generation role is bound to a named provider.
#[derive(Deserialize, Clone)]
pub struct ImageProviderConfig {
    #[serde(default)]
    pub provider: ImageProvider,
    #[serde(default)]
    pub base_url: String,
    #[serde(skip_serializing)]
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_image_mime_type")]
    pub mime_type: String,
    #[serde(default = "default_image_aspect_ratio")]
    pub aspect_ratio: String,
    /// TCP 连接超时（秒），默认 30s
    #[serde(default = "default_image_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    /// 图片生成请求整体超时（秒），默认 600s（图片生成耗时长，远高于普通请求）
    #[serde(default = "default_image_request_timeout_secs")]
    pub request_timeout_secs: u64,
    /// 下载已生成图片的超时（秒），默认 120s
    #[serde(default = "default_image_download_timeout_secs")]
    pub download_timeout_secs: u64,
}

impl Default for ImageProviderConfig {
    fn default() -> Self {
        Self {
            provider: ImageProvider::default(),
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            mime_type: default_image_mime_type(),
            aspect_ratio: default_image_aspect_ratio(),
            connect_timeout_secs: default_image_connect_timeout_secs(),
            request_timeout_secs: default_image_request_timeout_secs(),
            download_timeout_secs: default_image_download_timeout_secs(),
        }
    }
}

impl fmt::Debug for ImageProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageProviderConfig")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("mime_type", &self.mime_type)
            .field("aspect_ratio", &self.aspect_ratio)
            .field("connect_timeout_secs", &self.connect_timeout_secs)
            .field("request_timeout_secs", &self.request_timeout_secs)
            .field("download_timeout_secs", &self.download_timeout_secs)
            .finish()
    }
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ImageProvider {
    Openai,
    #[default]
    Google,
}

// ── Role-based provider configuration (Phase 1 foundation) ──────────

/// A named provider profile under `[llm.providers.<name>]`.
#[derive(Deserialize, Clone)]
pub struct NamedProviderConfig {
    #[serde(default)]
    pub provider: ChatProvider,
    pub base_url: String,
    #[serde(skip_serializing)]
    pub api_key: String,
    pub model: String,
    /// Optional nested thinking/reasoning controls (DeepSeek V4 Pro, etc.).
    #[serde(default)]
    pub thinking: Option<ThinkingConfig>,
    /// Flat thinking fields – alternative to nested `[llm.providers.*.thinking]`.
    /// Used as fallback when `thinking` is not configured.
    #[serde(default)]
    pub thinking_enabled: Option<bool>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// When `true`, collapse all `system` role messages into the first `user`
    /// message before sending to an OpenAI-compatible provider.
    #[serde(default)]
    pub openai_no_system_role: Option<bool>,
    /// When `true`, enable structured JSON output for both OpenAI and Google providers.
    /// For OpenAI: injects `response_format: { "type": "json_object" }`.
    /// For Google: injects `responseMimeType: "application/json"` and optional schema.
    #[serde(default)]
    pub structured_json_output: Option<bool>,
    /// Maximum output tokens for chat/generate requests.
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
}

impl NamedProviderConfig {
    /// Return the effective thinking config, merging flat fields with nested style.
    /// Nested `thinking` takes precedence; flat fields are used as fallback.
    pub fn effective_thinking(&self) -> Option<ThinkingConfig> {
        if let Some(ref nested) = self.thinking {
            return Some(nested.clone());
        }
        if self.thinking_enabled.is_some() || self.reasoning_effort.is_some() {
            return Some(ThinkingConfig {
                thinking_enabled: self.thinking_enabled,
                reasoning_effort: self.reasoning_effort.clone(),
            });
        }
        None
    }
}

impl fmt::Debug for NamedProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NamedProviderConfig")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("thinking", &self.thinking)
            .field("thinking_enabled", &self.thinking_enabled)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("openai_no_system_role", &self.openai_no_system_role)
            .field("structured_json_output", &self.structured_json_output)
            .field("max_output_tokens", &self.max_output_tokens)
            .finish()
    }
}

/// Role → provider-name bindings under `[llm.roles]`.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct RoleBindings {
    #[serde(default)]
    pub vision_recognition: Option<String>,
    #[serde(default)]
    pub structured_extraction: Option<String>,
    #[serde(default)]
    pub pedagogical_analysis: Option<String>,
    #[serde(default)]
    pub summary_synthesis: Option<String>,
    #[serde(default)]
    pub infographic_planning: Option<String>,
    #[serde(default)]
    pub practice_planning: Option<String>,
    #[serde(default)]
    pub practice_generation: Option<String>,
    #[serde(default)]
    pub image_generation: Option<String>,
    #[serde(default)]
    pub embedding: Option<String>,
}

/// Well-known role names used across the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoleKind {
    VisionRecognition,
    StructuredExtraction,
    PedagogicalAnalysis,
    SummarySynthesis,
    InfographicPlanning,
    PracticePlanning,
    PracticeGeneration,
    ImageGeneration,
    Embedding,
}

/// Resolved provider profile returned by `resolve_role_provider`.
#[derive(Debug, Clone)]
pub struct ResolvedProvider {
    pub provider: ChatProvider,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// Effective thinking/reasoning controls (from named provider or legacy `[llm.chat]`).
    pub thinking: Option<ThinkingConfig>,
    /// Collapse `system` messages into `user` for OpenAI-compatible requests.
    pub openai_no_system_role: bool,
    /// Enable structured JSON output (provider-agnostic).
    /// For OpenAI: injects `response_format: { "type": "json_object" }`.
    /// For Google: injects `responseMimeType: "application/json"` and optional schema.
    pub structured_json_output: bool,
    /// Maximum output tokens for chat/generate requests.
    pub max_output_tokens: Option<u32>,
}

impl LlmConfig {
    /// Resolve a role to a concrete provider profile.
    ///
    /// Resolution order:
    /// 1. If `llm.roles.<role>` is set and maps to a named provider in `llm.providers`, use it.
    /// 2. Otherwise, fall back to the legacy provider:
    ///    - `Embedding` → `llm.embedding`
    ///    - `ImageGeneration` → `llm.image` (error if missing)
    ///    - everything else → `llm.chat`
    pub fn resolve_role_provider(&self, role: RoleKind) -> ResolvedProvider {
        let provider_name = match role {
            RoleKind::VisionRecognition => self.roles.vision_recognition.as_deref(),
            RoleKind::StructuredExtraction => self.roles.structured_extraction.as_deref(),
            RoleKind::PedagogicalAnalysis => self.roles.pedagogical_analysis.as_deref(),
            RoleKind::SummarySynthesis => self.roles.summary_synthesis.as_deref(),
            RoleKind::InfographicPlanning => self.roles.infographic_planning.as_deref(),
            RoleKind::PracticePlanning => self.roles.practice_planning.as_deref(),
            RoleKind::PracticeGeneration => self.roles.practice_generation.as_deref(),
            RoleKind::ImageGeneration => self.roles.image_generation.as_deref(),
            RoleKind::Embedding => self.roles.embedding.as_deref(),
        };

        if let Some(name) = provider_name {
            if let Some(provider) = self.providers.get(name) {
                return ResolvedProvider {
                    provider: provider.provider,
                    base_url: provider.base_url.clone(),
                    api_key: provider.api_key.clone(),
                    model: provider.model.clone(),
                    thinking: provider.effective_thinking(),
                    openai_no_system_role: provider.openai_no_system_role.unwrap_or(false),
                    structured_json_output: provider.structured_json_output.unwrap_or(false),
                    max_output_tokens: provider.max_output_tokens,
                };
            }
            tracing::warn!(
                role = ?role,
                provider_name = %name,
                "Role mapped to unknown provider, falling back to legacy config"
            );
        }

        // Legacy fallback
        match role {
            RoleKind::Embedding => ResolvedProvider {
                provider: match self.embedding.provider {
                    EmbeddingProvider::Openai => ChatProvider::Openai,
                    EmbeddingProvider::Google => ChatProvider::Google,
                },
                base_url: self.embedding.base_url.clone(),
                api_key: self.embedding.api_key.clone(),
                model: self.embedding.model.clone(),
                thinking: None,
                openai_no_system_role: false,
                structured_json_output: false,
                max_output_tokens: None,
            },
            RoleKind::ImageGeneration => match &self.image {
                Some(img) => ResolvedProvider {
                    provider: match img.provider {
                        ImageProvider::Openai => ChatProvider::Openai,
                        ImageProvider::Google => ChatProvider::Google,
                    },
                    base_url: img.base_url.clone(),
                    api_key: img.api_key.clone(),
                    model: img.model.clone(),
                    thinking: None,
                    openai_no_system_role: false,
                    structured_json_output: false,
                    max_output_tokens: None,
                },
                None => ResolvedProvider {
                    provider: self.chat.provider,
                    base_url: self.chat.base_url.clone(),
                    api_key: self.chat.api_key.clone(),
                    model: self.chat.model.clone(),
                    thinking: self.chat.effective_thinking(),
                    openai_no_system_role: self.chat.openai_no_system_role.unwrap_or(false),
                    structured_json_output: self.chat.structured_json_output.unwrap_or(false),
                    max_output_tokens: self.chat.max_output_tokens,
                },
            },
            _ => ResolvedProvider {
                provider: self.chat.provider,
                base_url: self.chat.base_url.clone(),
                api_key: self.chat.api_key.clone(),
                model: self.chat.model.clone(),
                thinking: self.chat.effective_thinking(),
                openai_no_system_role: self.chat.openai_no_system_role.unwrap_or(false),
                structured_json_output: self.chat.structured_json_output.unwrap_or(false),
                max_output_tokens: self.chat.max_output_tokens,
            },
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct RetryConfig {
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_base_delay_ms")]
    pub base_delay_ms: u64,
    #[serde(default = "default_max_delay_ms")]
    pub max_delay_ms: u64,
    #[serde(default = "default_retryable_codes")]
    pub retryable_status_codes: Vec<u16>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            base_delay_ms: default_base_delay_ms(),
            max_delay_ms: default_max_delay_ms(),
            retryable_status_codes: default_retryable_codes(),
        }
    }
}

impl RetryConfig {
    pub fn base_delay(&self) -> Duration {
        Duration::from_millis(self.base_delay_ms)
    }

    pub fn max_delay(&self) -> Duration {
        Duration::from_millis(self.max_delay_ms)
    }

    pub fn is_retryable(&self, status: u16) -> bool {
        self.retryable_status_codes.contains(&status)
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub url: String,
    pub auth_token: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StorageConfig {
    pub image_dir: PathBuf,
    pub pdf_dir: PathBuf,
    #[serde(default = "default_generated_image_dir")]
    pub generated_image_dir: PathBuf,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DefaultsConfig {
    #[serde(default = "default_grade_level")]
    pub grade_level: String,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            grade_level: default_grade_level(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct PdfConfig {
    pub font_path: PathBuf,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SearchConfig {
    /// 混合搜索时图片 embedding 的权重 (0.0~1.0)
    /// 文本权重 = 1.0 - image_weight
    /// 默认 0.3，即文本为主
    #[serde(default = "default_image_weight")]
    pub image_weight: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub file: Option<PathBuf>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file: None,
        }
    }
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            image_weight: default_image_weight(),
        }
    }
}

fn default_embedding_dimensions() -> u32 {
    1536
}
fn default_max_attempts() -> u32 {
    5
}
fn default_base_delay_ms() -> u64 {
    500
}
fn default_max_delay_ms() -> u64 {
    30000
}
fn default_retryable_codes() -> Vec<u16> {
    vec![429, 500, 502, 503, 504]
}
fn default_grade_level() -> String {
    "二年级".to_string()
}
fn default_generated_image_dir() -> PathBuf {
    PathBuf::from("./data/generated-images")
}
fn default_image_weight() -> f64 {
    0.3
}
fn default_log_level() -> String {
    "info".to_string()
}
fn default_image_mime_type() -> String {
    "image/png".to_string()
}
fn default_image_aspect_ratio() -> String {
    "3:4".to_string()
}
fn default_image_connect_timeout_secs() -> u64 {
    30
}
fn default_image_request_timeout_secs() -> u64 {
    600
}
fn default_image_download_timeout_secs() -> u64 {
    120
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("读取配置文件失败: {}", path.display()))?;
        let config: AppConfig = toml::from_str(&content).with_context(|| "解析配置文件失败")?;

        // 环境变量覆盖
        let mut config = config;
        if let Ok(v) = std::env::var("ERROR_BOOK_LLM_API_KEY") {
            config.llm.chat.api_key = v.clone();
            config.llm.embedding.api_key = v.clone();
            if let Some(image) = config.llm.image.as_mut() {
                image.api_key = v;
            }
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_IMAGE_API_KEY") {
            if let Some(image) = config.llm.image.as_mut() {
                image.api_key = v;
            }
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_CHAT_API_KEY") {
            config.llm.chat.api_key = v;
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_EMBEDDING_API_KEY") {
            config.llm.embedding.api_key = v;
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_LLM_BASE_URL") {
            config.llm.chat.base_url = v.clone();
            config.llm.embedding.base_url = v.clone();
            if let Some(image) = config.llm.image.as_mut() {
                image.base_url = v;
            }
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_IMAGE_BASE_URL") {
            if let Some(image) = config.llm.image.as_mut() {
                image.base_url = v;
            }
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_CHAT_BASE_URL") {
            config.llm.chat.base_url = v;
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_CHAT_PROVIDER") {
            config.llm.chat.provider = match v.to_ascii_lowercase().as_str() {
                "google" => ChatProvider::Google,
                "openai" => ChatProvider::Openai,
                other => anyhow::bail!("不支持的 chat provider: {}，仅支持 google/openai", other),
            };
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_EMBEDDING_BASE_URL") {
            config.llm.embedding.base_url = v;
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_EMBEDDING_PROVIDER") {
            config.llm.embedding.provider = match v.to_ascii_lowercase().as_str() {
                "google" => EmbeddingProvider::Google,
                "openai" => EmbeddingProvider::Openai,
                other => anyhow::bail!(
                    "不支持的 embedding provider: {}，仅支持 google/openai",
                    other
                ),
            };
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_IMAGE_PROVIDER") {
            let provider = match v.to_ascii_lowercase().as_str() {
                "google" => ImageProvider::Google,
                "openai" => ImageProvider::Openai,
                other => anyhow::bail!("不支持的 image provider: {}，仅支持 google/openai", other),
            };
            if let Some(image) = config.llm.image.as_mut() {
                image.provider = provider;
            }
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_IMAGE_MODEL") {
            if let Some(image) = config.llm.image.as_mut() {
                image.model = v;
            }
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_DB_URL") {
            config.database.url = v;
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_LOG_LEVEL") {
            config.logging.level = v;
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_LOG_FILE") {
            config.logging.file = Some(PathBuf::from(v));
        }

        config.resolve_paths(path)?;
        config.validate()?;

        Ok(config)
    }

    pub fn chat_api_url(&self) -> String {
        format!(
            "{}/chat/completions",
            self.llm.chat.base_url.trim_end_matches('/')
        )
    }

    pub fn embeddings_api_url(&self) -> String {
        format!(
            "{}/embeddings",
            self.llm.embedding.base_url.trim_end_matches('/')
        )
    }

    pub fn image_api_url(&self) -> Option<String> {
        self.llm.image.as_ref().map(|image| {
            let base = image.base_url.trim_end_matches('/');
            match image.provider {
                ImageProvider::Openai => format!("{}/images/generations", base),
                ImageProvider::Google => {
                    if image.model.starts_with("gemini-") {
                        format!("{}/v1beta/models/{}:generateContent", base, image.model)
                    } else {
                        format!("{}/v1beta/models/{}:predict", base, image.model)
                    }
                }
            }
        })
    }

    /// Build image generation API URL from a resolved provider profile.
    ///
    /// This is the role-aware counterpart to `image_api_url()`: instead of reading
    /// from the legacy `[llm.image]` section directly, it accepts the resolved
    /// `ChatProvider`, `base_url`, and `model` (which may come from
    /// `resolve_role_provider(RoleKind::ImageGeneration)`).
    pub fn build_image_api_url_from_provider(
        provider: ChatProvider,
        base_url: &str,
        model: &str,
    ) -> String {
        let base = base_url.trim_end_matches('/');
        match provider {
            ChatProvider::Openai => format!("{}/images/generations", base),
            ChatProvider::Google => {
                if model.starts_with("gemini-") {
                    format!("{}/v1beta/models/{}:generateContent", base, model)
                } else {
                    format!("{}/v1beta/models/{}:predict", base, model)
                }
            }
        }
    }

    /// 确保存储目录存在
    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.storage.image_dir)
            .with_context(|| format!("创建图片目录失败: {}", self.storage.image_dir.display()))?;
        std::fs::create_dir_all(&self.storage.pdf_dir)
            .with_context(|| format!("创建PDF目录失败: {}", self.storage.pdf_dir.display()))?;
        std::fs::create_dir_all(&self.storage.generated_image_dir).with_context(|| {
            format!(
                "创建生成图片目录失败: {}",
                self.storage.generated_image_dir.display()
            )
        })?;
        if let Some(path) = &self.logging.file {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("创建日志目录失败: {}", parent.display()))?;
            }
        }
        Ok(())
    }

    fn resolve_paths(&mut self, config_path: &Path) -> Result<()> {
        let base_dir = config_path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));

        if self.storage.image_dir.is_relative() {
            self.storage.image_dir = base_dir.join(&self.storage.image_dir);
        }
        if self.storage.pdf_dir.is_relative() {
            self.storage.pdf_dir = base_dir.join(&self.storage.pdf_dir);
        }
        if self.storage.generated_image_dir.is_relative() {
            self.storage.generated_image_dir = base_dir.join(&self.storage.generated_image_dir);
        }
        if self.pdf.font_path.is_relative() {
            self.pdf.font_path = base_dir.join(&self.pdf.font_path);
        }
        if let Some(path) = &self.logging.file {
            if path.is_relative() {
                self.logging.file = Some(base_dir.join(path));
            }
        }

        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        self.validate_pdf_font()?;
        self.validate_image_config()?;
        Ok(())
    }

    fn validate_pdf_font(&self) -> Result<()> {
        let path = &self.pdf.font_path;
        if path.as_os_str().is_empty() {
            anyhow::bail!("pdf.font_path 未配置");
        }
        if !path.exists() {
            anyhow::bail!("PDF 字体文件不存在: {}", path.display());
        }
        if !path.is_file() {
            anyhow::bail!("pdf.font_path 不是文件: {}", path.display());
        }
        let bytes = std::fs::read(path)
            .with_context(|| format!("读取 PDF 字体文件失败: {}", path.display()))?;
        if bytes.len() < 100_000 {
            anyhow::bail!("PDF 字体文件过小，可能无效: {}", path.display());
        }
        let data = typst::foundations::Bytes::new(bytes);
        if typst::text::Font::iter(data).next().is_none() {
            anyhow::bail!("PDF 字体文件无法解析为有效字体: {}", path.display());
        }
        Ok(())
    }

    fn validate_image_config(&self) -> Result<()> {
        let Some(image) = &self.llm.image else {
            return Ok(());
        };

        // If image_generation role is bound to a named provider, legacy
        // `[llm.image]` is only an extra-fields container (mime_type,
        // aspect_ratio, timeouts) — missing provider fields are acceptable.
        let has_role_provider = self
            .llm
            .roles
            .image_generation
            .as_ref()
            .and_then(|name| self.llm.providers.get(name))
            .is_some();

        if has_role_provider {
            return Ok(());
        }

        // Legacy path: `[llm.image]` is the actual provider source, so it
        // must be fully configured.
        if image.base_url.trim().is_empty() {
            anyhow::bail!("llm.image.base_url 未配置");
        }
        if image.api_key.trim().is_empty() {
            anyhow::bail!("llm.image.api_key 未配置");
        }
        if image.model.trim().is_empty() {
            anyhow::bail!("llm.image.model 未配置");
        }
        Ok(())
    }

    /// Resolve a merged `HttpProfile` for a given service.
    ///
    /// Resolution order:
    /// 1. `llm.http.defaults` provides shared transport fields.
    /// 2. `llm.http.<service>` provides the per-service `request_timeout_secs` override.
    /// 3. Legacy fallback: if the service is Image/ImageDownload and `llm.http.*` has no
    ///    override, fall back to the old `llm.image.{connect,request,download}_timeout_secs`.
    pub fn resolve_http_profile(&self, kind: HttpServiceKind) -> HttpProfile {
        let d = &self.llm.http.defaults;

        let request_timeout =
            resolve_request_timeout(&self.llm.http, kind, self.llm.image.as_ref());

        HttpProfile {
            connect_timeout: Duration::from_secs(d.connect_timeout_secs),
            request_timeout,
            http1_only: d.http1_only,
            connection_close: d.connection_close,
            disable_compression: d.disable_compression,
            tcp_keepalive: Duration::from_secs(d.tcp_keepalive_secs),
            tcp_nodelay: d.tcp_nodelay,
            user_agent: d.user_agent.clone(),
        }
    }
}

/// Resolve request timeout with legacy fallback for image services.
fn resolve_request_timeout(
    http: &HttpConfig,
    kind: HttpServiceKind,
    legacy_image: Option<&ImageProviderConfig>,
) -> Duration {
    let (override_val, default_secs) = match kind {
        HttpServiceKind::Chat => (http.chat.request_timeout_secs, 120),
        HttpServiceKind::Embedding => (http.embedding.request_timeout_secs, 60),
        HttpServiceKind::Image => {
            // Legacy fallback: llm.image.request_timeout_secs (default 600)
            let fallback = legacy_image
                .map(|ic| ic.request_timeout_secs)
                .unwrap_or(default_image_request_timeout_secs());
            (
                http.image.request_timeout_secs.or(Some(fallback)),
                default_image_request_timeout_secs(),
            )
        }
        HttpServiceKind::ImageDownload => {
            // Legacy fallback: llm.image.download_timeout_secs (default 120)
            let fallback = legacy_image
                .map(|ic| ic.download_timeout_secs)
                .unwrap_or(default_image_download_timeout_secs());
            (
                http.image_download.request_timeout_secs.or(Some(fallback)),
                default_image_download_timeout_secs(),
            )
        }
    };

    Duration::from_secs(override_val.unwrap_or(default_secs))
}

/// Build a `reqwest::Client` from a resolved `HttpProfile`.
///
/// Applies all shared transport settings (connect timeout, http1_only, user agent,
/// compression control, tcp_keepalive, tcp_nodelay). Connection pool is minimised
/// (idle timeout 1 s, max idle per host 0) for curl-like behaviour when
/// `connection_close` is enabled.
pub fn build_client_from_profile(profile: &HttpProfile) -> reqwest::Client {
    let mut builder = reqwest::ClientBuilder::new()
        .connect_timeout(profile.connect_timeout)
        .timeout(profile.request_timeout)
        .user_agent(&profile.user_agent)
        .tcp_keepalive(profile.tcp_keepalive)
        .tcp_nodelay(profile.tcp_nodelay);

    if profile.http1_only {
        builder = builder.http1_only();
    }

    // Minimise connection reuse when connection_close is enabled (curl-like behaviour)
    if profile.connection_close {
        builder = builder
            .pool_idle_timeout(Duration::from_secs(1))
            .pool_max_idle_per_host(0);
    }

    // Disable compression negotiation
    if profile.disable_compression {
        builder = builder.no_gzip().no_brotli().no_zstd().no_deflate();
    }

    builder
        .build()
        .expect("failed to build reqwest Client from HttpProfile")
}
