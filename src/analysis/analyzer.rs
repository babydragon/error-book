use std::path::Path;

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::config::{AppConfig, RoleKind};
use crate::db::models::{AnalysisArtifact, AnalysisRequest, ErrorRecord, DATA_VERSION_CURRENT};
use crate::db::repository::Repository;
use crate::llm::client::{ChatClient, ChatMessage};
use crate::llm::embedding::EmbeddingClient;
use crate::llm::prompts;
use crate::storage::image::ImageStorage;

use super::parser;

/// 管线阶段常量
pub(crate) const STAGE_INPUT_CONTEXT: &str = "input_context";
const STAGE_LEGACY_COMBINED_RAW: &str = "legacy_combined_raw";
const STAGE_LEGACY_COMBINED_PARSED: &str = "legacy_combined_parsed";
const STAGE_VISION_RECOGNITION_RAW: &str = "vision_recognition_raw";
const STAGE_VISION_RECOGNITION_PARSED: &str = "vision_recognition_parsed";
const STAGE_STRUCTURED_EXTRACTION_RAW: &str = "structured_extraction_raw";
const STAGE_STRUCTURED_EXTRACTION_PARSED: &str = "structured_extraction_parsed";
const STAGE_PEDAGOGICAL_ANALYSIS_RAW: &str = "pedagogical_analysis_raw";
const STAGE_PEDAGOGICAL_ANALYSIS_PARSED: &str = "pedagogical_analysis_parsed";
pub(crate) const ARTIFACT_SCHEMA_VERSION: &str = "v1";
/// 当前管线版本标识
pub const PIPELINE_VERSION: &str = "analysis-pipeline-v2";

