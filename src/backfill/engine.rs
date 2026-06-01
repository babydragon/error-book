use std::fmt;

use anyhow::Result;
use uuid::Uuid;

use crate::analysis::analyzer::Analyzer;
use crate::db::models::{BackfillRun, BackfillStats};
use crate::db::repository::Repository;

use super::error_records;
use super::error_records_analysis;

/// 支持的 backfill scope
#[derive(Debug, Clone, PartialEq)]
pub enum BackfillScope {
    ErrorRecords,
    /// LLM 驱动的结构化分析回填（用户主动触发，`all` 不包含）
    ErrorRecordsAnalysis,
    All,
}

impl BackfillScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ErrorRecords => "error-records",
            Self::ErrorRecordsAnalysis => "error-records-analysis",
            Self::All => "all",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "error-records" => Some(Self::ErrorRecords),
            "error-records-analysis" => Some(Self::ErrorRecordsAnalysis),
            "all" => Some(Self::All),
            _ => None,
        }
    }
}

impl fmt::Display for BackfillScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Backfill 运行参数
pub struct BackfillOptions {
    pub scope: BackfillScope,
    pub dry_run: bool,
    pub limit: Option<u64>,
    pub fail_fast: bool,
}

/// Backfill 运行结果
pub struct BackfillResult {
    pub run_id: String,
    pub scope: String,
    pub stats: BackfillStats,
    pub dry_run: bool,
}

/// 执行 backfill
/// `analyzer`: 仅 `ErrorRecordsAnalysis` scope 需要，其他 scope 传入 None 即可
pub async fn run_backfill(
    repository: &Repository,
    opts: BackfillOptions,
    analyzer: Option<&Analyzer>,
) -> Result<BackfillResult> {
    let now = chrono::Utc::now().timestamp();
    let run_id = Uuid::new_v4().to_string();

    // 创建 backfill run 记录
    let run = BackfillRun {
        id: run_id.clone(),
        scope: opts.scope.as_str().to_string(),
        status: "running".to_string(),
        dry_run: opts.dry_run,
        started_at: now,
        updated_at: now,
        completed_at: None,
        stats_json: None,
        error_message: None,
        last_table: None,
        last_row_id: None,
    };
    repository.insert_backfill_run(&run).await?;

    tracing::info!(
        id = %run_id,
        scope = %opts.scope,
        dry_run = opts.dry_run,
        "开始 backfill"
    );

    let result = match opts.scope {
        BackfillScope::ErrorRecords => {
            error_records::run_error_records_backfill(repository, &run_id, &opts).await
        }
        BackfillScope::ErrorRecordsAnalysis => {
            let analyzer = analyzer.ok_or_else(|| {
                anyhow::anyhow!(
                    "scope=error-records-analysis 需要 Analyzer 实例，内部调用错误"
                )
            })?;
            error_records_analysis::run_error_records_analysis_backfill(
                repository,
                analyzer,
                &run_id,
                &opts,
            )
            .await
        }
        BackfillScope::All => {
            // `all` is an explicit orchestrator: run only unattended-safe deterministic
            // backfills and skip user-facing generated artifacts that should not be
            // auto-migrated without human review.

            tracing::info!(
                id = %run_id,
                "BackfillScope::All — starting orchestrated run (unattended-safe scopes only)"
            );

            // Execute: error-records (deterministic schema/data migration)
            tracing::info!(
                id = %run_id,
                scope = "error-records",
                "executing scope: deterministic data-version backfill"
            );
            let stats = error_records::run_error_records_backfill(repository, &run_id, &opts).await?;

            // Explicitly skipped scopes — these are user-facing generated artifacts
            // and must not be auto-migrated unattended.
            tracing::warn!(
                id = %run_id,
                scope = "summaries",
                "skipped scope: summaries are user-facing generated artifacts — not auto-migrated"
            );
            tracing::warn!(
                id = %run_id,
                scope = "practice-sets",
                "skipped scope: practice-sets are user-facing generated artifacts — not auto-migrated"
            );
            tracing::warn!(
                id = %run_id,
                scope = "summary-images",
                "skipped scope: summary-images are user-facing generated artifacts — not auto-migrated"
            );
            tracing::warn!(
                id = %run_id,
                scope = "error-records-analysis",
                "skipped scope: error-records-analysis — LLM-backed, requires explicit user invocation"
            );

            tracing::info!(
                id = %run_id,
                "BackfillScope::All — orchestrated run complete"
            );

            Ok(stats)
        }
    };

    let completed_at = chrono::Utc::now().timestamp();

    match result {
        Ok(stats) => {
            let stats_json = serde_json::to_string(&stats)?;
            repository
                .complete_backfill_run(&run_id, &stats_json, completed_at)
                .await?;

            tracing::info!(
                id = %run_id,
                total = stats.total_scanned,
                upgraded = stats.upgraded,
                failed = stats.failed,
                "Backfill 完成"
            );

            Ok(BackfillResult {
                run_id,
                scope: opts.scope.as_str().to_string(),
                stats,
                dry_run: opts.dry_run,
            })
        }
        Err(e) => {
            tracing::error!(id = %run_id, error = %e, "Backfill 失败");
            repository
                .fail_backfill_run(&run_id, &e.to_string(), None, completed_at)
                .await?;
            Err(e)
        }
    }
}
