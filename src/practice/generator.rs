use anyhow::{Context, Result};
use uuid::Uuid;

use crate::analysis::parser::PracticeQuestion;
use crate::config::{AppConfig, RoleKind};
use crate::db::models::{PracticeSet, DATA_VERSION_CURRENT};
use crate::db::repository::Repository;
use crate::llm::client::ChatClient;
use crate::practice::image_generator::PracticeImageGenerator;
use crate::practice::planner::{heuristic_plan, PracticeImageRole, PracticePlanner};
use crate::storage::image::ImageStorage;

/// Default number of questions requested per batch.
/// 为了优先保证长题干场景稳定性，practice generation 当前固定单题分批。
const DEFAULT_BATCH_SIZE: u32 = 1;

/// 巩固练习生成器
pub struct PracticeGenerator {
    config: AppConfig,
    chat_client: ChatClient,
    repository: Repository,
    generated_image_storage: ImageStorage,
}

/// 计算下一批的题目数量。
/// 策略：每批最多 `DEFAULT_BATCH_SIZE` 题；剩余不足时取剩余数。
/// 这是 `pub(crate)` 以便测试可直接调用。
pub(crate) fn next_batch_size(remaining: u32) -> u32 {
    remaining.min(DEFAULT_BATCH_SIZE)
}

impl PracticeGenerator {
    pub fn new(
        config: AppConfig,
        chat_client: ChatClient,
        repository: Repository,
        generated_image_storage: ImageStorage,
    ) -> Self {
        Self { config, chat_client, repository, generated_image_storage }
    }

    /// 生成巩固练习题
    /// 1. 读取总结（获取薄弱知识点 + 原题参考）
    /// 2. 循环分批调用 LLM，每批 1 题，直到凑满 count
    /// 3. 每批携带已有题目避免重复
    /// 4. 存储练习集
    pub async fn generate(
        &self,
        summary_id: &str,
        count: u32,
        requirements: Option<&str>,
        pdf_path: Option<&str>,
    ) -> Result<PracticeSet> {
        // 1. 读取总结
        let summary = self.repository.get_summary(summary_id).await?
            .ok_or_else(|| anyhow::anyhow!("总结记录不存在: {}", summary_id))?;

        tracing::info!(id = %summary.id, subject = %summary.subject, total = count, "读取总结，开始分批生成练习题");

        // 2. 获取参考题目
        let related_ids: Vec<String> = serde_json::from_str(&summary.related_error_ids)
            .unwrap_or_default();
        let mut reference_questions = Vec::new();
        for rid in related_ids.iter().take(5) {
            if let Some(record) = self.repository.get_error_record(rid).await? {
                let classification: Vec<String> = serde_json::from_str(&record.classification)
                    .unwrap_or_default();
                let question_preview = truncate_chars(&record.original_question, 400);
                reference_questions.push(format!(
                    "知识点: {}\n原题: {}",
                    classification.join("、"),
                    question_preview,
                ));
            }
        }
        // 3. 解析薄弱知识点
        let weak_points: Vec<String> = serde_json::from_str(&summary.weak_points)
            .unwrap_or_default();
        if weak_points.is_empty() {
            anyhow::bail!("总结中没有薄弱知识点，无法生成练习题");
        }

        let requirements = normalize_requirements(requirements);
        let planner = PracticePlanner::new(self.config.clone(), self.chat_client.clone());
        let plan = match planner
            .plan(
                &summary,
                &weak_points,
                &reference_questions,
                count,
                requirements.as_deref(),
                &[],
            )
            .await
        {
            Ok(plan) => plan,
            Err(err) => {
                tracing::warn!(error = %err, "练习题规划失败，回退到启发式规划");
                heuristic_plan(&summary.subject, &self.config.defaults.grade_level, &weak_points, count)
            }
        };
        if plan.items.is_empty() {
            anyhow::bail!("练习题规划为空，无法继续生成");
        }
        tracing::info!(plan = ?plan, "练习题规划完成");

        // 4. 分批生成
        let grade_level = &self.config.defaults.grade_level;
        let mut all_questions: Vec<PracticeQuestion> = Vec::new();
        let mut batch_index: u32 = 0;
        let image_generator = PracticeImageGenerator::new(
            self.config.clone(),
            self.repository.clone(),
            self.generated_image_storage.clone(),
        );

        while all_questions.len() < count as usize {
            let remaining = count as usize - all_questions.len();
            let batch_count = next_batch_size(remaining as u32);
            let already = all_questions.len();
            let plan_item = plan
                .items
                .get(already)
                .cloned()
                .unwrap_or_else(|| plan.items.last().cloned().expect("practice plan should have at least one item"));
            let target_points = std::iter::once(plan_item.primary_point.clone())
                .chain(plan_item.secondary_points.iter().cloned())
                .collect::<Vec<_>>();
            let reference_for_batch = select_reference_questions(&reference_questions, &target_points);

            tracing::info!(
                batch = batch_index,
                requested = batch_count,
                already_generated = already,
                remaining,
                target_points = %target_points.join("、"),
                "开始生成第 {} 批练习题",
                batch_index + 1,
            );

            let messages = crate::llm::prompts::build_practice_batch_prompt(
                &summary.subject,
                grade_level,
                &plan_item,
                &reference_for_batch,
                &all_questions,
                batch_count,
                requirements.as_deref(),
            );

            let raw_response = self.chat_client.chat_with_role(
                &self.config.llm,
                RoleKind::PracticeGeneration,
                messages,
                Some(0.2),
            ).await.map_err(|e| {
                anyhow::anyhow!(
                    "第 {} 批练习题 LLM 请求失败 (batch_index={}, requested={}, already_generated={}): {}",
                    batch_index + 1, batch_index, batch_count, already, e
                )
            })?;

            tracing::debug!(
                batch = batch_index,
                response_len = raw_response.len(),
                "LLM 练习题响应"
            );

            let mut batch_questions = crate::analysis::parser::parse_practice_response(&raw_response)
                .with_context(|| {
                    format!(
                        "第 {} 批练习题解析失败 (batch_index={}, requested={}, already_generated={}, target_points={})",
                        batch_index + 1,
                        batch_index,
                        batch_count,
                        already,
                        target_points.join("、")
                    )
                })?;

            for q in &mut batch_questions {
                q.question_form = Some(plan_item.question_form.clone());
                q.material_type = Some(plan_item.material_type.clone());
                q.requires_image = plan_item.requires_image;
                q.image_role = Some(plan_item.image_role.clone());
                q.image_type = Some(plan_item.image_type.clone());
                q.dependency_mode = Some(plan_item.dependency_mode.clone());
                q.image_spec = plan_item.image_spec.clone();
                if q.requires_image {
                    match image_generator.generate_for_question(q).await {
                        Ok(Some(path)) => q.image_path = Some(path),
                        Ok(None) => {}
                        Err(err) => {
                            let fallback_allowed = q
                                .image_spec
                                .as_ref()
                                .map(|s| s.fallback_allowed)
                                .unwrap_or(false)
                                || !matches!(q.image_role, Some(PracticeImageRole::AnswerBearing | PracticeImageRole::SourceMaterial));
                            if fallback_allowed {
                                tracing::warn!(
                                    batch = batch_index,
                                    error = %err,
                                    question_form = ?q.question_form,
                                    "练习题配图生成失败，按可降级策略继续"
                                );
                            } else {
                                return Err(err).with_context(|| {
                                    format!("第 {} 批练习题配图生成失败", batch_index + 1)
                                });
                            }
                        }
                    }
                }
            }

            tracing::info!(
                batch = batch_index,
                expected = batch_count,
                received = batch_questions.len(),
                "第 {} 批解析完成",
                batch_index + 1,
            );

            all_questions.extend(batch_questions);
            batch_index += 1;
        }

        // 5. 截取精确数量（若某批多返回了题目）
        all_questions.truncate(count as usize);

        if all_questions.len() != count as usize {
            anyhow::bail!(
                "练习题数量不符合预期：期望 {} 道，实际 {} 道（共 {} 批）",
                count,
                all_questions.len(),
                batch_index
            );
        }

        // 6. 存储练习集
        let now = chrono::Utc::now();
        let questions_json = serde_json::to_string(&all_questions)?;
        let practice = PracticeSet {
            id: Uuid::new_v4().to_string(),
            summary_id: summary_id.to_string(),
            subject: summary.subject.clone(),
            requirements: requirements.clone(),
            questions: questions_json,
            pdf_path: pdf_path.map(|p| p.to_string()),
            created_at: now.timestamp(),
            data_version: DATA_VERSION_CURRENT,
            generator_version: Some("practice/v2-batch".to_string()),
        };

        self.repository.insert_practice_set(&practice).await?;
        tracing::info!(id = %practice.id, count = all_questions.len(), batches = batch_index, "练习题生成完成");

        Ok(practice)
    }
}