/// Short reminder appended to retry attempts so the LLM produces clean JSON.
const RETRY_JSON_REMINDER: &str =
    "\n\nIMPORTANT: Return ONLY valid JSON. Do NOT wrap in markdown code fences. Output the raw JSON object only.";

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
        let error_id = Uuid::new_v4().to_string();

        // Buffer artifacts in memory; only flush to DB after the parent
        // error_records row has been inserted successfully. This avoids
        // FOREIGN KEY constraint violations on analysis_artifacts.error_id.
        let mut pending_artifacts: Vec<AnalysisArtifact> = Vec::new();

        // ── 阶段 1: intake / input_context ──
        let input_ctx = self.stage_intake(&request).await?;

        // buffer input_context artifact (not yet persisted)
        Self::buffer_input_context_artifact(&mut pending_artifacts, &error_id, &request, &input_ctx)?;

        // ── Try multi-stage pipeline, fallback to legacy combined ──
        let pipeline_outcome = match self
            .run_multi_stage_pipeline(&mut pending_artifacts, &error_id, &input_ctx, &request)
            .await
        {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Multi-stage pipeline failed, falling back to legacy combined analysis"
                );
                self.run_legacy_combined(&mut pending_artifacts, &error_id, &input_ctx, &request)
                    .await?
            }
        };

        // ── 阶段 final: embedding_and_persist ──
        let record = self
            .stage_embedding_and_persist(
                &error_id,
                &input_ctx,
                &pipeline_outcome,
                &request,
            )
            .await?;

        // Parent error_records row now exists — safe to flush buffered artifacts.
        self.flush_pending_artifacts(&error_id, &pending_artifacts).await?;

        tracing::info!(id = %record.id, pipeline = %pipeline_outcome.pipeline_label, "错题分析完成");
        Ok(record)
    }

    /// 基于 error_id 重新运行多阶段管线，**不插入新记录**，仅返回结构化字段供 backfill 写回。
    ///
    /// 图片从 image_storage 已存储的相对路径中读取，不再重复保存。
    /// 返回的 `ReanalysisResult` 包含所有新结构化字段以及待持久化的分析产物。
    pub async fn reanalyze_from_stored_image(&self, record: &ErrorRecord) -> Result<ReanalysisResult> {
        // 1. 从已存储路径读取图片
        let image_base64 = self.image_storage.read_base64(&record.image_path).await
            .with_context(|| format!("Backfill re-analyze: 读取已存储图片失败 (path={})", record.image_path))?;
        let media_type = detect_media_type(Path::new(&record.image_path)).to_string();

        let input_ctx = InputContext {
            stored_name: record.image_path.clone(),
            source_image_path: record.image_path.clone(),
            image_base64,
            media_type,
        };

        // 2. 构造合成请求（复用 prompt 构建逻辑）
        let request = AnalysisRequest {
            image_path: record.image_path.clone(),
            subject: Some(record.subject.clone()),
            grade_level: Some(record.grade_level.clone()),
            color_teacher: None,
            color_correction: None,
        };

        // 3. 运行多阶段管线（含 legacy fallback）
        let mut pending_artifacts: Vec<AnalysisArtifact> = Vec::new();

        // buffer input_context artifact
        Self::buffer_input_context_artifact(&mut pending_artifacts, &record.id, &request, &input_ctx)?;

        let pipeline_outcome = match self
            .run_multi_stage_pipeline(&mut pending_artifacts, &record.id, &input_ctx, &request)
            .await
        {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Backfill re-analyze: 多阶段管线失败，回退到 legacy combined"
                );
                self.run_legacy_combined(&mut pending_artifacts, &record.id, &input_ctx, &request)
                    .await?
            }
        };

        Ok(ReanalysisResult {
            question_markdown_clean: pipeline_outcome.question_markdown_clean,
            question_structure_json: pipeline_outcome.question_structure_json,
            student_answer_text: pipeline_outcome.student_answer_text,
            teacher_marks_json: pipeline_outcome.teacher_marks_json,
            question_type: pipeline_outcome.question_type,
            difficulty: pipeline_outcome.difficulty,
            error_type: pipeline_outcome.error_type,
            error_subtype: pipeline_outcome.error_subtype,
            root_cause_code: pipeline_outcome.root_cause_code,
            confidence_json: pipeline_outcome.confidence_json,
            pipeline_version: PIPELINE_VERSION.to_string(),
            model_trace_json: Some(serde_json::to_string(&pipeline_outcome.model_trace)?),
            artifacts: pending_artifacts,
        })
    }

    // ── Multi-stage pipeline (vision → extraction → pedagogical) ──────

    async fn run_multi_stage_pipeline(
        &self,
        pending_artifacts: &mut Vec<AnalysisArtifact>,
        error_id: &str,
        input_ctx: &InputContext,
        request: &AnalysisRequest,
    ) -> Result<PipelineOutcome> {
        // Stage 2a: Vision Recognition (with retry)
        let (vision_raw, vision_parsed) = self
            .run_vision_recognition_with_retry(pending_artifacts, error_id, input_ctx, request)
            .await?;
        Self::buffer_artifact(
            pending_artifacts,
            error_id,
            STAGE_VISION_RECOGNITION_PARSED,
            &serde_json::to_value(&vision_parsed)?,
            None,
        )?;

        // Stage 2b: Structured Extraction (with retry)
        let (extraction_raw, extraction_parsed) = self
            .run_structured_extraction_with_retry(pending_artifacts, error_id, &vision_raw, request)
            .await?;
        Self::buffer_artifact(
            pending_artifacts,
            error_id,
            STAGE_STRUCTURED_EXTRACTION_PARSED,
            &serde_json::to_value(&extraction_parsed)?,
            None,
        )?;

        // Stage 2c: Pedagogical Analysis (with retry)
        let student_answer = &vision_parsed.student_answer_text;
        let (_pedagogical_raw, pedagogical_parsed) = self
            .run_pedagogical_analysis_with_retry(
                pending_artifacts,
                error_id,
                &extraction_raw,
                student_answer,
                request,
            )
            .await?;
        Self::buffer_artifact(
            pending_artifacts,
            error_id,
            STAGE_PEDAGOGICAL_ANALYSIS_PARSED,
            &serde_json::to_value(&pedagogical_parsed)?,
            None,
        )?;

        // Build model trace for multi-stage
        let v_info = self
            .chat_client
            .model_info_for_role(&self.config.llm, RoleKind::VisionRecognition);
        let e_info = self
            .chat_client
            .model_info_for_role(&self.config.llm, RoleKind::StructuredExtraction);
        let p_info = self
            .chat_client
            .model_info_for_role(&self.config.llm, RoleKind::PedagogicalAnalysis);
        let model_trace = serde_json::json!({
            "stages": [
                { "stage": "vision_recognition", "provider": v_info.1, "model": v_info.0 },
                { "stage": "structured_extraction", "provider": e_info.1, "model": e_info.0 },
                { "stage": "pedagogical_analysis", "provider": p_info.1, "model": p_info.0 },
            ]
        });

        Ok(PipelineOutcome {
            pipeline_label: "multi_stage_v2".to_string(),
            // Legacy-compatible fields
            subject: pedagogical_parsed.subject.clone(),
            classification: pedagogical_parsed.classification.clone(),
            original_question: extraction_parsed.question_markdown_clean.clone(),
            image_regions: extraction_parsed.image_regions.clone(),
            error_reason: pedagogical_parsed.error_reason.clone(),
            suggestions: pedagogical_parsed.suggestions.clone(),
            model_trace,
            // New structured fields
            question_markdown_clean: Some(extraction_parsed.question_markdown_clean.clone()),
            question_structure_json: Some(serde_json::to_string(
                &extraction_parsed.question_structure,
            )?),
            student_answer_text: Some(vision_parsed.student_answer_text.clone()),
            teacher_marks_json: Some(serde_json::to_string(&vision_parsed.teacher_marks)?),
            question_type: Some(extraction_parsed.question_structure.question_type.clone()),
            difficulty: Some(extraction_parsed.difficulty.clone()),
            error_type: Some(pedagogical_parsed.error_type.clone()),
            error_subtype: Some(pedagogical_parsed.error_subtype.clone()),
            confidence_json: Some(serde_json::to_string(&pedagogical_parsed.confidence)?),
            root_cause_code: Some(pedagogical_parsed.root_cause_code.clone()),
        })
    }

    // ── Per-stage retry wrappers ──────────────────────────────────

    /// Run vision recognition with a single retry on parse failure.
    /// Returns `(raw_response, parsed_result)` and buffers raw artifacts for each attempt.
    async fn run_vision_recognition_with_retry(
        &self,
        pending_artifacts: &mut Vec<AnalysisArtifact>,
        error_id: &str,
        input_ctx: &InputContext,
        request: &AnalysisRequest,
    ) -> Result<(String, parser::VisionRecognitionResult)> {
        let model_name: Option<String> = self
            .chat_client
            .model_info_for_role(&self.config.llm, RoleKind::VisionRecognition)
            .0
            .into();

        // Attempt 1
        let raw1 = self
            .stage_vision_recognition(input_ctx, request, None)
            .await?;

        match parser::parse_vision_recognition(&raw1) {
            Ok(parsed) => {
                Self::buffer_artifact(
                    pending_artifacts,
                    error_id,
                    STAGE_VISION_RECOGNITION_RAW,
                    &serde_json::json!({
                        "raw_response": raw1,
                        "pipeline_mode": "multi_stage",
                        "attempt": 1,
                    }),
                    model_name.clone(),
                )?;
                Ok((raw1, parsed))
            }
            Err(e1) => {
                tracing::warn!(error = %e1, "Vision recognition attempt 1 parse failed, retrying");
                Self::buffer_artifact(
                    pending_artifacts,
                    error_id,
                    STAGE_VISION_RECOGNITION_RAW,
                    &serde_json::json!({
                        "raw_response": raw1,
                        "pipeline_mode": "multi_stage",
                        "attempt": 1,
                        "retry_reason": "parse_failure",
                    }),
                    model_name.clone(),
                )?;

                // Attempt 2 with JSON reminder
                let raw2 = self
                    .stage_vision_recognition(input_ctx, request, Some(RETRY_JSON_REMINDER))
                    .await?;
                let parsed = parser::parse_vision_recognition(&raw2)?;
                Self::buffer_artifact(
                    pending_artifacts,
                    error_id,
                    STAGE_VISION_RECOGNITION_RAW,
                    &serde_json::json!({
                        "raw_response": raw2,
                        "pipeline_mode": "multi_stage",
                        "attempt": 2,
                        "retry_reason": "parse_failure",
                    }),
                    model_name,
                )?;
                Ok((raw2, parsed))
            }
        }
    }

    /// Run structured extraction with a single retry on parse failure.
    async fn run_structured_extraction_with_retry(
        &self,
        pending_artifacts: &mut Vec<AnalysisArtifact>,
        error_id: &str,
        vision_raw: &str,
        request: &AnalysisRequest,
    ) -> Result<(String, parser::StructuredExtractionResult)> {
        let model_name: Option<String> = self
            .chat_client
            .model_info_for_role(&self.config.llm, RoleKind::StructuredExtraction)
            .0
            .into();

        // Attempt 1
        let raw1 = self
            .stage_structured_extraction(vision_raw, request, None)
            .await?;

        match parser::parse_structured_extraction(&raw1) {
            Ok(parsed) => {
                Self::buffer_artifact(
                    pending_artifacts,
                    error_id,
                    STAGE_STRUCTURED_EXTRACTION_RAW,
                    &serde_json::json!({
                        "raw_response": raw1,
                        "pipeline_mode": "multi_stage",
                        "attempt": 1,
                    }),
                    model_name.clone(),
                )?;
                Ok((raw1, parsed))
            }
            Err(e1) => {
                tracing::warn!(error = %e1, "Structured extraction attempt 1 parse failed, retrying");
                Self::buffer_artifact(
                    pending_artifacts,
                    error_id,
                    STAGE_STRUCTURED_EXTRACTION_RAW,
                    &serde_json::json!({
                        "raw_response": raw1,
                        "pipeline_mode": "multi_stage",
                        "attempt": 1,
                        "retry_reason": "parse_failure",
                    }),
                    model_name.clone(),
                )?;

                // Attempt 2 with JSON reminder
                let raw2 = self
                    .stage_structured_extraction(vision_raw, request, Some(RETRY_JSON_REMINDER))
                    .await?;
                let parsed = parser::parse_structured_extraction(&raw2)?;
                Self::buffer_artifact(
                    pending_artifacts,
                    error_id,
                    STAGE_STRUCTURED_EXTRACTION_RAW,
                    &serde_json::json!({
                        "raw_response": raw2,
                        "pipeline_mode": "multi_stage",
                        "attempt": 2,
                        "retry_reason": "parse_failure",
                    }),
                    model_name,
                )?;
                Ok((raw2, parsed))
            }
        }
    }

    /// Run pedagogical analysis with a single retry on parse failure.
    async fn run_pedagogical_analysis_with_retry(
        &self,
        pending_artifacts: &mut Vec<AnalysisArtifact>,
        error_id: &str,
        extraction_raw: &str,
        student_answer: &str,
        request: &AnalysisRequest,
    ) -> Result<(String, parser::PedagogicalAnalysisResult)> {
        let model_name: Option<String> = self
            .chat_client
            .model_info_for_role(&self.config.llm, RoleKind::PedagogicalAnalysis)
            .0
            .into();

        // Attempt 1
        let raw1 = self
            .stage_pedagogical_analysis(extraction_raw, student_answer, request, None)
            .await?;

        match parser::parse_pedagogical_analysis(&raw1) {
            Ok(parsed) => {
                Self::buffer_artifact(
                    pending_artifacts,
                    error_id,
                    STAGE_PEDAGOGICAL_ANALYSIS_RAW,
                    &serde_json::json!({
                        "raw_response": raw1,
                        "pipeline_mode": "multi_stage",
                        "attempt": 1,
                    }),
                    model_name.clone(),
                )?;
                Ok((raw1, parsed))
            }
            Err(e1) => {
                tracing::warn!(error = %e1, "Pedagogical analysis attempt 1 parse failed, retrying");
                Self::buffer_artifact(
                    pending_artifacts,
                    error_id,
                    STAGE_PEDAGOGICAL_ANALYSIS_RAW,
                    &serde_json::json!({
                        "raw_response": raw1,
                        "pipeline_mode": "multi_stage",
                        "attempt": 1,
                        "retry_reason": "parse_failure",
                    }),
                    model_name.clone(),
                )?;

                // Attempt 2 with JSON reminder
                let raw2 = self
                    .stage_pedagogical_analysis(extraction_raw, student_answer, request, Some(RETRY_JSON_REMINDER))
                    .await?;
                let parsed = parser::parse_pedagogical_analysis(&raw2)?;
                Self::buffer_artifact(
                    pending_artifacts,
                    error_id,
                    STAGE_PEDAGOGICAL_ANALYSIS_RAW,
                    &serde_json::json!({
                        "raw_response": raw2,
                        "pipeline_mode": "multi_stage",
                        "attempt": 2,
                        "retry_reason": "parse_failure",
                    }),
                    model_name,
                )?;
                Ok((raw2, parsed))
            }
        }
    }

    // ── Legacy combined pipeline (existing, used as fallback) ──────

    async fn run_legacy_combined(
        &self,
        pending_artifacts: &mut Vec<AnalysisArtifact>,
        error_id: &str,
        input_ctx: &InputContext,
        request: &AnalysisRequest,
    ) -> Result<PipelineOutcome> {
        let raw_response = self
            .stage_legacy_combined_call(input_ctx, request)
            .await?;

        let (model_name, provider_name) = self.chat_client.model_info();
        Self::buffer_legacy_combined_raw_artifact(pending_artifacts, error_id, &raw_response, model_name, provider_name)?;

        let parsed = parser::parse_analysis_response_structured(&raw_response)
            .context("解析 LLM 响应失败")?;

        Self::buffer_legacy_combined_parsed_artifact(pending_artifacts, error_id, &parsed)?;

        let model_trace = serde_json::json!({
            "stages": [{
                "stage": "legacy_combined",
                "provider": provider_name,
                "model": model_name,
            }]
        });

        Ok(PipelineOutcome {
            pipeline_label: "legacy_combined".to_string(),
            subject: parsed.pedagogical_analysis.subject.clone(),
            classification: parsed.pedagogical_analysis.classification.clone(),
            original_question: parsed.question_extraction.question_markdown.clone(),
            image_regions: parsed.question_extraction.image_regions.clone(),
            error_reason: parsed.pedagogical_analysis.error_reason.clone(),
            suggestions: parsed.pedagogical_analysis.suggestions.clone(),
            model_trace,
            // Fill only basic fields from legacy path
            question_markdown_clean: Some(parsed.question_extraction.question_markdown.clone()),
            question_structure_json: None,
            student_answer_text: None,
            teacher_marks_json: None,
            question_type: None,
            difficulty: None,
            error_type: None,
            error_subtype: None,
            confidence_json: None,
            root_cause_code: None,
        })
    }

    // ── 阶段 1: intake / input_context ──────────────────────────

    /// 输入阶段：保存图片、读取 base64、记录媒体类型
    async fn stage_intake(&self, request: &AnalysisRequest) -> Result<InputContext> {
        let image_path = Path::new(&request.image_path);
        if !image_path.exists() {
            anyhow::bail!("图片文件不存在: {}", request.image_path);
        }

        let stored_name = self.image_storage.save(image_path).await?;
        tracing::info!(stored_name = %stored_name, "图片已保存");

        let image_base64 = self.image_storage.read_base64(&stored_name).await?;
        let media_type = detect_media_type(image_path);

        Ok(InputContext {
            stored_name,
            source_image_path: request.image_path.clone(),
            image_base64,
            media_type: media_type.to_string(),
        })
    }

    // ── Multi-stage: vision_recognition ─────────────────────────

    async fn stage_vision_recognition(
        &self,
        input_ctx: &InputContext,
        request: &AnalysisRequest,
        retry_suffix: Option<&str>,
    ) -> Result<String> {
        let system_msgs = prompts::build_vision_recognition_prompt(request);
        let mut messages = system_msgs;
        let user_text = match retry_suffix {
            Some(suffix) => format!("{}{}", prompts::vision_recognition_user_text(), suffix),
            None => prompts::vision_recognition_user_text().to_string(),
        };
        messages.push(ChatMessage::user_image_text(
            &input_ctx.image_base64,
            &input_ctx.media_type,
            &user_text,
        ));

        tracing::info!("开始 multi-stage 阶段 1: vision_recognition...");
        let raw = self
            .chat_client
            .chat_with_role(
                &self.config.llm,
                RoleKind::VisionRecognition,
                messages,
                Some(0.1),
            )
            .await?;
        tracing::debug!(response_len = raw.len(), "Vision recognition 原始响应");
        Ok(raw)
    }

    // ── Multi-stage: structured_extraction ──────────────────────

    async fn stage_structured_extraction(
        &self,
        vision_json: &str,
        request: &AnalysisRequest,
        retry_suffix: Option<&str>,
    ) -> Result<String> {
        let system_msgs = prompts::build_structured_extraction_prompt(&self.config, request);
        let mut messages = system_msgs;
        let user_text = match retry_suffix {
            Some(suffix) => format!("{}{}", prompts::structured_extraction_user_text(vision_json), suffix),
            None => prompts::structured_extraction_user_text(vision_json),
        };
        messages.push(ChatMessage::user_text(&user_text));

        tracing::info!("开始 multi-stage 阶段 2: structured_extraction...");
        let raw = self
            .chat_client
            .chat_with_role(
                &self.config.llm,
                RoleKind::StructuredExtraction,
                messages,
                Some(0.1),
            )
            .await?;
        tracing::debug!(response_len = raw.len(), "Structured extraction 原始响应");
        Ok(raw)
    }

    // ── Multi-stage: pedagogical_analysis ────────────────────────

    async fn stage_pedagogical_analysis(
        &self,
        extraction_json: &str,
        student_answer: &str,
        request: &AnalysisRequest,
        retry_suffix: Option<&str>,
    ) -> Result<String> {
        let system_msgs = prompts::build_pedagogical_analysis_prompt(&self.config, request);
        let mut messages = system_msgs;
        let user_text = match retry_suffix {
            Some(suffix) => format!("{}{}", prompts::pedagogical_analysis_user_text(extraction_json, student_answer), suffix),
            None => prompts::pedagogical_analysis_user_text(extraction_json, student_answer),
        };
        messages.push(ChatMessage::user_text(&user_text));

        tracing::info!("开始 multi-stage 阶段 3: pedagogical_analysis...");
        let raw = self
            .chat_client
            .chat_with_role(
                &self.config.llm,
                RoleKind::PedagogicalAnalysis,
                messages,
                Some(0.3),
            )
            .await?;
        tracing::debug!(response_len = raw.len(), "Pedagogical analysis 原始响应");
        Ok(raw)
    }

    // ── Legacy: combined_call ─────────────────────────────────────

    /// 构造现有 prompt 并发起现有单次多模态调用
    async fn stage_legacy_combined_call(
        &self,
        input_ctx: &InputContext,
        request: &AnalysisRequest,
    ) -> Result<String> {
        let mut messages = prompts::build_analysis_prompt(&self.config, request);
        messages.push(ChatMessage::user_image_text(
            &input_ctx.image_base64,
            &input_ctx.media_type,
            prompts::analysis_user_text(),
        ));

        tracing::info!("开始调用 LLM 分析错题 (legacy combined)...");
        // Use VisionRecognition role as this is a multimodal call (image + text).
        // It is the most appropriate fallback since the legacy combined path
        // performs visual recognition as part of a single multimodal request.
        let raw_response = self.chat_client.chat_with_role(
            &self.config.llm,
            RoleKind::VisionRecognition,
            messages,
            Some(0.3),
        ).await?;
        tracing::debug!(response_len = raw_response.len(), "LLM 原始响应");

        Ok(raw_response)
    }

    // ── 阶段 final: embedding_and_persist ───────────────────────

    /// 生成 embedding 并落 ErrorRecord
    async fn stage_embedding_and_persist(
        &self,
        error_id: &str,
        input_ctx: &InputContext,
        outcome: &PipelineOutcome,
        request: &AnalysisRequest,
    ) -> Result<ErrorRecord> {
        let embedding_text = format!(
            "科目: {}\n知识点: {}\n原题: {}\n原因: {}\n建议: {}",
            outcome.subject,
            outcome.classification.join("、"),
            outcome.original_question,
            outcome.error_reason,
            outcome.suggestions,
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
            .embed_image_only(&input_ctx.image_base64, &input_ctx.media_type)
            .await?;
        tracing::info!(dimensions = image_embedding.len(), "图片 Embedding 生成完成");

        let record = ErrorRecord {
            id: error_id.to_string(),
            image_path: input_ctx.stored_name.clone(),
            subject: outcome.subject.clone(),
            grade_level: request
                .grade_level
                .clone()
                .unwrap_or_else(|| self.config.defaults.grade_level.clone()),
            original_question: outcome.original_question.clone(),
            image_regions: if outcome.image_regions.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&outcome.image_regions)?)
            },
            classification: serde_json::to_string(&outcome.classification)?,
            error_reason: outcome.error_reason.clone(),
            suggestions: outcome.suggestions.clone(),
            text_embedding,
            image_embedding,
            created_at: chrono::Utc::now().timestamp(),
            // New structured fields from pipeline outcome
            question_markdown_clean: outcome.question_markdown_clean.clone(),
            pipeline_version: Some(PIPELINE_VERSION.to_string()),
            model_trace_json: Some(serde_json::to_string(&outcome.model_trace)?),
            question_structure_json: outcome.question_structure_json.clone(),
            student_answer_text: outcome.student_answer_text.clone(),
            teacher_marks_json: outcome.teacher_marks_json.clone(),
            question_type: outcome.question_type.clone(),
            difficulty: outcome.difficulty.clone(),
            error_type: outcome.error_type.clone(),
            error_subtype: outcome.error_subtype.clone(),
            root_cause_code: outcome.root_cause_code.clone(),
            confidence_json: outcome.confidence_json.clone(),
            data_version: DATA_VERSION_CURRENT,
        };

        self.repository
            .insert_error_record(&record, &outcome.classification)
            .await?;

        Ok(record)
    }

    // ── Artifact 缓冲 + 落库辅助 ───────────────────────────────

    /// Buffer input_context artifact in memory (no DB write yet).
    fn buffer_input_context_artifact(
        pending_artifacts: &mut Vec<AnalysisArtifact>,
        error_id: &str,
        request: &AnalysisRequest,
        input_ctx: &InputContext,
    ) -> Result<()> {
        let payload = serde_json::json!({
            "stored_image_path": input_ctx.stored_name,
            "source_image_path": input_ctx.source_image_path,
            "mime_type": input_ctx.media_type,
            "subject_hint": request.subject,
            "grade_level_hint": request.grade_level,
            "teacher_color": request.color_teacher,
            "correction_color": request.color_correction,
        });

        Self::buffer_artifact(pending_artifacts, error_id, STAGE_INPUT_CONTEXT, &payload, None)?;
        Ok(())
    }

    /// Buffer legacy combined raw artifact in memory.
    fn buffer_legacy_combined_raw_artifact(
        pending_artifacts: &mut Vec<AnalysisArtifact>,
        error_id: &str,
        raw_response: &str,
        model_name: &str,
        provider_name: &str,
    ) -> Result<()> {
        let payload = serde_json::json!({
            "raw_response": raw_response,
            "prompt_version": ARTIFACT_SCHEMA_VERSION,
            "analysis_mode": "legacy_combined",
            "provider": provider_name,
            "model": model_name,
        });

        Self::buffer_artifact(
            pending_artifacts,
            error_id,
            STAGE_LEGACY_COMBINED_RAW,
            &payload,
            Some(model_name.to_string()),
        )?;
        Ok(())
    }

    /// Buffer legacy combined parsed artifact in memory.
    fn buffer_legacy_combined_parsed_artifact(
        pending_artifacts: &mut Vec<AnalysisArtifact>,
        error_id: &str,
        parsed: &parser::ParsedLegacyAnalysis,
    ) -> Result<()> {
        let payload = serde_json::to_value(parsed)?;
        Self::buffer_artifact(pending_artifacts, error_id, STAGE_LEGACY_COMBINED_PARSED, &payload, None)?;
        Ok(())
    }

    /// Generic artifact buffer helper: appends to the in-memory vec, no DB write.
    fn buffer_artifact(
        pending_artifacts: &mut Vec<AnalysisArtifact>,
        error_id: &str,
        stage: &str,
        payload: &serde_json::Value,
        model_name: Option<String>,
    ) -> Result<()> {
        pending_artifacts.push(AnalysisArtifact {
            id: Uuid::new_v4().to_string(),
            error_id: error_id.to_string(),
            stage: stage.to_string(),
            schema_version: ARTIFACT_SCHEMA_VERSION.to_string(),
            model_name,
            payload_json: serde_json::to_string(payload)?,
            created_at: chrono::Utc::now().timestamp(),
        });
        Ok(())
    }

    /// Flush all buffered artifacts to DB. Call only after the parent
    /// error_records row has been inserted successfully.
    async fn flush_pending_artifacts(
        &self,
        error_id: &str,
        pending_artifacts: &[AnalysisArtifact],
    ) -> Result<()> {
        for artifact in pending_artifacts {
            if let Err(e) = self.repository.insert_analysis_artifact(artifact).await {
                tracing::error!(
                    error_id = %error_id,
                    artifact_id = %artifact.id,
                    stage = %artifact.stage,
                    error = %e,
                    "Failed to flush buffered artifact to DB"
                );
                return Err(e).context(format!(
                    "Failed to insert artifact (stage={}) for error_id={}",
                    artifact.stage, error_id
                ));
            }
        }
        tracing::info!(
            error_id = %error_id,
            count = pending_artifacts.len(),
            "All buffered artifacts flushed to DB"
        );
        Ok(())
    }
}

