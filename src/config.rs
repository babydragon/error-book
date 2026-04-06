use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub llm: LlmConfig,
    pub database: DatabaseConfig,
    pub storage: StorageConfig,
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub pdf: PdfConfig,
    #[serde(default)]
    pub search: SearchConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub chat_model: String,
    pub embedding_model: String,
    #[serde(default = "default_embedding_dimensions")]
    pub embedding_dimensions: u32,
    /// Google AI Studio API base URL（用于 embedding 等需要原生 Google 格式的接口）
    /// 默认使用 base_url 去掉 /openai/ 后缀
    #[serde(default)]
    pub google_base_url: Option<String>,
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
    #[serde(default = "default_font_path")]
    pub font_path: PathBuf,
}

impl Default for PdfConfig {
    fn default() -> Self {
        Self {
            font_path: default_font_path(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct SearchConfig {
    /// 混合搜索时图片 embedding 的权重 (0.0~1.0)
    /// 文本权重 = 1.0 - image_weight
    /// 默认 0.3，即文本为主
    #[serde(default = "default_image_weight")]
    pub image_weight: f64,
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
fn default_font_path() -> PathBuf {
    PathBuf::from("./fonts/NotoSansSC-Regular.ttf")
}
fn default_image_weight() -> f64 {
    0.3
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("读取配置文件失败: {}", path.display()))?;
        let config: AppConfig = toml::from_str(&content).with_context(|| "解析配置文件失败")?;

        // 环境变量覆盖
        let mut config = config;
        if let Ok(v) = std::env::var("ERROR_BOOK_LLM_API_KEY") {
            config.llm.api_key = v;
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_LLM_BASE_URL") {
            config.llm.base_url = v;
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_DB_URL") {
            config.database.url = v;
        }
        if let Ok(v) = std::env::var("ERROR_BOOK_GOOGLE_BASE_URL") {
            config.llm.google_base_url = Some(v);
        }

        Ok(config)
    }

    pub fn chat_api_url(&self) -> String {
        format!(
            "{}/chat/completions",
            self.llm.base_url.trim_end_matches('/')
        )
    }

    pub fn embeddings_api_url(&self) -> String {
        format!("{}/embeddings", self.llm.base_url.trim_end_matches('/'))
    }

    /// Google AI Studio embedContent endpoint
    /// 格式: {google_base_url}/v1beta/models/{model}:embedContent
    pub fn google_embed_url(&self) -> String {
        let base = self.google_base_url();
        let model = &self.llm.embedding_model;
        format!(
            "{}/v1beta/models/{}:embedContent",
            base.trim_end_matches('/'),
            model
        )
    }

    /// 获取 Google AI Studio API base URL
    /// 如果配置了 google_base_url 则使用，否则从 base_url 推导（去掉 /openai/ 等后缀）
    fn google_base_url(&self) -> String {
        if let Some(ref url) = self.llm.google_base_url {
            return url.clone();
        }
        // 从 base_url 推导：去掉末尾的路径段（如 /v1beta/openai/ → 取根）
        let url = self.llm.base_url.trim_end_matches('/');
        // 常见模式: https://xxx.com/v1beta/openai → https://xxx.com
        // 或: https://xxx.com/v1/openai → https://xxx.com
        if let Some(idx) = url.find("/openai") {
            url[..idx].to_string()
        } else {
            url.to_string()
        }
    }

    /// 确保存储目录存在
    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.storage.image_dir)
            .with_context(|| format!("创建图片目录失败: {}", self.storage.image_dir.display()))?;
        std::fs::create_dir_all(&self.storage.pdf_dir)
            .with_context(|| format!("创建PDF目录失败: {}", self.storage.pdf_dir.display()))?;
        Ok(())
    }
}
