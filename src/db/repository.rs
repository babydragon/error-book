use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use chrono::NaiveDateTime;

use super::models::{AnalysisArtifact, AnalysisBackfillFields, BackfillRun, ErrorRecord, ErrorRecordWithScore, McpJob, PracticeSet, Summary, SummaryImage};

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
            "INSERT INTO error_records (id, image_path, subject, grade_level, original_question, image_regions, classification, error_reason, suggestions, text_embedding, image_embedding, created_at, question_markdown_clean, question_structure_json, student_answer_text, teacher_marks_json, question_type, difficulty, error_type, error_subtype, root_cause_code, confidence_json, pipeline_version, model_trace_json, data_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, vector32(?10), vector32(?11), ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
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
                // 结构化预留字段
                opt_to_value(&record.question_markdown_clean),
                opt_to_value(&record.question_structure_json),
                opt_to_value(&record.student_answer_text),
                opt_to_value(&record.teacher_marks_json),
                opt_to_value(&record.question_type),
                opt_to_value(&record.difficulty),
                opt_to_value(&record.error_type),
                opt_to_value(&record.error_subtype),
                opt_to_value(&record.root_cause_code),
                opt_to_value(&record.confidence_json),
                opt_to_value(&record.pipeline_version),
                opt_to_value(&record.model_trace_json),
                libsql::Value::from(record.data_version),
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
                "SELECT id, image_path, subject, grade_level, original_question, image_regions, classification, error_reason, suggestions, created_at, question_markdown_clean, question_structure_json, student_answer_text, teacher_marks_json, question_type, difficulty, error_type, error_subtype, root_cause_code, confidence_json, pipeline_version, model_trace_json, data_version FROM error_records WHERE id = ?1",
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
            "SELECT id, image_path, subject, grade_level, original_question, image_regions, classification, error_reason, suggestions, created_at, question_markdown_clean, question_structure_json, student_answer_text, teacher_marks_json, question_type, difficulty, error_type, error_subtype, root_cause_code, confidence_json, pipeline_version, model_trace_json, data_version FROM error_records {} ORDER BY created_at DESC {}",
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
                "SELECT e.id, e.image_path, e.subject, e.grade_level, e.original_question, e.image_regions, e.classification, e.error_reason, e.suggestions, e.created_at, e.question_markdown_clean, e.question_structure_json, e.student_answer_text, e.teacher_marks_json, e.question_type, e.difficulty, e.error_type, e.error_subtype, e.root_cause_code, e.confidence_json, e.pipeline_version, e.model_trace_json, e.data_version FROM error_records e JOIN error_classification_tags t ON e.id = t.error_id WHERE t.tag = ?1 ORDER BY e.created_at DESC",
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
            "SELECT id, image_path, subject, grade_level, original_question, image_regions, classification, error_reason, suggestions, created_at, question_markdown_clean, question_structure_json, student_answer_text, teacher_marks_json, question_type, difficulty, error_type, error_subtype, root_cause_code, confidence_json, pipeline_version, model_trace_json, data_version, \
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
            let distance: f64 = row.get(23)?;
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
            "SELECT id, image_path, subject, grade_level, original_question, image_regions, classification, error_reason, suggestions, created_at, question_markdown_clean, question_structure_json, student_answer_text, teacher_marks_json, question_type, difficulty, error_type, error_subtype, root_cause_code, confidence_json, pipeline_version, model_trace_json, data_version, \
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
            let distance: f64 = row.get(23)?;
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
            summary.data_version.into(),
            summary.generator_version.clone().map(libsql::Value::from).unwrap_or(libsql::Value::Null),
        ];
        conn.execute(
            "INSERT INTO summaries (id, subject, period_type, period_start, period_end, common_reasons, common_suggestions, weak_points, detail, related_error_ids, created_at, data_version, generator_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
                    "SELECT id, subject, period_type, period_start, period_end, common_reasons, common_suggestions, weak_points, detail, related_error_ids, created_at, data_version, generator_version FROM summaries WHERE subject = ?1 ORDER BY created_at DESC{}",
                    limit_clause
                ),
                vec![libsql::Value::from(s.to_string())],
            ),
            None => (
                format!(
                    "SELECT id, subject, period_type, period_start, period_end, common_reasons, common_suggestions, weak_points, detail, related_error_ids, created_at, data_version, generator_version FROM summaries ORDER BY created_at DESC{}",
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
                "SELECT id, subject, period_type, period_start, period_end, common_reasons, common_suggestions, weak_points, detail, related_error_ids, created_at, data_version, generator_version FROM summaries WHERE id = ?1",
                [id],
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(Some(row_to_summary(&row)?)),
            None => Ok(None),
        }
    }

    pub async fn insert_summary_image(&self, image: &SummaryImage) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO summary_images (id, summary_id, prompt, image_path, mime_type, created_at, data_version, generator_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            libsql::params_from_iter(vec![
                libsql::Value::from(image.id.clone()),
                libsql::Value::from(image.summary_id.clone()),
                libsql::Value::from(image.prompt.clone()),
                libsql::Value::from(image.image_path.clone()),
                libsql::Value::from(image.mime_type.clone()),
                libsql::Value::from(image.created_at),
                libsql::Value::from(image.data_version),
                image.generator_version.clone().map(libsql::Value::from).unwrap_or(libsql::Value::Null),
            ]),
        ).await?;
        tracing::info!(id = %image.id, summary_id = %image.summary_id, "总结信息图已保存");
        Ok(())
    }

    pub async fn list_summary_images(&self, summary_id: &str) -> Result<Vec<SummaryImage>> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT id, summary_id, prompt, image_path, mime_type, created_at, data_version, generator_version FROM summary_images WHERE summary_id = ?1 ORDER BY created_at DESC",
                [summary_id],
            )
            .await?;

        let mut items = Vec::new();
        while let Some(row) = rows.next().await? {
            items.push(row_to_summary_image(&row)?);
        }
        Ok(items)
    }

    // ===== Cascade Delete =====

    /// Delete all summary_images rows for a given summary_id. Returns count deleted.
    pub async fn delete_summary_images_by_summary(&self, summary_id: &str) -> Result<u64> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM summary_images WHERE summary_id = ?1",
            [summary_id],
        ).await?;
        // libsql execute returns rows affected via the changes() function
        // We need to query changes count separately
        let count = self.count_deleted(&conn).await?;
        tracing::info!(summary_id = %summary_id, count = count, "已删除 summary_images");
        Ok(count)
    }

    /// Delete all practice_sets rows for a given summary_id. Returns count deleted.
    pub async fn delete_practice_sets_by_summary(&self, summary_id: &str) -> Result<u64> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM practice_sets WHERE summary_id = ?1",
            [summary_id],
        ).await?;
        let count = self.count_deleted(&conn).await?;
        tracing::info!(summary_id = %summary_id, count = count, "已删除 practice_sets");
        Ok(count)
    }

    /// Delete a summary row by id.
    pub async fn delete_summary(&self, summary_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM summaries WHERE id = ?1",
            [summary_id],
        ).await?;
        tracing::info!(summary_id = %summary_id, "已删除 summary");
        Ok(())
    }

    /// Helper: get the number of rows affected by the last DELETE on this connection.
    async fn count_deleted(&self, conn: &libsql::Connection) -> Result<u64> {
        let mut rows = conn.query("SELECT changes()", ()).await?;
        match rows.next().await? {
            Some(row) => Ok(row.get::<i64>(0)? as u64),
            None => Ok(0),
        }
    }

    // ===== 巩固练习 ===== 

    /// 按 ID 查询练习集
    pub async fn get_practice_set(&self, id: &str) -> Result<Option<PracticeSet>> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT id, summary_id, subject, requirements, questions, pdf_path, created_at, data_version, generator_version FROM practice_sets WHERE id = ?1",
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
            "SELECT id, summary_id, subject, requirements, questions, pdf_path, created_at, data_version, generator_version FROM practice_sets {} ORDER BY created_at DESC {}",
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
            practice.data_version.into(),
            practice.generator_version.clone().map(libsql::Value::from).unwrap_or(libsql::Value::Null),
        ];
        conn.execute(
            "INSERT INTO practice_sets (id, summary_id, subject, requirements, questions, pdf_path, created_at, data_version, generator_version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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

    // ===== 分析产物 =====

    /// 插入分析产物
    pub async fn insert_analysis_artifact(&self, artifact: &AnalysisArtifact) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO analysis_artifacts (id, error_id, stage, schema_version, model_name, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params_from_iter(vec![
                libsql::Value::from(artifact.id.clone()),
                libsql::Value::from(artifact.error_id.clone()),
                libsql::Value::from(artifact.stage.clone()),
                libsql::Value::from(artifact.schema_version.clone()),
                opt_to_value(&artifact.model_name),
                libsql::Value::from(artifact.payload_json.clone()),
                libsql::Value::from(artifact.created_at),
            ]),
        ).await?;
        tracing::info!(id = %artifact.id, error_id = %artifact.error_id, stage = %artifact.stage, "分析产物已保存");
        Ok(())
    }

    /// 查询分析产物列表
    pub async fn list_analysis_artifacts(
        &self,
        error_id: &str,
        stage: Option<&str>,
    ) -> Result<Vec<AnalysisArtifact>> {
        let conn = self.conn()?;

        let (sql, params): (String, Vec<libsql::Value>) = match stage {
            Some(s) => (
                "SELECT id, error_id, stage, schema_version, model_name, payload_json, created_at FROM analysis_artifacts WHERE error_id = ?1 AND stage = ?2 ORDER BY created_at ASC".to_string(),
                vec![libsql::Value::from(error_id.to_string()), libsql::Value::from(s.to_string())],
            ),
            None => (
                "SELECT id, error_id, stage, schema_version, model_name, payload_json, created_at FROM analysis_artifacts WHERE error_id = ?1 ORDER BY created_at ASC".to_string(),
                vec![libsql::Value::from(error_id.to_string())],
            ),
        };

        let mut rows = conn.query(&sql, libsql::params_from_iter(params)).await?;

        let mut artifacts = Vec::new();
        while let Some(row) = rows.next().await? {
            artifacts.push(row_to_analysis_artifact(&row)?);
        }
        Ok(artifacts)
    }

    // ===== Backfill Run =====

    pub async fn insert_backfill_run(&self, run: &BackfillRun) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO backfill_runs (id, scope, status, dry_run, started_at, updated_at, completed_at, stats_json, error_message, last_table, last_row_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            libsql::params_from_iter(vec![
                libsql::Value::from(run.id.clone()),
                libsql::Value::from(run.scope.clone()),
                libsql::Value::from(run.status.clone()),
                libsql::Value::from(if run.dry_run { 1i64 } else { 0i64 }),
                libsql::Value::from(run.started_at),
                libsql::Value::from(run.updated_at),
                run.completed_at.map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                run.stats_json.clone().map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                run.error_message.clone().map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                run.last_table.clone().map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                run.last_row_id.clone().map(libsql::Value::from).unwrap_or(libsql::Value::Null),
            ]),
        ).await?;
        tracing::info!(id = %run.id, scope = %run.scope, dry_run = run.dry_run, "Backfill run 已创建");
        Ok(())
    }

    pub async fn update_backfill_run_progress(
        &self,
        id: &str,
        stats_json: &str,
        last_table: Option<&str>,
        last_row_id: Option<&str>,
        updated_at: i64,
    ) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE backfill_runs SET stats_json = ?1, last_table = ?2, last_row_id = ?3, updated_at = ?4 WHERE id = ?5",
            libsql::params_from_iter(vec![
                libsql::Value::from(stats_json.to_string()),
                last_table.map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                last_row_id.map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                libsql::Value::from(updated_at),
                libsql::Value::from(id.to_string()),
            ]),
        ).await?;
        Ok(())
    }

    pub async fn complete_backfill_run(
        &self,
        id: &str,
        stats_json: &str,
        completed_at: i64,
    ) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE backfill_runs SET status = 'completed', stats_json = ?1, updated_at = ?2, completed_at = ?2, error_message = NULL WHERE id = ?3",
            libsql::params_from_iter(vec![
                libsql::Value::from(stats_json.to_string()),
                libsql::Value::from(completed_at),
                libsql::Value::from(id.to_string()),
            ]),
        ).await?;
        tracing::info!(id = %id, "Backfill run 已完成");
        Ok(())
    }

    pub async fn fail_backfill_run(
        &self,
        id: &str,
        error_message: &str,
        stats_json: Option<&str>,
        completed_at: i64,
    ) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE backfill_runs SET status = 'failed', error_message = ?1, stats_json = ?2, updated_at = ?3, completed_at = ?3 WHERE id = ?4",
            libsql::params_from_iter(vec![
                libsql::Value::from(error_message.to_string()),
                stats_json.map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                libsql::Value::from(completed_at),
                libsql::Value::from(id.to_string()),
            ]),
        ).await?;
        tracing::info!(id = %id, "Backfill run 已标记为失败");
        Ok(())
    }

    pub async fn get_backfill_run(&self, id: &str) -> Result<Option<BackfillRun>> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT id, scope, status, dry_run, started_at, updated_at, completed_at, stats_json, error_message, last_table, last_row_id FROM backfill_runs WHERE id = ?1",
                [id],
            )
            .await?;

        match rows.next().await? {
            Some(row) => Ok(Some(row_to_backfill_run(&row)?)),
            None => Ok(None),
        }
    }

    pub async fn list_backfill_runs(
        &self,
        scope: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<BackfillRun>> {
        let conn = self.conn()?;
        let limit_clause = match limit {
            Some(l) => format!(" LIMIT {}", l),
            None => String::new(),
        };

        let (sql, params): (String, Vec<libsql::Value>) = match scope {
            Some(s) => (
                format!("SELECT id, scope, status, dry_run, started_at, updated_at, completed_at, stats_json, error_message, last_table, last_row_id FROM backfill_runs WHERE scope = ?1 ORDER BY started_at DESC{}", limit_clause),
                vec![libsql::Value::from(s.to_string())],
            ),
            None => (
                format!("SELECT id, scope, status, dry_run, started_at, updated_at, completed_at, stats_json, error_message, last_table, last_row_id FROM backfill_runs ORDER BY started_at DESC{}", limit_clause),
                Vec::new(),
            ),
        };

        let mut rows = if params.is_empty() {
            conn.query(&sql, ()).await?
        } else {
            conn.query(&sql, libsql::params_from_iter(params)).await?
        };

        let mut runs = Vec::new();
        while let Some(row) = rows.next().await? {
            runs.push(row_to_backfill_run(&row)?);
        }
        Ok(runs)
    }

    /// Backfill 专用：批量查询 data_version < target 的 error_records
    pub async fn list_error_records_for_backfill(
        &self,
        target_version: i64,
        limit: Option<u32>,
        after_id: Option<&str>,
    ) -> Result<Vec<ErrorRecord>> {
        let conn = self.conn()?;

        let mut conditions = vec![format!("data_version < {}", target_version)];
        let mut param_values: Vec<libsql::Value> = Vec::new();

        if let Some(aid) = after_id {
            conditions.push("id > ?1".to_string());
            param_values.push(libsql::Value::from(aid.to_string()));
        }

        let limit_clause = match limit {
            Some(l) => format!("LIMIT {}", l),
            None => String::new(),
        };

        let sql = format!(
            "SELECT id, image_path, subject, grade_level, original_question, image_regions, classification, error_reason, suggestions, created_at, question_markdown_clean, question_structure_json, student_answer_text, teacher_marks_json, question_type, difficulty, error_type, error_subtype, root_cause_code, confidence_json, pipeline_version, model_trace_json, data_version FROM error_records WHERE {} ORDER BY id ASC {}",
            conditions.join(" AND "),
            limit_clause
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

    /// Backfill 专用：更新单条 error_record 的指定字段
    pub async fn update_error_record_backfill(
        &self,
        id: &str,
        question_markdown_clean: Option<&str>,
        pipeline_version: Option<&str>,
        data_version: i64,
    ) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE error_records SET question_markdown_clean = ?1, pipeline_version = ?2, data_version = ?3 WHERE id = ?4",
            libsql::params_from_iter(vec![
                question_markdown_clean.map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                pipeline_version.map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                libsql::Value::from(data_version),
                libsql::Value::from(id.to_string()),
            ]),
        ).await?;
        Ok(())
    }

    /// Analysis backfill 专用：查询尚未填充结构化分析字段的 error_records
    /// 条件：question_type / error_type / difficulty 中至少一个为 NULL
    pub async fn list_error_records_for_analysis_backfill(
        &self,
        limit: Option<u32>,
        after_id: Option<&str>,
    ) -> Result<Vec<ErrorRecord>> {
        let conn = self.conn()?;

        let mut conditions = vec![
            "(question_type IS NULL OR error_type IS NULL OR difficulty IS NULL)".to_string()
        ];
        let mut param_values: Vec<libsql::Value> = Vec::new();

        if let Some(aid) = after_id {
            conditions.push("id > ?1".to_string());
            param_values.push(libsql::Value::from(aid.to_string()));
        }

        let limit_clause = match limit {
            Some(l) => format!("LIMIT {}", l),
            None => String::new(),
        };

        let sql = format!(
            "SELECT id, image_path, subject, grade_level, original_question, image_regions, classification, error_reason, suggestions, created_at, question_markdown_clean, question_structure_json, student_answer_text, teacher_marks_json, question_type, difficulty, error_type, error_subtype, root_cause_code, confidence_json, pipeline_version, model_trace_json, data_version FROM error_records WHERE {} ORDER BY id ASC {}",
            conditions.join(" AND "),
            limit_clause
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

    /// Analysis backfill 专用：仅更新结构化分析字段，不动 legacy 核心字段
    pub async fn update_error_record_analysis_backfill(
        &self,
        id: &str,
        fields: &AnalysisBackfillFields,
    ) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE error_records SET \
             question_markdown_clean = ?1, \
             question_structure_json = ?2, \
             student_answer_text = ?3, \
             teacher_marks_json = ?4, \
             question_type = ?5, \
             difficulty = ?6, \
             error_type = ?7, \
             error_subtype = ?8, \
             root_cause_code = ?9, \
             confidence_json = ?10, \
             pipeline_version = ?11, \
             model_trace_json = ?12, \
             data_version = ?13 \
             WHERE id = ?14",
            libsql::params_from_iter(vec![
                opt_to_value(&fields.question_markdown_clean),
                opt_to_value(&fields.question_structure_json),
                opt_to_value(&fields.student_answer_text),
                opt_to_value(&fields.teacher_marks_json),
                opt_to_value(&fields.question_type),
                opt_to_value(&fields.difficulty),
                opt_to_value(&fields.error_type),
                opt_to_value(&fields.error_subtype),
                opt_to_value(&fields.root_cause_code),
                opt_to_value(&fields.confidence_json),
                libsql::Value::from(fields.pipeline_version.clone()),
                opt_to_value(&fields.model_trace_json),
                libsql::Value::from(super::models::DATA_VERSION_CURRENT),
                libsql::Value::from(id.to_string()),
            ]),
        ).await?;
        tracing::info!(id = %id, "Analysis backfill 结构化字段已更新");
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
        // 结构化预留字段
        question_markdown_clean: row.get::<Option<String>>(10)?,
        question_structure_json: row.get::<Option<String>>(11)?,
        student_answer_text: row.get::<Option<String>>(12)?,
        teacher_marks_json: row.get::<Option<String>>(13)?,
        question_type: row.get::<Option<String>>(14)?,
        difficulty: row.get::<Option<String>>(15)?,
        error_type: row.get::<Option<String>>(16)?,
        error_subtype: row.get::<Option<String>>(17)?,
        root_cause_code: row.get::<Option<String>>(18)?,
        confidence_json: row.get::<Option<String>>(19)?,
        pipeline_version: row.get::<Option<String>>(20)?,
        model_trace_json: row.get::<Option<String>>(21)?,
        data_version: row.get::<i64>(22)?,
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
        data_version: row.get::<i64>(11)?,
        generator_version: row.get::<Option<String>>(12)?,
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
        data_version: row.get::<i64>(7)?,
        generator_version: row.get::<Option<String>>(8)?,
    })
}

