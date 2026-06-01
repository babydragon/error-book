use anyhow::Result;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::analysis::parser::PracticeQuestion;
use crate::config::{AppConfig, RoleKind};
use crate::db::models::Summary;
use crate::llm::client::ChatClient;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PracticeImageRole {
    None,
    Decorative,
    Supportive,
    AnswerBearing,
    SourceMaterial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PracticeImageType {
    None,
    GeometryDiagram,
    Table,
    BarChart,
    LineChart,
    PieChart,
    SequenceDiagram,
    SceneIllustration,
    PictureStory,
    MapOrLayout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PracticeDependencyMode {
    TextOnly,
    RenderFromSpec,
    JointPlanned,
    ImageFirstFinalizeText,
}

#[derive(Debug, Clone, Default)]
pub struct PracticeImageSpec {
    pub prompt: String,
    pub elements: Vec<String>,
    pub fallback_allowed: bool,
}

impl<'de> Deserialize<'de> for PracticeImageRole {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let normalized = normalize_enum_token(&raw);
        match normalized.as_str() {
            "none" | "no_image" => Ok(Self::None),
            "decorative" => Ok(Self::Decorative),
            "supportive" | "support" => Ok(Self::Supportive),
            "answer_bearing" | "answerbearing" | "essential" => Ok(Self::AnswerBearing),
            "source_material" | "sourcematerial" => Ok(Self::SourceMaterial),
            other => Err(serde::de::Error::custom(format!(
                "未知 image_role: {} (normalized={})",
                raw, other
            ))),
        }
    }
}

impl Serialize for PracticeImageRole {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::None => "none",
            Self::Decorative => "decorative",
            Self::Supportive => "supportive",
            Self::AnswerBearing => "answer_bearing",
            Self::SourceMaterial => "source_material",
        })
    }
}

impl<'de> Deserialize<'de> for PracticeImageType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let normalized = normalize_enum_token(&raw);
        match normalized.as_str() {
            "none" | "no_image" => Ok(Self::None),
            "diagram" | "geometry_diagram" | "geometry" => Ok(Self::GeometryDiagram),
            "math_diagram" | "mathdiagram" => Ok(Self::GeometryDiagram),
            "table" => Ok(Self::Table),
            "bar_chart" | "barchart" => Ok(Self::BarChart),
            "line_chart" | "linechart" => Ok(Self::LineChart),
            "pie_chart" | "piechart" => Ok(Self::PieChart),
            "sequence_diagram" | "sequence" => Ok(Self::SequenceDiagram),
            "scene_illustration" | "scene" | "illustration" => Ok(Self::SceneIllustration),
            "picture_story" | "picturestory" => Ok(Self::PictureStory),
            "map_or_layout" | "map" | "layout" => Ok(Self::MapOrLayout),
            other => Err(serde::de::Error::custom(format!(
                "未知 image_type: {} (normalized={})",
                raw, other
            ))),
        }
    }
}

impl Serialize for PracticeImageType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::None => "none",
            Self::GeometryDiagram => "geometry_diagram",
            Self::Table => "table",
            Self::BarChart => "bar_chart",
            Self::LineChart => "line_chart",
            Self::PieChart => "pie_chart",
            Self::SequenceDiagram => "sequence_diagram",
            Self::SceneIllustration => "scene_illustration",
            Self::PictureStory => "picture_story",
            Self::MapOrLayout => "map_or_layout",
        })
    }
}

impl<'de> Deserialize<'de> for PracticeDependencyMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let normalized = normalize_enum_token(&raw);
        match normalized.as_str() {
            "text_only" | "textonly" => Ok(Self::TextOnly),
            "standalone" => Ok(Self::TextOnly),
            "render_from_spec" | "renderfromspec" => Ok(Self::RenderFromSpec),
            "joint_planned" | "jointplanned" => Ok(Self::JointPlanned),
            "image_first_finalize_text" | "imagefirstfinalizetext" => Ok(Self::ImageFirstFinalizeText),
            "essential" => Ok(Self::RenderFromSpec),
            other => Err(serde::de::Error::custom(format!(
                "未知 dependency_mode: {} (normalized={})",
                raw, other
            ))),
        }
    }
}