// ── 内部数据结构 ────────────────────────────────────────────────

/// 重新分析结果（用于 backfill），包含结构化字段和待持久化的分析产物。
/// 不包含 legacy 核心字段（original_question, classification, error_reason, suggestions）。
pub struct ReanalysisResult {
    pub question_markdown_clean: Option<String>,
    pub question_structure_json: Option<String>,
    pub student_answer_text: Option<String>,
    pub teacher_marks_json: Option<String>,
    pub question_type: Option<String>,
    pub difficulty: Option<String>,
    pub error_type: Option<String>,
    pub error_subtype: Option<String>,
    pub root_cause_code: Option<String>,
    pub confidence_json: Option<String>,
    pub pipeline_version: String,
    pub model_trace_json: Option<String>,
    /// 待持久化的分析产物（调用方负责 flush，因为 error_id 已存在于 DB）
    pub artifacts: Vec<AnalysisArtifact>,
}

/// 阶段 1 的产出
struct InputContext {
    stored_name: String,
    source_image_path: String,
    image_base64: String,
    media_type: String,
}

/// Pipeline outcome: unified result from either multi-stage or legacy combined
struct PipelineOutcome {
    /// Which pipeline produced this result
    pipeline_label: String,
    // Legacy-compatible fields (always filled)
    subject: String,
    classification: Vec<String>,
    original_question: String,
    image_regions: Vec<Vec<f64>>,
    error_reason: String,
    suggestions: String,
    model_trace: serde_json::Value,
    // New structured fields (filled by multi-stage, None by legacy)
    question_markdown_clean: Option<String>,
    question_structure_json: Option<String>,
    student_answer_text: Option<String>,
    teacher_marks_json: Option<String>,
    question_type: Option<String>,
    difficulty: Option<String>,
    error_type: Option<String>,
    error_subtype: Option<String>,
    confidence_json: Option<String>,
    root_cause_code: Option<String>,
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
