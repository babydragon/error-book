use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use chrono::NaiveDateTime;

use super::models::{ErrorRecord, ErrorRecordWithScore, McpJob, PracticeSet, Summary};

/// 数据访问层
#[derive(Clone)]
pub struct Repository {
    db: Arc<libsql::Database>,
}

impl Repository {
    pub fn new(db: Arc<libsql::Database>) -> Self {
        Self { db }
    }

    pub fn conn(&self) -> Result<libsql::Connection> {
        Ok(self.db.connect()?)
    }

    // ===== 错题记录 =====

    /// 插入错题记录 + 分类标签 + 两个 embedding
    pub async fn insert_error_record(
        &self,
        record: &ErrorRecord,
        tags: &[String],
    ) -> Result<()> {
        let conn = self.conn()?;

        let text_embedding_json = serde_json::to_string(&record.text_embedding)?;
        let image_embedding_json = serde_json::to_string(&record.image_embedding)?;

        conn.execute(
            "INSERT INTO error_records (id, image_path, subject, grade_level, original_question, image_regions, classification, error_reason, suggestions, text_embedding, image_embedding, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, vector32(?10), vector32(?11), ?12)",
            libsql::params_from_iter(vec![
                libsql::Value::from(record.id.clone()),
                libsql::Value::from(record.image_path.clone()),
                libsql::Value::from(record.subject.clone()),
                libsql::Value::from(record.grade_level.clone()),
                libsql::Value::from(record.original_question.clone()),
                record.image_regions.clone().map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                libsql::Value::from(record.classification.clone()),
                libsql::Value::from(record.error_reason.clone()),
                libsql::Value::from(record.suggestions.clone()),
                libsql::Value::from(text_embedding_json),
                libsql::Value::from(image_embedding_json),
                libsql::Value::from(record.created_at),
            ]),
        ).await?;

        // 同步写入分类标签子表
        for tag in tags {
            conn.execute(
                "INSERT INTO error_classification_tags (error_id, tag) VALUES (?1, ?2)",
                libsql::params_from_iter(vec![
                    libsql::Value::from(record.id.clone()),
                    libsql::Value::from(tag.clone()),
                ]),
            ).await?;
        }

        tracing::info!(id = %record.id, "错题记录已保存");
        Ok(())
    }

