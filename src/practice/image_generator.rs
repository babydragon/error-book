use anyhow::Result;

use crate::analysis::parser::PracticeQuestion;
use crate::config::AppConfig;
use crate::db::repository::Repository;
use crate::practice::planner::PracticeImageType;
use crate::storage::image::ImageStorage;
use crate::summary::image_generator::SummaryImageGenerator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PracticeImageClass {
    Precise,
    Scene,
}

#[derive(Clone)]
pub struct PracticeImageGenerator {
    inner: SummaryImageGenerator,
}

impl PracticeImageGenerator {
    pub fn new(config: AppConfig, repository: Repository, storage: ImageStorage) -> Self {
        Self {
            inner: SummaryImageGenerator::new(config, repository, storage),
        }
    }

    pub async fn generate_for_question(&self, question: &PracticeQuestion) -> Result<Option<String>> {
        if !question.requires_image {
            return Ok(None);
        }

        let image_spec = match &question.image_spec {
            Some(spec) => spec,
            None => {
                let prompt = build_fallback_prompt(question);
                let generated = self.inner.generate_raw_image(&prompt).await?;
                return Ok(Some(generated.image_path));
            }
        };

        let prompt = build_image_prompt(question, image_spec);
        tracing::info!(
            question_form = ?question.question_form,
            material_type = ?question.material_type,
            image_type = ?question.image_type,
            image_role = ?question.image_role,
            dependency_mode = ?question.dependency_mode,
            final_prompt = %prompt,
            "practice 题图最终生图 prompt"
        );

        let generated = self.inner.generate_raw_image(&prompt).await?;
        Ok(Some(generated.image_path))
    }
}

fn classify_image(question: &PracticeQuestion) -> PracticeImageClass {
    if matches!(
        question.image_type,
        Some(
            PracticeImageType::GeometryDiagram
                | PracticeImageType::Table
                | PracticeImageType::BarChart
                | PracticeImageType::LineChart
                | PracticeImageType::PieChart
                | PracticeImageType::SequenceDiagram
                | PracticeImageType::MapOrLayout
        )
    ) {
        PracticeImageClass::Precise
    } else {
        PracticeImageClass::Scene
    }
}

fn build_fallback_prompt(question: &PracticeQuestion) -> String {
    format!(
        "请为一道小学练习题生成配图。题型：{}；材料形态：{}；题目内容：{}。要求这是题目配图，不是照片，不是纸张扫描图；避免多余文字，避免练习本横线、墨渍、阴影、手写批注、水印、装饰背景，适合打印。",
        question.question_form.as_deref().unwrap_or("未指定"),
        question.material_type.as_deref().unwrap_or("未指定"),
        question.question
    )
}

fn build_image_prompt(question: &PracticeQuestion, image_spec: &crate::practice::planner::PracticeImageSpec) -> String {
    let class = classify_image(question);
    let image_description = extract_image_description(&question.question);
    let planner_text = image_spec.prompt.trim();
    let elements = if image_spec.elements.is_empty() {
        String::new()
    } else {
        format!("\n必须包含元素：{}。", image_spec.elements.join("、"))
    };

    match class {
        PracticeImageClass::Precise => {
            let main_description = image_description
                .clone()
                .unwrap_or_else(|| {
                    if planner_text.is_empty() {
                        "未提供单独图片说明，请根据题目中的图片相关描述生成。".to_string()
                    } else {
                        planner_text.to_string()
                    }
                });
            let planner_reference = if image_description.is_some() || planner_text.is_empty() {
                String::new()
            } else {
                format!("\n附加参考：{}", planner_text)
            };
            format!(
                "请生成一张干净、简洁、用于小学数学/语文/英语题目的教学配图。\n这是题目配图，不是照片，不是手绘草稿，不是扫描页，不是练习本截图。\n必须严格围绕给定的图片说明生成，避免自由发挥。\n以“图片说明”为唯一主要绘制依据；题目正文仅作为辅助参考，planner 给出的泛化描述不要优先采用。\n不要添加图片说明中未提到的数字、符号、珠子状态、表格数据、刻度、箭头、标签或其它会误导学生的信息。\n不要出现墨渍、污点、折痕、纸张纹理、练习本横线、手写字、阴影、水印、边框装饰、卡通贴纸、背景杂物。\n整体风格：白底、清晰、教材插图风、可打印。\n图片说明：{}\n题目正文（仅供辅助理解，不是主要绘制依据）：{}{}{}",
                main_description,
                strip_image_description(&question.question).trim(),
                elements,
                planner_reference,
            )
        }
        PracticeImageClass::Scene => format!(
            "请生成一张干净、简洁、用于小学题目的场景配图。\n这是题目配图，不是照片，不是扫描页，不要出现练习本横线、墨渍、手写批注、水印、背景杂物。\n图片应与题目一致，但不要添加多余文字。\n题目内容：{}\n图像要求：{}{}",
            question.question,
            if planner_text.is_empty() { "未指定" } else { planner_text },
            elements
        ),
    }
}

