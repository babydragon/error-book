use anyhow::{Context, Result};
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// Chat LLM provider configuration (independent from embedding)
#[derive(Deserialize, Clone)]
pub struct ChatProviderConfig {
    #[serde(default)]
    pub provider: ChatProvider,
    pub base_url: String,
    #[serde(skip_serializing)]
    pub api_key: String,
    pub model: String,
}

impl fmt::Debug for ChatProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatProviderConfig")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
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

/// Embedding LLM provider configuration (independent from chat)
#[derive(Deserialize, Clone)]
pub struct EmbeddingProviderConfig {
    #[serde(default)]
    pub provider: EmbeddingProvider,
    pub base_url: String,
    #[serde(skip_serializing)]
    pub api_key: String,
    pub model: String,
    #[serde(default = "default_embedding_dimensions")]
    pub dimensions: u32,
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
    pub chat: ChatProviderConfig,
    pub embedding: EmbeddingProviderConfig,
    #[serde(default)]
    pub retry: RetryConfig,
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
fn default_image_weight() -> f64 {
    0.3
}
fn default_log_level() -> String {
    "info".to_string()
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
            config.llm.embedding.api_key = v;
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_CHAT_API_KEY") {
            config.llm.chat.api_key = v;
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_EMBEDDING_API_KEY") {
            config.llm.embedding.api_key = v;
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_LLM_BASE_URL") {
            config.llm.chat.base_url = v.clone();
            config.llm.embedding.base_url = v;
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

    /// 确保存储目录存在
    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.storage.image_dir)
            .with_context(|| format!("创建图片目录失败: {}", self.storage.image_dir.display()))?;
        std::fs::create_dir_all(&self.storage.pdf_dir)
            .with_context(|| format!("创建PDF目录失败: {}", self.storage.pdf_dir.display()))?;
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
        self.validate_pdf_font()
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
}