    /// 按 ID 查询错题
    pub async fn get_error_record(&self, id: &str) -> Result<Option<ErrorRecord>> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT id, image_path, subject, grade_level, original_question, image_regions, classification, error_reason, suggestions, created_at FROM error_records WHERE id = ?1",
                [id],
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(Some(row_to_error_record(&row)?)),
            None => Ok(None),
        }
    }

    /// 按科目 + 时间范围查询错题
    pub async fn list_error_records(
        &self,
        subject: Option<&str>,
        from: Option<NaiveDateTime>,
        to: Option<NaiveDateTime>,
        limit: Option<u32>,
    ) -> Result<Vec<ErrorRecord>> {
        let conn = self.conn()?;

        let mut conditions = Vec::new();
        let mut param_values: Vec<libsql::Value> = Vec::new();
        let mut param_idx = 1;

        if let Some(s) = subject {
            conditions.push(format!("subject = ?{}", param_idx));
            param_values.push(libsql::Value::from(s.to_string()));
            param_idx += 1;
        }
        if let Some(f) = from {
            conditions.push(format!("created_at >= ?{}", param_idx));
            param_values.push(libsql::Value::from(f.and_utc().timestamp()));
            param_idx += 1;
        }
        if let Some(t) = to {
            conditions.push(format!("created_at <= ?{}", param_idx));
            param_values.push(libsql::Value::from(t.and_utc().timestamp()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let limit_clause = match limit {
            Some(l) => format!("LIMIT {}", l),
            None => String::new(),
        };

        let sql = format!(
            "SELECT id, image_path, subject, grade_level, original_question, image_regions, classification, error_reason, suggestions, created_at FROM error_records {} ORDER BY created_at DESC {}",
            where_clause, limit_clause
        );

        let mut rows = if param_values.is_empty() {
            conn.query(&sql, ()).await?
        } else {
            conn.query(&sql, libsql::params_from_iter(param_values)).await?
        };

        let mut records = Vec::new();
        while let Some(row) = rows.next().await? {
            records.push(row_to_error_record(&row)?);
        }
        Ok(records)
    }

    /// 按分类标签查询错题（通过子表 JOIN）
    pub async fn list_errors_by_tag(&self, tag: &str) -> Result<Vec<ErrorRecord>> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT e.id, e.image_path, e.subject, e.grade_level, e.original_question, e.image_regions, e.classification, e.error_reason, e.suggestions, e.created_at FROM error_records e JOIN error_classification_tags t ON e.id = t.error_id WHERE t.tag = ?1 ORDER BY e.created_at DESC",
                [tag],
            )
            .await?;

        let mut records = Vec::new();
        while let Some(row) = rows.next().await? {
            records.push(row_to_error_record(&row)?);
        }
        Ok(records)
    }

    // ===== 向量搜索 =====

    /// 按文本向量搜索（暴力扫描，数据量小时足够高效）
    pub async fn search_by_text_vector(
        &self,
        query_embedding: &[f32],
        limit: u32,
        subject: Option<&str>,
    ) -> Result<Vec<ErrorRecordWithScore>> {
        let conn = self.conn()?;
        let query_json = serde_json::to_string(query_embedding)?;

        let subject_filter = match subject {
            Some(s) => format!("WHERE subject = '{}'", s.replace('\'', "''")),
            None => String::new(),
        };

        let sql = format!(
            "SELECT id, image_path, subject, grade_level, original_question, image_regions, classification, error_reason, suggestions, created_at, \
             vector_distance_cos(text_embedding, vector32(?1)) AS distance \
             FROM error_records {subject_filter} \
             ORDER BY distance ASC LIMIT {limit}",
            subject_filter = subject_filter,
            limit = limit,
        );

        let mut rows = conn.query(&sql, [query_json]).await?;

        let mut results = Vec::new();
        while let Some(row) = rows.next().await? {
            let record = row_to_error_record(&row)?;
            let distance: f64 = row.get(10)?;
            results.push(ErrorRecordWithScore { record, distance });
        }
        Ok(results)
    }

    /// 按图片向量搜索（暴力扫描）
    pub async fn search_by_image_vector(
        &self,
        query_embedding: &[f32],
        limit: u32,
        subject: Option<&str>,
    ) -> Result<Vec<ErrorRecordWithScore>> {
        let conn = self.conn()?;
        let query_json = serde_json::to_string(query_embedding)?;

        let subject_filter = match subject {
            Some(s) => format!("WHERE subject = '{}'", s.replace('\'', "''")),
            None => String::new(),
        };

        let sql = format!(
            "SELECT id, image_path, subject, grade_level, original_question, image_regions, classification, error_reason, suggestions, created_at, \
             vector_distance_cos(image_embedding, vector32(?1)) AS distance \
             FROM error_records {subject_filter} \
             ORDER BY distance ASC LIMIT {limit}",
            subject_filter = subject_filter,
            limit = limit,
        );

        let mut rows = conn.query(&sql, [query_json]).await?;

        let mut results = Vec::new();
        while let Some(row) = rows.next().await? {
            let record = row_to_error_record(&row)?;
            let distance: f64 = row.get(10)?;
            results.push(ErrorRecordWithScore { record, distance });
        }
        Ok(results)
    }

    /// 混合搜索：文本向量 + 图片向量加权融合
    /// text_weight: 文本相似度权重 (0.0~1.0)，图片权重 = 1.0 - text_weight
    pub async fn search_mixed(
        &self,
        text_query: Option<&[f32]>,
        image_query: Option<&[f32]>,
        text_weight: f64,
        limit: u32,
        subject: Option<&str>,
    ) -> Result<Vec<ErrorRecordWithScore>> {
        let fetch_limit = limit * 3; // 多取一些用于融合排序

        // 分别搜索
        let mut text_results = Vec::new();
        let mut image_results = Vec::new();

        if let Some(text_emb) = text_query {
            text_results = self.search_by_text_vector(text_emb, fetch_limit, subject).await?;
        }
        if let Some(image_emb) = image_query {
            image_results = self.search_by_image_vector(image_emb, fetch_limit, subject).await?;
        }

        // 融合：按 id 合并分数
        // 相似度 = text_weight * text_similarity + image_weight * image_similarity
        let image_weight = 1.0 - text_weight;
        let mut scores: HashMap<String, f64> = HashMap::new();

        for r in &text_results {
            let sim = r.similarity();
            *scores.entry(r.record.id.clone()).or_insert(0.0) += text_weight * sim;
        }
        for r in &image_results {
            let sim = r.similarity();
            *scores.entry(r.record.id.clone()).or_insert(0.0) += image_weight * sim;
        }

        // 用文本结果作为基础记录（图片结果中补充缺失的）
        let mut record_map: HashMap<String, ErrorRecord> = HashMap::new();
        for r in text_results.iter().chain(image_results.iter()) {
            record_map.entry(r.record.id.clone()).or_insert_with(|| r.record.clone());
        }

        // 排序
        let mut fused: Vec<(String, f64)> = scores.into_iter().collect();
        fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        fused.truncate(limit as usize);

        // 构造结果（distance = 1 - fused_score，因为 distance 是余弦距离）
        let results: Vec<ErrorRecordWithScore> = fused
            .into_iter()
            .filter_map(|(id, score)| {
                record_map.remove(&id).map(|record| ErrorRecordWithScore {
                    record,
                    distance: 1.0 - score,
                })
            })
            .collect();

        Ok(results)
    }

    // ===== 总结 =====

    /// 插入总结
    pub async fn insert_summary(&self, summary: &Summary) -> Result<()> {
        let conn = self.conn()?;
        let param_values: Vec<libsql::Value> = vec![
            summary.id.clone().into(),
            summary.subject.clone().into(),
            summary.period_type.clone().into(),
            summary.period_start.into(),
            summary.period_end.into(),
            summary.common_reasons.clone().into(),
            summary.common_suggestions.clone().into(),
            summary.weak_points.clone().into(),
            summary.detail.clone().into(),
            summary.related_error_ids.clone().into(),
            summary.created_at.into(),
        ];
        conn.execute(
            "INSERT INTO summaries (id, subject, period_type, period_start, period_end, common_reasons, common_suggestions, weak_points, detail, related_error_ids, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            libsql::params_from_iter(param_values),
        ).await?;
        tracing::info!(id = %summary.id, "总结已保存");
        Ok(())
    }

    /// 按科目查询总结列表
    pub async fn list_summaries(&self, subject: Option<&str>, limit: Option<u32>) -> Result<Vec<Summary>> {
        let conn = self.conn()?;

        let limit_clause = match limit {
            Some(l) => format!(" LIMIT {}", l),
            None => String::new(),
        };

        let (sql, param_values): (String, Vec<libsql::Value>) = match subject {
            Some(s) => (
                format!(
                    "SELECT id, subject, period_type, period_start, period_end, common_reasons, common_suggestions, weak_points, detail, related_error_ids, created_at FROM summaries WHERE subject = ?1 ORDER BY created_at DESC{}",
                    limit_clause
                ),
                vec![libsql::Value::from(s.to_string())],
            ),
            None => (
                format!(
                    "SELECT id, subject, period_type, period_start, period_end, common_reasons, common_suggestions, weak_points, detail, related_error_ids, created_at FROM summaries ORDER BY created_at DESC{}",
                    limit_clause
                ),
                Vec::new(),
            ),
        };

        let mut rows = if param_values.is_empty() {
            conn.query(&sql, ()).await?
        } else {
            conn.query(&sql, libsql::params_from_iter(param_values)).await?
        };

        let mut summaries = Vec::new();
        while let Some(row) = rows.next().await? {
            summaries.push(row_to_summary(&row)?);
        }
        Ok(summaries)
    }

    /// 按 ID 查询总结
    pub async fn get_summary(&self, id: &str) -> Result<Option<Summary>> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT id, subject, period_type, period_start, period_end, common_reasons, common_suggestions, weak_points, detail, related_error_ids, created_at FROM summaries WHERE id = ?1",
                [id],
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(Some(row_to_summary(&row)?)),
            None => Ok(None),
        }
    }

    // ===== 巩固练习 =====

    /// 按 ID 查询练习集
    pub async fn get_practice_set(&self, id: &str) -> Result<Option<PracticeSet>> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT id, summary_id, subject, requirements, questions, pdf_path, created_at FROM practice_sets WHERE id = ?1",
                [id],
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(Some(row_to_practice_set(&row)?)),
            None => Ok(None),
        }
    }

    /// 查询练习集列表
    pub async fn list_practice_sets(
        &self,
        subject: Option<&str>,
        summary_id: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<PracticeSet>> {
        let conn = self.conn()?;

        let mut conditions = Vec::new();
        let mut param_values: Vec<libsql::Value> = Vec::new();
        let mut param_idx = 1;

        if let Some(s) = subject {
            conditions.push(format!("subject = ?{}", param_idx));
            param_values.push(libsql::Value::from(s.to_string()));
            param_idx += 1;
        }
        if let Some(id) = summary_id {
            conditions.push(format!("summary_id = ?{}", param_idx));
            param_values.push(libsql::Value::from(id.to_string()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let limit_clause = match limit {
            Some(l) => format!("LIMIT {}", l),
            None => String::new(),
        };

        let sql = format!(
            "SELECT id, summary_id, subject, requirements, questions, pdf_path, created_at FROM practice_sets {} ORDER BY created_at DESC {}",
            where_clause, limit_clause
        );

        let mut rows = if param_values.is_empty() {
            conn.query(&sql, ()).await?
        } else {
            conn.query(&sql, libsql::params_from_iter(param_values)).await?
        };

        let mut practices = Vec::new();
        while let Some(row) = rows.next().await? {
            practices.push(row_to_practice_set(&row)?);
        }
        Ok(practices)
    }

    /// 更新练习集的 pdf_path
    pub async fn update_practice_set_pdf_path(&self, id: &str, pdf_path: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE practice_sets SET pdf_path = ?1 WHERE id = ?2",
            libsql::params_from_iter(vec![
                libsql::Value::from(pdf_path.to_string()),
                libsql::Value::from(id.to_string()),
            ]),
        ).await?;
        tracing::info!(id = %id, path = %pdf_path, "练习集 pdf_path 已更新");
        Ok(())
    }

    /// 插入练习集
    pub async fn insert_practice_set(&self, practice: &PracticeSet) -> Result<()> {
        let conn = self.conn()?;
        let param_values: Vec<libsql::Value> = vec![
            practice.id.clone().into(),
            practice.summary_id.clone().into(),
            practice.subject.clone().into(),
            practice.requirements.clone().map(libsql::Value::from).unwrap_or(libsql::Value::Null),
            practice.questions.clone().into(),
            practice.pdf_path.clone().map(libsql::Value::from).unwrap_or(libsql::Value::Null),
            practice.created_at.into(),
        ];
        conn.execute(
            "INSERT INTO practice_sets (id, summary_id, subject, requirements, questions, pdf_path, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params_from_iter(param_values),
        ).await?;
        tracing::info!(id = %practice.id, "练习集已保存");
        Ok(())
    }

    // ===== MCP 后台任务 =====

    pub async fn insert_mcp_job(&self, job: &McpJob) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO mcp_jobs (id, kind, status, input_json, result_json, error_message, progress_message, created_at, updated_at, started_at, completed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            libsql::params_from_iter(vec![
                libsql::Value::from(job.id.clone()),
                libsql::Value::from(job.kind.clone()),
                libsql::Value::from(job.status.clone()),
                libsql::Value::from(job.input_json.clone()),
                job.result_json.clone().map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                job.error_message.clone().map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                job.progress_message.clone().map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                libsql::Value::from(job.created_at),
                libsql::Value::from(job.updated_at),
                job.started_at.map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                job.completed_at.map(libsql::Value::from).unwrap_or(libsql::Value::Null),
            ]),
        ).await?;
        Ok(())
    }

    pub async fn get_mcp_job(&self, id: &str) -> Result<Option<McpJob>> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT id, kind, status, input_json, result_json, error_message, progress_message, created_at, updated_at, started_at, completed_at FROM mcp_jobs WHERE id = ?1",
                [id],
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(Some(row_to_mcp_job(&row)?)),
            None => Ok(None),
        }
    }

    pub async fn list_mcp_jobs(&self, kind: Option<&str>, limit: Option<u32>) -> Result<Vec<McpJob>> {
        let conn = self.conn()?;
        let limit_clause = match limit {
            Some(l) => format!(" LIMIT {}", l),
            None => String::new(),
        };

        let (sql, params): (String, Vec<libsql::Value>) = match kind {
            Some(kind) => (
                format!("SELECT id, kind, status, input_json, result_json, error_message, progress_message, created_at, updated_at, started_at, completed_at FROM mcp_jobs WHERE kind = ?1 ORDER BY created_at DESC{}", limit_clause),
                vec![libsql::Value::from(kind.to_string())],
            ),
            None => (
                format!("SELECT id, kind, status, input_json, result_json, error_message, progress_message, created_at, updated_at, started_at, completed_at FROM mcp_jobs ORDER BY created_at DESC{}", limit_clause),
                Vec::new(),
            ),
        };

        let mut rows = if params.is_empty() {
            conn.query(&sql, ()).await?
        } else {
            conn.query(&sql, libsql::params_from_iter(params)).await?
        };
        let mut jobs = Vec::new();
        while let Some(row) = rows.next().await? {
            jobs.push(row_to_mcp_job(&row)?);
        }
        Ok(jobs)
    }

    pub async fn mark_mcp_job_running(&self, id: &str, progress: Option<&str>, started_at: i64) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE mcp_jobs SET status = 'running', progress_message = ?1, started_at = ?2, updated_at = ?2 WHERE id = ?3",
            libsql::params_from_iter(vec![
                progress.map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                libsql::Value::from(started_at),
                libsql::Value::from(id.to_string()),
            ]),
        ).await?;
        Ok(())
    }

    pub async fn update_mcp_job_progress(&self, id: &str, progress: &str, updated_at: i64) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE mcp_jobs SET progress_message = ?1, updated_at = ?2 WHERE id = ?3",
            libsql::params_from_iter(vec![
                libsql::Value::from(progress.to_string()),
                libsql::Value::from(updated_at),
                libsql::Value::from(id.to_string()),
            ]),
        ).await?;
        Ok(())
    }

    pub async fn complete_mcp_job(&self, id: &str, result_json: &str, completed_at: i64) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE mcp_jobs SET status = 'succeeded', result_json = ?1, error_message = NULL, progress_message = NULL, updated_at = ?2, completed_at = ?2 WHERE id = ?3",
            libsql::params_from_iter(vec![
                libsql::Value::from(result_json.to_string()),
                libsql::Value::from(completed_at),
                libsql::Value::from(id.to_string()),
            ]),
        ).await?;
        Ok(())
    }

    pub async fn fail_mcp_job(&self, id: &str, error_message: &str, completed_at: i64) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE mcp_jobs SET status = 'failed', error_message = ?1, progress_message = NULL, updated_at = ?2, completed_at = ?2 WHERE id = ?3",
            libsql::params_from_iter(vec![
                libsql::Value::from(error_message.to_string()),
                libsql::Value::from(completed_at),
                libsql::Value::from(id.to_string()),
            ]),
        ).await?;
        Ok(())
    }
}

