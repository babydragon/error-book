use anyhow::{Context, Result};
use base64::Engine;
use rand::RngExt;
use serde::Deserialize;
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

use crate::config::{AppConfig, ImageProvider};
use crate::db::models::{Summary, SummaryImage};
use crate::db::repository::Repository;
use crate::storage::image::ImageStorage;

#[derive(Clone)]
pub struct SummaryImageGenerator {
    config: AppConfig,
    http: reqwest::Client,
    repository: Repository,
    storage: ImageStorage,
}

pub struct GeneratedSummaryImage {
    pub record: SummaryImage,
    pub full_path: std::path::PathBuf,
}

#[derive(Debug, Deserialize)]
struct GoogleImagePredictResponse {
    #[serde(default)]
    predictions: Vec<GoogleImagePrediction>,
}

#[derive(Debug, Deserialize)]
struct GoogleImagePrediction {
    #[serde(rename = "bytesBase64Encoded")]
    bytes_base64_encoded: Option<String>,
    #[serde(rename = "mimeType")]
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleGenerateContentResponse {
    #[serde(default)]
    candidates: Vec<GoogleGenerateContentCandidate>,
}

#[derive(Debug, Deserialize)]
struct GoogleGenerateContentCandidate {
    content: Option<GoogleGenerateContent>,
}

#[derive(Debug, Deserialize)]
struct GoogleGenerateContent {
    #[serde(default)]
    parts: Vec<GoogleGenerateContentPart>,
}

#[derive(Debug, Deserialize)]
struct GoogleGenerateContentPart {
    text: Option<String>,
    #[serde(rename = "inlineData")]
    inline_data: Option<GoogleInlineData>,
}

#[derive(Debug, Deserialize)]
struct GoogleInlineData {
    #[serde(rename = "mimeType")]
    mime_type: Option<String>,
    data: String,
}

impl SummaryImageGenerator {
    pub fn new(config: AppConfig, repository: Repository, storage: ImageStorage) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
            repository,
            storage,
        }
    }

    pub async fn generate(
        &self,
        summary_id: &str,
        extra_requirements: Option<&str>,
    ) -> Result<GeneratedSummaryImage> {
        let summary = self
            .repository
            .get_summary(summary_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("总结记录不存在: {}", summary_id))?;

        self.generate_from_summary(&summary, extra_requirements).await
    }

    pub async fn generate_from_summary(
        &self,
        summary: &Summary,
        extra_requirements: Option<&str>,
    ) -> Result<GeneratedSummaryImage> {
        let image_config = self
            .config
            .llm
            .image
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("未配置 llm.image，无法生成总结信息图"))?;

        match image_config.provider {
            ImageProvider::Google => {}
            ImageProvider::Openai => {
                anyhow::bail!("当前仅实现 llm.image.provider=google")
            }
        }

        let weak_points: Vec<String> = serde_json::from_str(&summary.weak_points).unwrap_or_default();
        let prompt = crate::llm::prompts::build_summary_infographic_prompt(
            &summary.subject,
            &self.config.defaults.grade_level,
            summary,
            &weak_points,
            extra_requirements,
        );

        let response = self.generate_google_image(&prompt).await?;
        let mime_type = response
            .mime_type
            .unwrap_or_else(|| image_config.mime_type.clone());
        let image_base64 = response
            .bytes_base64_encoded
            .ok_or_else(|| anyhow::anyhow!("图片接口未返回图片数据"))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(image_base64)
            .context("解析图片 base64 失败")?;

        let extension = extension_from_mime_type(&mime_type);
        let image_path = self.storage.save_bytes(&bytes, extension).await?;
        let record = SummaryImage {
            id: Uuid::new_v4().to_string(),
            summary_id: summary.id.clone(),
            prompt,
            image_path: image_path.clone(),
            mime_type,
            created_at: chrono::Utc::now().timestamp(),
        };
        self.repository.insert_summary_image(&record).await?;

        Ok(GeneratedSummaryImage {
            full_path: self.storage.full_path(&image_path),
            record,
        })
    }

    async fn generate_google_image(&self, prompt: &str) -> Result<GoogleImagePrediction> {
        let image_config = self
            .config
            .llm
            .image
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("未配置 llm.image"))?;

        let api_url = self
            .config
            .image_api_url()
            .ok_or_else(|| anyhow::anyhow!("无法构造图片 API 地址"))?;

        let body = if image_config.model.starts_with("gemini-") {
            serde_json::json!({
                "contents": [{
                    "parts": [{
                        "text": prompt,
                    }]
                }],
                "generationConfig": {
                    "responseModalities": ["TEXT", "IMAGE"]
                }
            })
        } else {
            serde_json::json!({
                "instances": [{
                    "prompt": prompt,
                }],
                "parameters": {
                    "sampleCount": 1,
                    "aspectRatio": image_config.aspect_ratio,
                    "outputOptions": {
                        "mimeType": image_config.mime_type,
                    }
                }
            })
        };

        tracing::debug!(url = %api_url, body = %serde_json::to_string(&body).unwrap_or_default(), "发送总结信息图生成请求");

        let retry_config = &self.config.llm.retry;
        let mut last_error = None;

        for attempt in 0..retry_config.max_attempts {
            match self.send_google_image_request(&api_url, &image_config.api_key, &image_config.model, &body).await {
                Ok(prediction) => return Ok(prediction),
                Err(error) => {
                    let retryable = is_retryable(&error, retry_config);
                    tracing::warn!(
                        attempt = attempt + 1,
                        max = retry_config.max_attempts,
                        retryable,
                        error = %error,
                        "总结信息图生成失败"
                    );

                    if !retryable || attempt + 1 >= retry_config.max_attempts {
                        return Err(error);
                    }

                    let delay = calculate_delay(retry_config, attempt);
                    sleep(delay).await;
                    last_error = Some(error);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("图片生成未知错误")))
    }

    async fn send_google_image_request(
        &self,
        api_url: &str,
        api_key: &str,
        model: &str,
        body: &serde_json::Value,
    ) -> Result<GoogleImagePrediction> {
        let response = self
            .http
            .post(api_url)
            .header("x-goog-api-key", api_key)
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await
            .context("Google 图片生成接口请求失败")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Google 图片生成失败 (status={}): {}", status.as_u16(), body);
        }

        if model.starts_with("gemini-") {
            let parsed: GoogleGenerateContentResponse = response
                .json()
                .await
                .context("解析 Gemini 图片生成响应失败")?;
            extract_gemini_image_prediction(parsed)
        } else {
            let parsed: GoogleImagePredictResponse = response
                .json()
                .await
                .context("解析 Imagen 图片生成响应失败")?;

            parsed
                .predictions
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("Imagen 图片生成返回空结果"))
        }
    }
}