impl Serialize for PracticeDependencyMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::TextOnly => "text_only",
            Self::RenderFromSpec => "render_from_spec",
            Self::JointPlanned => "joint_planned",
            Self::ImageFirstFinalizeText => "image_first_finalize_text",
        })
    }
}

impl<'de> Deserialize<'de> for PracticeImageSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::Null => Ok(Self::default()),
            Value::String(prompt) => Ok(Self {
                prompt,
                elements: Vec::new(),
                fallback_allowed: true,
            }),
            Value::Object(map) => {
                let prompt = map
                    .get("prompt")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let elements = map
                    .get("elements")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(ToString::to_string))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let fallback_allowed = map
                    .get("fallback_allowed")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                Ok(Self {
                    prompt,
                    elements,
                    fallback_allowed,
                })
            }
            other => Err(serde::de::Error::custom(format!(
                "image_spec 类型不支持: {}",
                other
            ))),
        }
    }
}

impl Serialize for PracticeImageSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("PracticeImageSpec", 3)?;
        state.serialize_field("prompt", &self.prompt)?;
        state.serialize_field("elements", &self.elements)?;
        state.serialize_field("fallback_allowed", &self.fallback_allowed)?;
        state.end()
    }
}

fn normalize_enum_token(raw: &str) -> String {
    raw.trim()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PracticeQuestionPlanItem {
    pub index: u32,
    pub primary_point: String,
    #[serde(default)]
    pub secondary_points: Vec<String>,
    pub question_form: String,
    pub material_type: String,
    #[serde(default)]
    pub target_reference_ids: Vec<String>,
    #[serde(default)]
    pub avoid_topics: Vec<String>,
    pub requires_image: bool,
    pub image_role: PracticeImageRole,
    pub image_type: PracticeImageType,
    pub dependency_mode: PracticeDependencyMode,
    pub image_spec: Option<PracticeImageSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PracticePlan {
    pub subject: String,
    pub grade_level: String,
    pub count: u32,
    #[serde(default)]
    pub rationale: String,
    pub items: Vec<PracticeQuestionPlanItem>,
}

pub struct PracticePlanner {
    config: AppConfig,
    chat_client: ChatClient,
}

impl PracticePlanner {
    pub fn new(config: AppConfig, chat_client: ChatClient) -> Self {
        Self { config, chat_client }
    }

    pub async fn plan(
        &self,
        summary: &Summary,
        weak_points: &[String],
        reference_questions: &[String],
        count: u32,
        requirements: Option<&str>,
        existing_questions: &[PracticeQuestion],
    ) -> Result<PracticePlan> {
        let grade_level = self.config.defaults.grade_level.clone();
        let messages = crate::llm::prompts::build_practice_plan_prompt(
            &summary.subject,
            &grade_level,
            weak_points,
            reference_questions,
            count,
            requirements,
            existing_questions,
        );

        let raw = self
            .chat_client
            .chat_with_role(
                &self.config.llm,
                RoleKind::PracticePlanning,
                messages,
                Some(0.2),
            )
            .await?;

        crate::analysis::parser::parse_practice_plan_response(&raw)
    }
}

pub fn heuristic_plan(
    subject: &str,
    grade_level: &str,
    weak_points: &[String],
    count: u32,
) -> PracticePlan {
    let canonical = dedupe_points(weak_points);
    let mut items = Vec::new();

    for idx in 0..count as usize {
        let primary = canonical
            .get(idx % canonical.len())
            .cloned()
            .unwrap_or_else(|| "综合练习".to_string());
        let secondary = choose_secondary_point(&canonical, &primary, idx)
            .into_iter()
            .collect::<Vec<_>>();

        let (form, material_type, requires_image, image_role, image_type, dependency_mode) =
            infer_question_shape(subject, &primary, &secondary);

        items.push(PracticeQuestionPlanItem {
            index: idx as u32 + 1,
            primary_point: primary,
            secondary_points: secondary,
            question_form: form,
            material_type,
            target_reference_ids: Vec::new(),
            avoid_topics: Vec::new(),
            requires_image,
            image_role,
            image_type,
            dependency_mode,
            image_spec: None,
        });
    }

    PracticePlan {
        subject: subject.to_string(),
        grade_level: grade_level.to_string(),
        count,
        rationale: "heuristic_fallback".to_string(),
        items,
    }
}

fn infer_question_shape(
    subject: &str,
    primary: &str,
    secondary: &[String],
) -> (
    String,
    String,
    bool,
    PracticeImageRole,
    PracticeImageType,
    PracticeDependencyMode,
) {
    let all_points = std::iter::once(primary).chain(secondary.iter().map(|s| s.as_str()));
    let joined = all_points.collect::<Vec<_>>().join("/");

    if subject.contains("数学") && (joined.contains("几何") || joined.contains("图形")) {
        return (
            "图形题".to_string(),
            "diagram".to_string(),
            true,
            PracticeImageRole::AnswerBearing,
            PracticeImageType::GeometryDiagram,
            PracticeDependencyMode::RenderFromSpec,
        );
    }

    if subject.contains("数学") && (joined.contains("统计") || joined.contains("表格")) {
        return (
            "统计图表题".to_string(),
            "chart".to_string(),
            true,
            PracticeImageRole::AnswerBearing,
            PracticeImageType::BarChart,
            PracticeDependencyMode::RenderFromSpec,
        );
    }

    if subject.contains("语文") && joined.contains("看图") {
        return (
            "看图表达".to_string(),
            "scene".to_string(),
            true,
            PracticeImageRole::SourceMaterial,
            PracticeImageType::SceneIllustration,
            PracticeDependencyMode::ImageFirstFinalizeText,
        );
    }

    if subject.contains("英语") && (joined.contains("看图") || joined.contains("场景")) {
        return (
            "看图表达".to_string(),
            "scene".to_string(),
            true,
            PracticeImageRole::SourceMaterial,
            PracticeImageType::SceneIllustration,
            PracticeDependencyMode::ImageFirstFinalizeText,
        );
    }

    (
        "综合练习题".to_string(),
        if subject.contains("语文") || subject.contains("英语") {
            "passage".to_string()
        } else {
            "text".to_string()
        },
        false,
        PracticeImageRole::None,
        PracticeImageType::None,
        PracticeDependencyMode::TextOnly,
    )
}

fn dedupe_points(weak_points: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for point in weak_points {
        let trimmed = point.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_lowercase()) {
            out.push(trimmed.to_string());
        }
    }
    if out.is_empty() {
        out.push("综合练习".to_string());
    }
    out
}

fn choose_secondary_point(points: &[String], primary: &str, batch_index: usize) -> Option<String> {
    let groups = compatibility_groups();
    let primary_group = groups.get(primary)?;
    points
        .iter()
        .enumerate()
        .filter(|(_, p)| p.as_str() != primary)
        .find(|(idx, p)| groups.get(p.as_str()) == Some(primary_group) && (batch_index + idx) % 2 == 0)
        .map(|(_, p)| p.clone())
}

fn compatibility_groups() -> std::collections::HashMap<&'static str, &'static str> {
    std::collections::HashMap::from([
        ("应用题", "math_word_problem"),
        ("数量关系", "math_word_problem"),
        ("审题", "math_word_problem"),
        ("列式", "math_word_problem"),
        ("阅读理解", "reading"),
        ("提取信息", "reading"),
        ("概括中心", "reading"),
        ("看图写话", "scene"),
        ("完形填空", "english_cloze"),
        ("上下文推断", "english_cloze"),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_plan_rotates_points() {
        let weak_points = vec!["应用题".to_string(), "数量关系".to_string()];
        let plan = heuristic_plan("数学", "二年级", &weak_points, 3);
        assert_eq!(plan.items.len(), 3);
        assert_eq!(plan.items[0].primary_point, "应用题");
        assert_eq!(plan.items[1].primary_point, "数量关系");
        assert_eq!(plan.items[2].primary_point, "应用题");
    }

    #[test]
    fn heuristic_plan_marks_geometry_as_image_question() {
        let weak_points = vec!["几何图形".to_string()];
        let plan = heuristic_plan("数学", "二年级", &weak_points, 1);
        assert!(plan.items[0].requires_image);
        assert_eq!(plan.items[0].image_type, PracticeImageType::GeometryDiagram);
    }
}