/// 从 Row 构造 ErrorRecord（不含 embedding，避免大量数据传输）
fn row_to_error_record(row: &libsql::Row) -> Result<ErrorRecord> {
    Ok(ErrorRecord {
        id: row.get::<String>(0)?,
        image_path: row.get::<String>(1)?,
        subject: row.get::<String>(2)?,
        grade_level: row.get::<String>(3)?,
        original_question: row.get::<String>(4)?,
        image_regions: row.get::<Option<String>>(5)?,
        classification: row.get::<String>(6)?,
        error_reason: row.get::<String>(7)?,
        suggestions: row.get::<String>(8)?,
        text_embedding: Vec::new(),
        image_embedding: Vec::new(),
        created_at: row.get::<i64>(9)?,
    })
}

/// 从 Row 构造 Summary
fn row_to_summary(row: &libsql::Row) -> Result<Summary> {
    Ok(Summary {
        id: row.get::<String>(0)?,
        subject: row.get::<String>(1)?,
        period_type: row.get::<String>(2)?,
        period_start: row.get::<i64>(3)?,
        period_end: row.get::<i64>(4)?,
        common_reasons: row.get::<String>(5)?,
        common_suggestions: row.get::<String>(6)?,
        weak_points: row.get::<String>(7)?,
        detail: row.get::<String>(8)?,
        related_error_ids: row.get::<String>(9)?,
        created_at: row.get::<i64>(10)?,
    })
}

/// 从 Row 构造 PracticeSet
fn row_to_practice_set(row: &libsql::Row) -> Result<PracticeSet> {
    Ok(PracticeSet {
        id: row.get::<String>(0)?,
        summary_id: row.get::<String>(1)?,
        subject: row.get::<String>(2)?,
        requirements: row.get::<Option<String>>(3)?,
        questions: row.get::<String>(4)?,
        pdf_path: row.get::<Option<String>>(5)?,
        created_at: row.get::<i64>(6)?,
    })
}

fn row_to_mcp_job(row: &libsql::Row) -> Result<McpJob> {
    Ok(McpJob {
        id: row.get::<String>(0)?,
        kind: row.get::<String>(1)?,
        status: row.get::<String>(2)?,
        input_json: row.get::<String>(3)?,
        result_json: row.get::<Option<String>>(4)?,
        error_message: row.get::<Option<String>>(5)?,
        progress_message: row.get::<Option<String>>(6)?,
        created_at: row.get::<i64>(7)?,
        updated_at: row.get::<i64>(8)?,
        started_at: row.get::<Option<i64>>(9)?,
        completed_at: row.get::<Option<i64>>(10)?,
    })
}