fn select_reference_questions(reference_questions: &[String], target_points: &[String]) -> String {
    if reference_questions.is_empty() {
        return String::new();
    }

    let mut matched = reference_questions
        .iter()
        .filter(|item| target_points.iter().any(|point| item.contains(point)))
        .take(2)
        .cloned()
        .collect::<Vec<_>>();

    if matched.is_empty() {
        matched.extend(reference_questions.iter().take(2).cloned());
    }

    matched.join("\n\n")
}

fn normalize_requirements(requirements: Option<&str>) -> Option<String> {
    let trimmed = requirements?.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(trimmed.chars().take(500).collect())
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for ch in s.chars() {
        if count >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
        count += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_size_remaining_large() {
        assert_eq!(next_batch_size(10), 1);
    }

    #[test]
    fn batch_size_remaining_exactly_max() {
        assert_eq!(next_batch_size(1), 1);
    }

    #[test]
    fn batch_size_remaining_two_still_one() {
        assert_eq!(next_batch_size(2), 1);
    }

    #[test]
    fn batch_size_remaining_one() {
        assert_eq!(next_batch_size(1), 1);
    }

    #[test]
    fn batch_size_remaining_zero() {
        assert_eq!(next_batch_size(0), 0);
    }

    #[test]
    fn select_reference_prefers_matching_points() {
        let refs = vec![
            "知识点: 应用题\n原题: ...".to_string(),
            "知识点: 阅读理解\n原题: ...".to_string(),
        ];
        let selected = select_reference_questions(&refs, &["阅读理解".to_string()]);
        assert!(selected.contains("阅读理解"));
        assert!(!selected.is_empty());
    }
}
