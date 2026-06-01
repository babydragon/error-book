use anyhow::Result;

/// 数据库迁移 SQL
pub const MIGRATION_SQL: &str = r#"
-- 错题记录表
CREATE TABLE IF NOT EXISTS error_records (
    id TEXT PRIMARY KEY,
    image_path TEXT NOT NULL,
    subject TEXT NOT NULL,
    grade_level TEXT NOT NULL DEFAULT '二年级',
    original_question TEXT NOT NULL,
    image_regions TEXT,
    classification TEXT NOT NULL,
    error_reason TEXT NOT NULL,
    suggestions TEXT NOT NULL,
    text_embedding F32_BLOB(1536),
    image_embedding F32_BLOB(1536),
    created_at INTEGER NOT NULL
);

-- 阶段性总结表
CREATE TABLE IF NOT EXISTS summaries (
    id TEXT PRIMARY KEY,
    subject TEXT NOT NULL,
    period_type TEXT NOT NULL,
    period_start INTEGER NOT NULL,
    period_end INTEGER NOT NULL,
    common_reasons TEXT NOT NULL,
    common_suggestions TEXT NOT NULL,
    weak_points TEXT NOT NULL,
    detail TEXT NOT NULL,
    related_error_ids TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

-- 巩固练习表
CREATE TABLE IF NOT EXISTS practice_sets (
    id TEXT PRIMARY KEY,
    summary_id TEXT NOT NULL REFERENCES summaries(id),
    subject TEXT NOT NULL,
    requirements TEXT,
    questions TEXT NOT NULL,
    pdf_path TEXT,
    created_at INTEGER NOT NULL
);

-- 总结信息图表
CREATE TABLE IF NOT EXISTS summary_images (
    id TEXT PRIMARY KEY,
    summary_id TEXT NOT NULL REFERENCES summaries(id),
    prompt TEXT NOT NULL,
    image_path TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

-- MCP 后台任务表
CREATE TABLE IF NOT EXISTS mcp_jobs (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    input_json TEXT NOT NULL,
    result_json TEXT,
    error_message TEXT,
    progress_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    started_at INTEGER,
    completed_at INTEGER
);

-- 分类标签子表
CREATE TABLE IF NOT EXISTS error_classification_tags (
    error_id TEXT NOT NULL REFERENCES error_records(id) ON DELETE CASCADE,
    tag TEXT NOT NULL
);

-- 索引
CREATE INDEX IF NOT EXISTS idx_errors_subject_time ON error_records(subject, created_at);
CREATE INDEX IF NOT EXISTS idx_errors_created_at ON error_records(created_at);
CREATE INDEX IF NOT EXISTS idx_summaries_subject ON summaries(subject, period_start);
CREATE INDEX IF NOT EXISTS idx_summary_images_summary_id ON summary_images(summary_id, created_at);
CREATE INDEX IF NOT EXISTS idx_ect_tag ON error_classification_tags(tag);
CREATE INDEX IF NOT EXISTS idx_ect_error_id ON error_classification_tags(error_id);
CREATE INDEX IF NOT EXISTS idx_mcp_jobs_kind_status ON mcp_jobs(kind, status, created_at);

-- 向量索引
CREATE INDEX IF NOT EXISTS idx_error_text_embedding ON error_records(libsql_vector_idx(text_embedding));
CREATE INDEX IF NOT EXISTS idx_error_image_embedding ON error_records(libsql_vector_idx(image_embedding));
"#;

/// 运行数据库迁移
pub async fn run_migration(db: &libsql::Database) -> Result<()> {
    let conn = db.connect()?;
    conn.execute_batch(MIGRATION_SQL).await?;
    ensure_practice_requirements_column(&conn).await?;
    ensure_structured_columns(&conn).await?;
    ensure_analysis_artifacts_table(&conn).await?;
    ensure_data_version_columns(&conn).await?;
    ensure_backfill_runs_table(&conn).await?;
    ensure_classification_tags_unique(&conn).await?;
    tracing::info!("数据库迁移完成");
    Ok(())
}

async fn ensure_practice_requirements_column(conn: &libsql::Connection) -> Result<()> {
    let mut rows = conn.query("PRAGMA table_info(practice_sets)", ()).await?;
    let mut has_requirements = false;

    while let Some(row) = rows.next().await? {
        let name: String = row.get(1)?;
        if name == "requirements" {
            has_requirements = true;
            break;
        }
    }

    if !has_requirements {
        conn.execute("ALTER TABLE practice_sets ADD COLUMN requirements TEXT", ())
            .await?;
        tracing::info!("已为 practice_sets 添加 requirements 列");
    }

    Ok(())
}

/// 为 error_records 增补结构化预留字段（nullable TEXT，兼容已有数据）
async fn ensure_structured_columns(conn: &libsql::Connection) -> Result<()> {
    let columns = [
        "question_markdown_clean",
        "question_structure_json",
        "student_answer_text",
        "teacher_marks_json",
        "question_type",
        "difficulty",
        "error_type",
        "error_subtype",
        "root_cause_code",
        "confidence_json",
        "pipeline_version",
        "model_trace_json",
    ];

    // 收集已有列名
    let mut rows = conn.query("PRAGMA table_info(error_records)", ()).await?;
    let mut existing: std::collections::HashSet<String> = std::collections::HashSet::new();
    while let Some(row) = rows.next().await? {
        let name: String = row.get(1)?;
        existing.insert(name);
    }

    for col in &columns {
        if !existing.contains(*col) {
            conn.execute(
                &format!("ALTER TABLE error_records ADD COLUMN {} TEXT", col),
                (),
            ).await?;
            tracing::info!(column = %col, "已为 error_records 添加列");
        }
    }

    Ok(())
}

/// 创建 analysis_artifacts 表及索引（幂等）
async fn ensure_analysis_artifacts_table(conn: &libsql::Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS analysis_artifacts (
            id TEXT PRIMARY KEY,
            error_id TEXT NOT NULL REFERENCES error_records(id) ON DELETE CASCADE,
            stage TEXT NOT NULL,
            schema_version TEXT NOT NULL,
            model_name TEXT,
            payload_json TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_aa_error_id ON analysis_artifacts(error_id);
        CREATE INDEX IF NOT EXISTS idx_aa_stage ON analysis_artifacts(stage);
        CREATE INDEX IF NOT EXISTS idx_aa_created_at ON analysis_artifacts(created_at);
        "#,
    ).await?;
    Ok(())
}

/// 为各表添加 data_version / generator_version 列（幂等补丁式 migration）
async fn ensure_data_version_columns(conn: &libsql::Connection) -> Result<()> {
    // error_records: data_version
    ensure_column(
        conn,
        "error_records",
        "data_version",
        "INTEGER NOT NULL DEFAULT 1",
    )
    .await?;

    // summaries: data_version + generator_version
    ensure_column(conn, "summaries", "data_version", "INTEGER NOT NULL DEFAULT 1").await?;
    ensure_column(conn, "summaries", "generator_version", "TEXT").await?;

    // practice_sets: data_version + generator_version
    ensure_column(
        conn,
        "practice_sets",
        "data_version",
        "INTEGER NOT NULL DEFAULT 1",
    )
    .await?;
    ensure_column(
        conn,
        "practice_sets",
        "generator_version",
        "TEXT",
    )
    .await?;

    // summary_images: data_version + generator_version
    ensure_column(
        conn,
        "summary_images",
        "data_version",
        "INTEGER NOT NULL DEFAULT 1",
    )
    .await?;
    ensure_column(
        conn,
        "summary_images",
        "generator_version",
        "TEXT",
    )
    .await?;

    Ok(())
}

/// 创建 backfill_runs 表（幂等）
async fn ensure_backfill_runs_table(conn: &libsql::Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS backfill_runs (
            id TEXT PRIMARY KEY,
            scope TEXT NOT NULL,
            status TEXT NOT NULL,
            dry_run INTEGER NOT NULL,
            started_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            completed_at INTEGER,
            stats_json TEXT,
            error_message TEXT,
            last_table TEXT,
            last_row_id TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_backfill_runs_scope ON backfill_runs(scope);
        CREATE INDEX IF NOT EXISTS idx_backfill_runs_status ON backfill_runs(status);
        "#,
    )
    .await?;
    Ok(())
}

/// 为 error_classification_tags 添加去重保障（幂等）
async fn ensure_classification_tags_unique(conn: &libsql::Connection) -> Result<()> {
    // 检查是否已有唯一索引；如果没有则尝试创建
    // SQLite 不支持 IF NOT EXISTS 用于索引名与普通索引区分，
    // 所以用 try-catch 风格：先查 pragma 再决定
    let mut rows = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='error_classification_tags'",
            (),
        )
        .await?;

    let mut has_unique = false;
    while let Some(row) = rows.next().await? {
        let name: String = row.get(0)?;
        if name.contains("unique") || name.contains("uniq") {
            has_unique = true;
            break;
        }
    }

    if !has_unique {
        // 尝试创建唯一索引；如果已有重复数据会失败，记录警告后跳过
        match conn
            .execute(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_ect_unique ON error_classification_tags(error_id, tag)",
                (),
            )
            .await
        {
            Ok(_) => {
                tracing::info!("已为 error_classification_tags 创建唯一索引");
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "无法为 error_classification_tags 创建唯一索引（可能存在重复数据），跳过"
                );
            }
        }
    }

    Ok(())
}

/// 通用辅助：为指定表添加列（如果不存在）
async fn ensure_column(
    conn: &libsql::Connection,
    table: &str,
    column: &str,
    col_type: &str,
) -> Result<()> {
    let mut rows = conn
        .query(&format!("PRAGMA table_info({})", table), ())
        .await?;

    let mut exists = false;
    while let Some(row) = rows.next().await? {
        let name: String = row.get(1)?;
        if name == column {
            exists = true;
            break;
        }
    }

    if !exists {
        conn.execute(
            &format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, col_type),
            (),
        )
        .await?;
        tracing::info!(table = %table, column = %column, "已添加列");
    }

    Ok(())
}
