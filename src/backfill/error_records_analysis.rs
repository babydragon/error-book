use anyhow::Result;

use crate::analysis::analyzer::{Analyzer, PIPELINE_VERSION};
use crate::db::models::{AnalysisBackfillFields, BackfillStats};
use crate::db::repository::Repository;

use super::engine::BackfillOptions;

/// 每批处理的记录数（LLM 调用密集，使用较小批次）
const BATCH_SIZE: u32 = 10;

/// 执行 error-records-analysis backfill：
/// 对尚未填充结构化分析字段的历史 error_records 重新运行多阶段管线，
/// 仅更新结构化字段和版本元数据，不覆盖 legacy 核心字段，不插入新记录。
pub async fn run_error_records_analysis_backfill(
    repository: &Repository,
    analyzer: &Analyzer,
    run_id: &str,
    opts: &BackfillOptions,
) -> Result<BackfillStats> {
    let mut stats = BackfillStats::default();
    let mut after_id: Option<String> = None;

    loop {
        let records = repository
            .list_error_records_for_analysis_backfill(
                Some(BATCH_SIZE),
                after_id.as_deref(),
            )
            .await?;

        if records.is_empty() {
            break;
        }

        // 应用 limit 截断
        let remaining = match opts.limit {
            Some(limit) => {
                let processed = stats.total_scanned;
                if processed >= limit {
                    break;
                }
                (limit - processed) as usize
            }
            None => records.len(),
        };

        for record in records.iter().take(remaining) {
            stats.total_scanned += 1;

            match process_single_record(repository, analyzer, record, opts.dry_run).await {
                Ok(did_upgrade) => {
                    if did_upgrade {
                        stats.upgraded += 1;
                    } else {
                        stats.skipped += 1;
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        id = %record.id,
                        error = %e,
                        "Analysis backfill 单条记录失败，继续处理"
                    );
                    stats.failed += 1;

                    if opts.fail_fast {
                        anyhow::bail!(
                            "Analysis backfill 在记录 {} 失败 (fail-fast): {}",
                            record.id,
                            e
                        );
                    }
                }
            }

            after_id = Some(record.id.clone());
        }

        // 更新进度
        let stats_json = serde_json::to_string(&stats)?;
        let _ = repository
            .update_backfill_run_progress(
                run_id,
                &stats_json,
                Some("error_records_analysis"),
                after_id.as_deref(),
                chrono::Utc::now().timestamp(),
            )
            .await;

        // 如果本批不足 BATCH_SIZE，说明已经处理完
        if records.len() < BATCH_SIZE as usize {
            break;
        }

        // 检查 limit 是否达到
        if let Some(limit) = opts.limit {
            if stats.total_scanned >= limit {
                break;
            }
        }
    }

    Ok(stats)
}

/// 处理单条记录：调用 Analyzer 重新分析已存储图片，然后仅更新结构化字段。
/// 返回 Ok(true) 表示有更新，Ok(false) 表示跳过。
async fn process_single_record(
    repository: &Repository,
    analyzer: &Analyzer,
    record: &crate::db::models::ErrorRecord,
    dry_run: bool,
) -> Result<bool> {
    // 二次检查：如果已有结构化字段则跳过（查询条件已过滤，此处为防御性检查）
    if record.question_type.is_some()
        && record.error_type.is_some()
        && record.difficulty.is_some()
        && record.pipeline_version.as_deref() == Some(PIPELINE_VERSION)
    {
        tracing::debug!(id = %record.id, "记录已由当前管线分析过，跳过");
        return Ok(false);
    }

    tracing::info!(id = %record.id, "开始 re-analyze 已存储图片");

    let result = analyzer.reanalyze_from_stored_image(record).await?;

    if dry_run {
        tracing::info!(
            id = %record.id,
            "[dry-run] 将更新: question_type={:?}, error_type={:?}, difficulty={:?}, pipeline_version={}",
            result.question_type,
            result.error_type,
            result.difficulty,
            result.pipeline_version,
        );
        return Ok(true);
    }

    // 构造结构化字段更新参数
    let fields = AnalysisBackfillFields {
        question_markdown_clean: result.question_markdown_clean,
        question_structure_json: result.question_structure_json,
        student_answer_text: result.student_answer_text,
        teacher_marks_json: result.teacher_marks_json,
        question_type: result.question_type,
        difficulty: result.difficulty,
        error_type: result.error_type,
        error_subtype: result.error_subtype,
        root_cause_code: result.root_cause_code,
        confidence_json: result.confidence_json,
        pipeline_version: result.pipeline_version,
        model_trace_json: result.model_trace_json,
    };

    // 仅更新结构化字段，不动 legacy 核心字段
    repository
        .update_error_record_analysis_backfill(&record.id, &fields)
        .await?;

    // 持久化分析产物（error_id 已存在于 DB，可直接写入）
    for artifact in &result.artifacts {
        if let Err(e) = repository.insert_analysis_artifact(artifact).await {
            tracing::warn!(
                id = %record.id,
                artifact_id = %artifact.id,
                stage = %artifact.stage,
                error = %e,
                "Analysis backfill: 分析产物写入失败（不阻塞主流程）"
            );
        }
    }

    tracing::info!(id = %record.id, "Analysis backfill 完成");
    Ok(true)
}
