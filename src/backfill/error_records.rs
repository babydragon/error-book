use anyhow::Result;

use crate::db::models::BackfillStats;
use crate::db::repository::Repository;

use super::engine::BackfillOptions;

/// error_records 表的最新 data_version
/// v1: 原始版本（旧数据无 data_version 字段，默认为 1）
/// v2: 规范化/补全版本（pipeline_version 为空时填 legacy-backfill-v1，
///     question_markdown_clean 为空时回填 original_question）
pub const LATEST_ERROR_RECORDS_DATA_VERSION: i64 = 2;

/// 每批处理的记录数
const BATCH_SIZE: u32 = 100;

/// 执行 error_records 的 backfill
pub async fn run_error_records_backfill(
    repository: &Repository,
    run_id: &str,
    opts: &BackfillOptions,
) -> Result<BackfillStats> {
    let mut stats = BackfillStats::default();
    let mut after_id: Option<String> = None;

    loop {
        let records = repository
            .list_error_records_for_backfill(
                LATEST_ERROR_RECORDS_DATA_VERSION,
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

            match upgrade_error_record(repository, record, opts.dry_run).await {
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
                        "Backfill 单条记录失败，继续处理"
                    );
                    stats.failed += 1;

                    if opts.fail_fast {
                        anyhow::bail!(
                            "Backfill 在记录 {} 失败 (fail-fast): {}",
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
                Some("error_records"),
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

/// 尝试升级单条 error_record。
/// 返回 Ok(true) 表示有升级变更，Ok(false) 表示跳过（无需变更）。
///
/// 当前升级路径：v1 -> v2
/// - 如果 `pipeline_version` 为空，填为 `legacy-backfill-v1`
/// - 如果 `question_markdown_clean` 为空，回填 `original_question`
/// - bump `data_version = 2`
async fn upgrade_error_record(
    repository: &Repository,
    record: &crate::db::models::ErrorRecord,
    dry_run: bool,
) -> Result<bool> {
    if record.data_version >= LATEST_ERROR_RECORDS_DATA_VERSION {
        return Ok(false);
    }

    // stepwise 升级：当前只实现 v1 -> v2
    let upgraded = upgrade_v1_to_v2(record)?;

    if dry_run {
        tracing::info!(
            id = %record.id,
            "[dry-run] 将升级: pipeline_version={:?}, question_markdown_clean 回填={}",
            upgraded.new_pipeline_version,
            upgraded.backfilled_question_markdown
        );
        return Ok(true);
    }

    repository
        .update_error_record_backfill(
            &record.id,
            upgraded.new_question_markdown_clean.as_deref(),
            upgraded.new_pipeline_version.as_deref(),
            LATEST_ERROR_RECORDS_DATA_VERSION,
        )
        .await?;

    Ok(true)
}

/// v1 -> v2 升级结果
struct UpgradeV1ToV2Result {
    new_pipeline_version: Option<String>,
    new_question_markdown_clean: Option<String>,
    backfilled_question_markdown: bool,
}

/// v1 -> v2 升级逻辑：
/// - 若 `pipeline_version` 为 None 或空，设为 `legacy-backfill-v1`
/// - 若 `question_markdown_clean` 为 None 或空，回填 `original_question`
fn upgrade_v1_to_v2(record: &crate::db::models::ErrorRecord) -> Result<UpgradeV1ToV2Result> {
    let new_pipeline_version = match &record.pipeline_version {
        None => Some("legacy-backfill-v1".to_string()),
        Some(s) if s.trim().is_empty() => Some("legacy-backfill-v1".to_string()),
        _ => None, // 保持原值
    };

    let (new_question_markdown_clean, backfilled) =
        match &record.question_markdown_clean {
            None => {
                let clean = record.original_question.trim().to_string();
                if clean.is_empty() {
                    (None, false)
                } else {
                    (Some(clean), true)
                }
            }
            Some(s) if s.trim().is_empty() => {
                let clean = record.original_question.trim().to_string();
                if clean.is_empty() {
                    (None, false)
                } else {
                    (Some(clean), true)
                }
            }
            _ => (None, false), // 保持原值
        };

    Ok(UpgradeV1ToV2Result {
        new_pipeline_version,
        new_question_markdown_clean,
        backfilled_question_markdown: backfilled,
    })
}