fn extension_from_mime_type(mime_type: &str) -> &'static str {
    match mime_type {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    }
}

fn extract_gemini_image_prediction(resp: GoogleGenerateContentResponse) -> Result<GoogleImagePrediction> {
    for candidate in resp.candidates {
        if let Some(content) = candidate.content {
            for part in content.parts {
                if let Some(inline) = part.inline_data {
                    return Ok(GoogleImagePrediction {
                        bytes_base64_encoded: Some(inline.data),
                        mime_type: inline.mime_type,
                    });
                }
                let _ = part.text;
            }
        }
    }

    anyhow::bail!("Gemini 图片生成返回中未找到图片数据")
}

fn is_retryable(error: &anyhow::Error, retry_config: &crate::config::RetryConfig) -> bool {
    error.chain().any(|cause| {
        let text = cause.to_string();
        retry_config
            .retryable_status_codes
            .iter()
            .any(|code| text.contains(&format!("status={}", code)))
    }) || error.downcast_ref::<reqwest::Error>().is_some()
}

fn calculate_delay(retry_config: &crate::config::RetryConfig, attempt: u32) -> Duration {
    let base = retry_config.base_delay();
    let max = retry_config.max_delay();
    let exp_delay = base * 2u32.saturating_pow(attempt);
    let jitter = Duration::from_millis(rand::rng().random_range(0..base.as_millis() as u64));
    (exp_delay + jitter).min(max)
}