fn extract_image_description(question_text: &str) -> Option<String> {
    let start_marker = "[图片说明：";
    let start = question_text.find(start_marker)? + start_marker.len();
    let rest = &question_text[start..];
    let end = rest.find(']')?;
    let desc = rest[..end].trim();
    if desc.is_empty() {
        None
    } else {
        Some(desc.to_string())
    }
}

fn strip_image_description(question_text: &str) -> String {
    let start_marker = "[图片说明：";
    let Some(start) = question_text.find(start_marker) else {
        return question_text.to_string();
    };
    let prefix = &question_text[..start];
    let rest = &question_text[start + start_marker.len()..];
    let Some(end) = rest.find(']') else {
        return question_text.to_string();
    };
    let suffix = &rest[end + 1..];
    format!("{}{}", prefix, suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_prompt_uses_question_metadata() {
        let q = PracticeQuestion {
            question: "观察下图回答问题".to_string(),
            answer: "略".to_string(),
            knowledge_points: vec!["看图表达".to_string()],
            question_form: Some("看图题".to_string()),
            material_type: Some("scene".to_string()),
            requires_image: true,
            image_role: None,
            image_type: None,
            dependency_mode: None,
            image_spec: None,
            image_path: None,
        };
        let prompt = build_fallback_prompt(&q);
        assert!(prompt.contains("看图题"));
        assert!(prompt.contains("观察下图回答问题"));
    }

    #[test]
    fn classifies_precise_diagram_types() {
        let q = PracticeQuestion {
            question: "根据计数器回答".to_string(),
            answer: "略".to_string(),
            knowledge_points: vec!["万以内数".to_string()],
            question_form: Some("选择题".to_string()),
            material_type: Some("图文结合".to_string()),
            requires_image: true,
            image_role: None,
            image_type: Some(PracticeImageType::GeometryDiagram),
            dependency_mode: None,
            image_spec: None,
            image_path: None,
        };
        assert_eq!(classify_image(&q), PracticeImageClass::Precise);
    }

    #[test]
    fn precise_prompt_contains_clean_teaching_constraints() {
        let q = PracticeQuestion {
            question: "[图片说明：一个计数器，从左到右依次标有千位、百位、十位、个位。千位上有2颗珠子，百位上没有珠子，十位上有3颗珠子，个位上没有珠子。]\n右图计数器上表示的数是（   ）。".to_string(),
            answer: "略".to_string(),
            knowledge_points: vec!["万以内数".to_string()],
            question_form: Some("选择题".to_string()),
            material_type: Some("图文结合".to_string()),
            requires_image: true,
            image_role: None,
            image_type: Some(PracticeImageType::GeometryDiagram),
            dependency_mode: None,
            image_spec: Some(crate::practice::planner::PracticeImageSpec {
                prompt: "一个计数器图示，展示四位数数位状态".to_string(),
                elements: vec!["计数器".to_string(), "千位".to_string()],
                fallback_allowed: true,
            }),
            image_path: None,
        };
        let prompt = build_image_prompt(&q, q.image_spec.as_ref().unwrap());
        assert!(prompt.contains("以“图片说明”为唯一主要绘制依据"));
        assert!(prompt.contains("一个计数器，从左到右依次标有千位、百位、十位、个位"));
        assert!(prompt.contains("练习本横线"));
        assert!(prompt.contains("必须包含元素"));
        assert!(!prompt.contains("展示四位数数位状态"), "prompt should not include planner freeform description when image description exists: {}", prompt);
    }

    #[test]
    fn extracts_image_description_from_question() {
        let q = "题干前文\n[图片说明：一个算盘，千位有1颗上珠。]\n后文";
        assert_eq!(extract_image_description(q).as_deref(), Some("一个算盘，千位有1颗上珠。"));
    }

    #[test]
    fn strip_image_description_removes_embedded_block() {
        let q = "题干前文\n[图片说明：一个算盘，千位有1颗上珠。]\n后文";
        let stripped = strip_image_description(q);
        assert!(!stripped.contains("图片说明"));
        assert!(stripped.contains("题干前文"));
        assert!(stripped.contains("后文"));
    }
}
