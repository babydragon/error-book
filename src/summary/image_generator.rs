use anyhow::{Context, Result};
use base64::Engine;
use rand::RngExt;
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use uuid::Uuid;

use crate::config::{AppConfig, HttpServiceKind, ImageProvider};
use crate::db::models::{Summary, SummaryImage};
use crate::db::repository::Repository;
use crate::storage::image::ImageStorage;

#[derive(Clone)]
pub struct SummaryImageGenerator {
    config: AppConfig,
    http: reqwest::Client,
    download_client: reqwest::Client,
    /// Stored for diagnostic logging (not available from built Client).
    request_timeout: Duration,
    connect_timeout: Duration,
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

#[derive(Debug, Deserialize)]
struct OpenAiImageGenerationResponse {
    #[serde(default)]
    data: Vec<OpenAiImageData>,
}

#[derive(Debug, Deserialize)]
struct OpenAiImageData {
    #[serde(rename = "b64_json")]
    b64_json: Option<String>,
    url: Option<String>,
}

impl SummaryImageGenerator {
    pub fn new(config: AppConfig, repository: Repository, storage: ImageStorage) -> Self {
        let image_profile = config.resolve_http_profile(HttpServiceKind::Image);
        let download_profile = config.resolve_http_profile(HttpServiceKind::ImageDownload);

        let http = crate::config::build_client_from_profile(&image_profile);
        let download_client = crate::config::build_client_from_profile(&download_profile);

        tracing::info!(
            request_timeout_secs = image_profile.request_timeout.as_secs(),
            connect_timeout_secs = image_profile.connect_timeout.as_secs(),
            download_timeout_secs = download_profile.request_timeout.as_secs(),
            user_agent = %image_profile.user_agent,
            http1_only = image_profile.http1_only,
            connection_close = image_profile.connection_close,
            disable_compression = image_profile.disable_compression,
            "图片生成 HTTP client 已构建 (统一 profile)"
        );

        Self {
            config,
            http,
            download_client,
            request_timeout: image_profile.request_timeout,
            connect_timeout: image_profile.connect_timeout,
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

        let weak_points: Vec<String> = serde_json::from_str(&summary.weak_points).unwrap_or_default();
        let prompt = crate::llm::prompts::build_summary_infographic_prompt(
            &summary.subject,
            &self.config.defaults.grade_level,
            summary,
            &weak_points,
            extra_requirements,
        );

        let response = match image_config.provider {
            ImageProvider::Google => self.generate_google_image(&prompt).await?,
            ImageProvider::Openai => self.generate_openai_image(&prompt).await?,
        };
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
                    let retryable = is_retryable_status_error(&error);
                    tracing::warn!(
                        attempt = attempt + 1,
                        max = retry_config.max_attempts,
                        retryable,
                        error = %error,
                        error_chain = %format_error_chain(&error),
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

    async fn generate_openai_image(&self, prompt: &str) -> Result<GoogleImagePrediction> {
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

        let body = serde_json::json!({
            "model": image_config.model,
            "prompt": prompt,
            "size": openai_size_from_aspect_ratio(&image_config.aspect_ratio),
            "output_format": openai_output_format_from_mime_type(&image_config.mime_type),
        });

        tracing::debug!(url = %api_url, body = %serde_json::to_string(&body).unwrap_or_default(), "发送 OpenAI 总结信息图生成请求");

        let retry_config = &self.config.llm.retry;
        let mut last_error = None;

        for attempt in 0..retry_config.max_attempts {
            match self
                .send_openai_image_request(&api_url, &image_config.api_key, &body, &image_config.mime_type)
                .await
            {
                Ok(prediction) => return Ok(prediction),
                Err(error) => {
                    let retryable = is_retryable_status_error(&error);
                    tracing::warn!(
                        attempt = attempt + 1,
                        max = retry_config.max_attempts,
                        retryable,
                        error = %error,
                        error_chain = %format_error_chain(&error),
                        "OpenAI 总结信息图生成失败"
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
        tracing::info!(
            url = %api_url,
            model = %model,
            "开始发送 Google 图片生成请求"
        );
        let t0 = Instant::now();

        let response = self
            .http
            .post(api_url)
            .header("x-goog-api-key", api_key)
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await
            .context("Google 图片生成接口请求失败")?;

        tracing::info!(
            status = %response.status(),
            elapsed_ms = t0.elapsed().as_millis() as u64,
            "收到 Google 图片生成响应头"
        );

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Google 图片生成失败 (status={}): {}", status.as_u16(), body);
        }

        let t1 = Instant::now();
        if model.starts_with("gemini-") {
            let parsed: GoogleGenerateContentResponse = response
                .json()
                .await
                .context("解析 Gemini 图片生成响应失败")?;
            tracing::info!(
                elapsed_ms = t1.elapsed().as_millis() as u64,
                "读取 Gemini 响应体完成"
            );
            extract_gemini_image_prediction(parsed)
        } else {
            let parsed: GoogleImagePredictResponse = response
                .json()
                .await
                .context("解析 Imagen 图片生成响应失败")?;

            tracing::info!(
                elapsed_ms = t1.elapsed().as_millis() as u64,
                "读取 Imagen 响应体完成"
            );

            parsed
                .predictions
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("Imagen 图片生成返回空结果"))
        }
    }

    async fn send_openai_image_request(
        &self,
        api_url: &str,
        api_key: &str,
        body: &serde_json::Value,
        fallback_mime_type: &str,
    ) -> Result<GoogleImagePrediction> {
        tracing::info!(
            url = %api_url,
            "开始发送 OpenAI 图片生成请求"
        );

        // Diagnostic: log the timeout / http-version / connection policy applied by the client.
        tracing::info!(
            request_timeout_secs = self.request_timeout.as_secs(),
            connect_timeout_secs = self.connect_timeout.as_secs(),
            http1_only = true,
            connection_close_style = true,
            "OpenAI 图片请求 client 配置诊断"
        );

        let t0 = Instant::now();

        let response = self
            .http
            .post(api_url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            // Explicit curl-like headers
            .header("Connection", "close")
            .header("User-Agent", "curl/8.0-compatible error-book/0.1")
            // Suppress automatic Accept-Encoding header that reqwest may add
            .header("Accept-Encoding", "identity")
            .json(body)
            .send()
            .await
            .context("OpenAI 图片生成接口请求失败")?;

        // Log safe request headers (never log Authorization)
        tracing::info!(
            status = %response.status(),
            elapsed_ms = t0.elapsed().as_millis() as u64,
            headers_safe = "Connection=close, User-Agent=curl/8.0-compatible error-book/0.1, Accept-Encoding=identity, Content-Type=application/json",
            "收到 OpenAI 图片生成响应头"
        );

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI 图片生成失败 (status={}): {}", status.as_u16(), body);
        }

        let t1 = Instant::now();
        let raw_body = response
            .text()
            .await
            .context("读取 OpenAI 图片生成响应失败")?;

        tracing::info!(
            elapsed_ms = t1.elapsed().as_millis() as u64,
            body_len = raw_body.len(),
            "读取 OpenAI 响应体完成"
        );

        tracing::debug!(status = %status, body = %raw_body, "收到 OpenAI 图片生成响应");

        let parsed: OpenAiImageGenerationResponse =
            serde_json::from_str(&raw_body).context("解析 OpenAI 图片生成响应失败")?;

        for item in parsed.data {
            if let Some(image_base64) = item.b64_json {
                return Ok(GoogleImagePrediction {
                    bytes_base64_encoded: Some(image_base64),
                    mime_type: Some(fallback_mime_type.to_string()),
                });
            }

            if let Some(url) = item.url {
                let downloaded = self
                    .download_image_as_base64(&url)
                    .await
                    .with_context(|| format!("下载 OpenAI 图片 URL 失败: {}", url))?;
                return Ok(GoogleImagePrediction {
                    bytes_base64_encoded: Some(downloaded),
                    mime_type: Some(fallback_mime_type.to_string()),
                });
            }
        }

        anyhow::bail!("OpenAI 图片生成返回中既没有 b64_json，也没有 url")
    }

    async fn download_image_as_base64(&self, url: &str) -> Result<String> {
        tracing::info!(url = %url, "开始下载图片 URL (含退避重试)");
        let t0 = Instant::now();

        const MAX_ATTEMPTS: u32 = 5;

        for attempt in 0..MAX_ATTEMPTS {
            let response = match self.download_client.get(url).send().await {
                Ok(resp) => resp,
                Err(err) => {
                    // Network-level errors (connect failure, timeout, DNS) are always retryable.
                    tracing::warn!(
                        attempt = attempt + 1,
                        max = MAX_ATTEMPTS,
                        retryable = true,
                        url = %url,
                        error = %err,
                        "下载图片 URL 网络错误"
                    );
                    if attempt + 1 >= MAX_ATTEMPTS {
                        anyhow::bail!("下载图片 URL 失败 (已达最大重试次数 {}): {}", MAX_ATTEMPTS, err);
                    }
                    let delay = download_retry_delay(attempt);
                    tracing::info!(
                        attempt = attempt + 1,
                        delay_ms = delay.as_millis() as u64,
                        url = %url,
                        "等待后重试下载图片"
                    );
                    sleep(delay).await;
                    continue;
                }
            };

            let status = response.status();
            let retryable = is_download_retryable_status(status);

            if retryable && attempt + 1 < MAX_ATTEMPTS {
                // Consume the body so the connection can be reused / dropped cleanly.
                let body_preview = response.text().await.unwrap_or_default();
                let body_snippet = truncate_str(&body_preview, 200);
                let delay = download_retry_delay(attempt);
                tracing::warn!(
                    attempt = attempt + 1,
                    max = MAX_ATTEMPTS,
                    status = status.as_u16(),
                    retryable,
                    delay_ms = delay.as_millis() as u64,
                    url = %url,
                    body_snippet = %body_snippet,
                    "下载图片 URL 返回可重试状态码，即将重试"
                );
                sleep(delay).await;
                continue;
            }

            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                anyhow::bail!(
                    "下载图片 URL 失败 (status={}, attempts={}): {}",
                    status.as_u16(),
                    attempt + 1,
                    truncate_str(&body, 300)
                );
            }

            let bytes = response.bytes().await.context("读取图片 URL 响应失败")?;

            tracing::info!(
                size_bytes = bytes.len(),
                elapsed_ms = t0.elapsed().as_millis() as u64,
                attempts = attempt + 1,
                url = %url,
                "图片 URL 下载完成"
            );

            return Ok(base64::engine::general_purpose::STANDARD.encode(bytes));
        }

        anyhow::bail!("下载图片 URL 失败: 已达最大重试次数 {}", MAX_ATTEMPTS)
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

fn openai_size_from_aspect_ratio(aspect_ratio: &str) -> &'static str {
    match aspect_ratio {
        "3:4" | "2:3" | "portrait" => "1024x1536",
        "4:3" | "3:2" | "landscape" => "1536x1024",
        _ => "1024x1024",
    }
}

fn openai_output_format_from_mime_type(mime_type: &str) -> &'static str {
    match mime_type {
        "image/jpeg" => "jpeg",
        "image/webp" => "webp",
        _ => "png",
    }
}

fn is_retryable_status_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let text = cause.to_string();
        text.contains("status=429")
            || text.contains("status=500")
            || text.contains("status=502")
            || text.contains("status=503")
            || text.contains("status=504")
    })
}

fn format_error_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .enumerate()
        .map(|(idx, cause)| format!("#{} {}", idx, cause))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn calculate_delay(retry_config: &crate::config::RetryConfig, attempt: u32) -> Duration {
    let base = retry_config.base_delay();
    let max = retry_config.max_delay();
    let exp_delay = base * 2u32.saturating_pow(attempt);
    let jitter = Duration::from_millis(rand::rng().random_range(0..base.as_millis() as u64));
    (exp_delay + jitter).min(max)
}

/// Returns `true` for HTTP status codes that indicate a transient / propagation delay
/// when downloading an image from a CDN (CloudFront / S3) URL.
fn is_download_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(
        status.as_u16(),
        403 | 404 | 429 | 500 | 502 | 503 | 504
    )
}

/// Exponential back-off with jitter for image URL download retries.
/// Delays ≈ 1 s, 2 s, 4 s, 8 s, 16 s (± small random jitter).
fn download_retry_delay(attempt: u32) -> Duration {
    let base_secs: u64 = 1;
    let exp_delay_secs = base_secs.saturating_mul(1u64 << attempt.min(4)); // cap shift at 4
    let jitter_ms = rand::rng().random_range(0..200); // 0–200 ms jitter
    Duration::from_secs(exp_delay_secs) + Duration::from_millis(jitter_ms)
}

/// Truncate a string for safe inclusion in log output.
fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        // Find a valid char boundary near max_len.
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}
