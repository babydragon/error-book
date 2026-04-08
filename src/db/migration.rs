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