fn row_to_summary_image(row: &libsql::Row) -> Result<SummaryImage> {
    Ok(SummaryImage {
        id: row.get::<String>(0)?,
        summary_id: row.get::<String>(1)?,
        prompt: row.get::<String>(2)?,
        image_path: row.get::<String>(3)?,
        mime_type: row.get::<String>(4)?,
        created_at: row.get::<i64>(5)?,
        data_version: row.get::<i64>(6)?,
        generator_version: row.get::<Option<String>>(7)?,
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

fn row_to_analysis_artifact(row: &libsql::Row) -> Result<AnalysisArtifact> {
    Ok(AnalysisArtifact {
        id: row.get::<String>(0)?,
        error_id: row.get::<String>(1)?,
        stage: row.get::<String>(2)?,
        schema_version: row.get::<String>(3)?,
        model_name: row.get::<Option<String>>(4)?,
        payload_json: row.get::<String>(5)?,
        created_at: row.get::<i64>(6)?,
    })
}

fn row_to_backfill_run(row: &libsql::Row) -> Result<BackfillRun> {
    Ok(BackfillRun {
        id: row.get::<String>(0)?,
        scope: row.get::<String>(1)?,
        status: row.get::<String>(2)?,
        dry_run: row.get::<i64>(3)? != 0,
        started_at: row.get::<i64>(4)?,
        updated_at: row.get::<i64>(5)?,
        completed_at: row.get::<Option<i64>>(6)?,
        stats_json: row.get::<Option<String>>(7)?,
        error_message: row.get::<Option<String>>(8)?,
        last_table: row.get::<Option<String>>(9)?,
        last_row_id: row.get::<Option<String>>(10)?,
    })
}

/// Helper: Option<String> -> libsql::Value (Null if None)
fn opt_to_value(opt: &Option<String>) -> libsql::Value {
    match opt {
        Some(s) => libsql::Value::from(s.clone()),
        None => libsql::Value::Null,
    }
}
