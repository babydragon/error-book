use crate::config::AppConfig;
use crate::db::models::AnalysisRequest;

use crate::analysis::parser::PracticeQuestion;
use crate::practice::planner::PracticeQuestionPlanItem;

// ═══════════════════════════════════════════════════════════════
// Legacy combined prompt (existing, kept for fallback)
// ═══════════════════════════════════════════════════════════════

/// 构建错题分析 Prompt
pub fn build_analysis_prompt(
    config: &AppConfig,
    request: &AnalysisRequest,
) -> Vec<super::client::ChatMessage> {
    let mut messages = Vec::new();

    // System prompt
    let system = format!(
        r#"你是一个小学老师，请按照给你的题目，分析学生做错的题目。

题目颜色定义：给你的题目，都是经过老师批改的。其中：
- {}：{}的内容
- {}：孩子订正的内容
- 其他颜色：孩子做题使用的笔，可能包含铅笔、黑色钢笔等。

注意：以上为默认颜色定义，如果让你分析的时候有特别说明，必须以当此的要求为准，特别是学生笔迹的颜色，可能会使用不同的笔作答。

错误分析要求：

1. 识别原题：要求获取完整原题内容，原题内容以markdown格式给出，去掉任何学生做题和老师批改内容，确保原题可以重新用来打印。
2. 将题目进行归类，按照题目考察知识点和给定的年级/科目范围，给出题目考查点标签，可以是多个。
3. 分析错误原因：按照题目科目，分析错误的原因，以及改进建议。改进建议需要能够执行的。

输出要求：

原题部分采用markdown格式输出，必须包含完整题目和原始题目结构，包括但不限于题目公共题干（例如阅读理解的文章、大题开始的要求）、配图等重新生成题目需要的内容。
对于配图，如果无法用markdown表示，请给出配图在原图中的坐标，要求格式是 [[x1, y1, x2, y2], ...]，每个子数组是一个配图的矩形区域坐标，分别是图片的左上角和右下角坐标。

其他输出采用JSON格式结构化输出，包含以下key：
- subject：科目，格式为字符串，例如语文、数学等。如果输入的时候有说明，以输入为准，否则按照题目来进行判断，所有科目都是小学阶段可能安排的。
- classification：题目分类，格式为字符串数组，包括考查知识点，例如语文可能有"字词书写"、"拼音运用"、"仿写句子"等，数学可能有"带余数除法"、"钟表"、"算盘"等。
- reason：错误原因，格式为字符串
- suggestions：改进建议，格式为字符串

年级：{}{}"#,
        request.color_teacher.as_deref().unwrap_or("红色"),
        "老师批改",
        request.color_correction.as_deref().unwrap_or("蓝色"),
        request
            .grade_level
            .as_deref()
            .unwrap_or(&config.defaults.grade_level),
        request
            .subject
            .as_deref()
            .map(|s| format!("\n科目：{}", s))
            .unwrap_or_default(),
    );

    messages.push(super::client::ChatMessage::system(&system));

    // User message 会由调用方添加图片
    messages
}

/// 获取用户消息的文本部分（不含图片）
pub fn analysis_user_text() -> &'static str {
    "请分析这张错题图片。"
}

/// 构建阶段性总结 Prompt
pub fn build_summary_prompt(
    subject: &str,
    grade_level: &str,
    records_text: &str,
) -> Vec<super::client::ChatMessage> {
    let system = format!(
        r#"你是一个经验丰富的小学{}教师，请根据以下一段时间内的学生错题记录，进行阶段性总结。

每条错题包含结构化字段：知识点、题型、难度、原题、学生作答、错误类型/子类、根因编码、错误原因、改进建议。
请优先基于每题的题型、难度、错误类型、根因编码和学生作答进行归纳，而非仅凭自由文本推测。

要求：
1. 归纳共性错误原因（按频次排序，优先按错误类型和根因编码分类）
2. 总结共性改进方向
3. 提炼薄弱知识点列表
4. 给出下一阶段的学习建议

输出严格采用JSON格式，包含以下key：
- common_reasons: 字符串，共性错误原因总结
- common_suggestions: 字符串，共性改进建议
- weak_points: 字符串数组，薄弱知识点列表
- detail: 字符串，详细分析内容"#,
        subject
    );

    let user = format!(
        "科目：{}\n年级：{}\n\n错题记录：\n{}",
        subject, grade_level, records_text
    );

    vec![
        super::client::ChatMessage::system(&system),
        super::client::ChatMessage::user_text(&user),
    ]
}

/// 构建巩固练习生成 Prompt
pub fn build_practice_prompt(
    subject: &str,
    grade_level: &str,
    weak_points: &[String],
    reference_questions: &str,
    count: u32,
    requirements: Option<&str>,
) -> Vec<super::client::ChatMessage> {
    let system = format!(
        r#"你是一个经验丰富的小学{}教师。请根据学生的薄弱知识点和参考题目风格，生成新的巩固练习题。

内容要求：
1. 题目必须覆盖给定的薄弱知识点
2. 题目风格参考给出的原题，但不要重复原题
3. 题目适合{}学生
4. 每道题都必须包含题目内容、参考答案、知识点

输出格式要求（非常重要）：
1. 只输出 JSON，不要输出任何解释、说明、前后缀、标题、备注
2. 不要使用 markdown 代码块，不要输出 ```json
3. 输出必须是一个 JSON 对象，包含一个 "questions" 数组
4. 第一个字符必须是 {{，最后一个字符必须是 }}
5. "questions" 数组必须且只能包含 {} 个对象，不能多也不能少
6. 每个对象必须且只能包含以下 3 个字段：
   - question: 字符串，题目内容，允许使用 markdown，但必须作为 JSON 字符串输出
   - answer: 字符串，参考答案
   - knowledge_points: 字符串数组，表示该题考查的知识点
7. 所有字段名必须完全一致，不能增加其他字段，不能省略字段
8. 如果 question 中需要换行，请使用 \n；如果内容中出现英文双引号，必须正确转义为 \"
9. knowledge_points 必须是 JSON 字符串数组，即使只有 1 个知识点也必须输出数组
10. 不要输出任何 emoji、表情符号、贴纸风格符号或装饰性 pictograph，不要使用 ✅📚⭐🎯😀 等字符

输出示例：
{{
  "questions": [
    {{
      "question": "1. 计算：12 ÷ 3 = ?",
      "answer": "4",
      "knowledge_points": ["表内除法"]
    }}
  ]
}}"#,
        subject, grade_level, count
    );

    let extra_requirements = requirements
        .map(|r| {
            format!(
                "\n\n额外要求：\n{}\n\n注意：额外要求不能改变题目数量、JSON 输出格式和必需字段。",
                r
            )
        })
        .unwrap_or_default();

    let user = format!(
        "薄弱知识点：{}\n\n参考题目风格：\n{}{}\n\n请生成 {} 道巩固练习题。请再次确认：最终回复必须是包含 \"questions\" 数组的合法 JSON 对象，且数组长度必须恰好为 {}。",
        weak_points.join("、"),
        reference_questions,
        extra_requirements,
        count,
        count
    );

    vec![
        super::client::ChatMessage::system(&system),
        super::client::ChatMessage::user_text(&user),
    ]
}

pub fn build_practice_plan_prompt(
    subject: &str,
    grade_level: &str,
    weak_points: &[String],
    reference_questions: &[String],
    count: u32,
    requirements: Option<&str>,
    existing_questions: &[PracticeQuestion],
) -> Vec<super::client::ChatMessage> {
    let system = format!(
        r#"你是一个小学{}练习题规划专家。你的任务不是直接出题，而是先为巩固练习生成一个出题规划。

要求：
1. 结合薄弱知识点、参考题风格、额外要求，规划共 {} 道题
2. 每题默认 1 个主知识点；只有自然兼容时才允许最多 1 个辅知识点
3. 不要把多个不相关知识点硬塞进同一题
4. 需要判断题型、材料形态，以及是否需要图片
5. 数学几何图/统计图/表格题优先标记为 requires_image=true
6. 看图写话、看图表达这类 source-material 场景要明确依赖图片
7. 所有枚举字段必须使用固定英文候选值，禁止输出中文解释句
8. 对计数器、算盘、表格、统计图、几何图、数轴、路线图这类“答题依赖精确数据”的图片，允许规划 requires_image=true，但不要假设自由生图一定可用；image_spec 更像结构说明，不是最终自由生图 prompt
9. 你可以先把图片理解成两大类：
   - 精确图：题目依赖图中的精确数据/位置/符号（如计数器、算盘、表格、统计图、几何图、路线图）
   - 场景图：主要提供情境或辅助理解，不承载唯一答案数据

输出必须是合法 JSON 对象，包含：
- subject
- grade_level
- count
- rationale
- items: 长度恰好为 {} 的数组

每个 item 包含：
- index
- primary_point
- secondary_points
- question_form
- material_type
- target_reference_ids
- avoid_topics
- requires_image
- image_role
- image_type
- dependency_mode
- image_spec（可为空）

严格枚举约束（只能从以下候选值中选择，不能改写、不能翻译、不能补充解释）：
- image_role: ["none", "decorative", "supportive", "answer_bearing", "source_material"]
- image_type: ["none", "geometry_diagram", "table", "bar_chart", "line_chart", "pie_chart", "sequence_diagram", "scene_illustration", "picture_story", "map_or_layout"]
- dependency_mode: ["text_only", "render_from_spec", "joint_planned", "image_first_finalize_text"]

额外硬性要求：
1. image_role / image_type / dependency_mode 这三个字段必须是单个英文字符串，不能是句子，不能带中文，不能带冒号，不能写成“必要：xxx”这类格式
2. image_spec 只能是 null、对象或简短字符串 prompt；如果是对象，只能包含 prompt、elements、fallback_allowed
3. question_form 和 material_type 可以用中文，但 image_role / image_type / dependency_mode 不可以用中文
4. 若 requires_image=false，则建议 image_role="none"、image_type="none"、dependency_mode="text_only"
5. 对计数器/算盘/表格/统计图/几何图等精确图，image_spec 应尽量描述结构元素，不要把它写成依赖模型自由发挥的长文案
6. 对精确图，image_spec 应写“必须出现哪些结构元素”和“不要出现哪些额外数据/符号”；对场景图，image_spec 可以更偏场景描述

输出示例：
{{
  "subject": "数学",
  "grade_level": "二年级",
  "count": 2,
  "rationale": "按薄弱知识点分配题型",
  "items": [
    {{
      "index": 1,
      "primary_point": "万以内数的认识与读写",
      "secondary_points": [],
      "question_form": "选择题",
      "material_type": "图文结合",
      "target_reference_ids": ["R4"],
      "avoid_topics": [],
      "requires_image": true,
      "image_role": "source_material",
      "image_type": "geometry_diagram",
      "dependency_mode": "render_from_spec",
      "image_spec": {{
        "prompt": "一个计数器图示，展示四位数数位状态",
        "elements": ["计数器", "千位", "百位", "十位", "个位"],
        "fallback_allowed": true
      }}
    }},
    {{
      "index": 2,
      "primary_point": "时间相关概念",
      "secondary_points": [],
      "question_form": "解答题",
      "material_type": "长题干文字",
      "target_reference_ids": ["R2"],
      "avoid_topics": [],
      "requires_image": false,
      "image_role": "none",
      "image_type": "none",
      "dependency_mode": "text_only",
      "image_spec": null
    }}
  ]
}}"#,
        subject, count, count
    );

    let references = reference_questions
        .iter()
        .take(5)
        .enumerate()
        .map(|(idx, item)| format!("R{}: {}", idx + 1, item))
        .collect::<Vec<_>>()
        .join("\n\n");
    let existing = existing_questions
        .iter()
        .take(6)
        .enumerate()
        .map(|(idx, q)| {
            format!(
                "{}. {} / {}",
                idx + 1,
                q.knowledge_points.join("、"),
                q.question.chars().take(60).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let extra = requirements
        .map(|r| format!("\n额外要求：{}", r))
        .unwrap_or_default();

    let user = format!(
        "科目：{}\n年级：{}\n目标题数：{}\n薄弱知识点：{}\n\n参考题：\n{}\n\n已生成题目（避免重复）：\n{}{}\n\n请先输出出题规划 JSON。",
        subject,
        grade_level,
        count,
        weak_points.join("、"),
        references,
        if existing.is_empty() { "无" } else { &existing },
        extra,
    );

    vec![
        super::client::ChatMessage::system(&system),
        super::client::ChatMessage::user_text(&user),
    ]
}

/// 构建单批练习题生成 Prompt（按规划分配知识点）
pub fn build_practice_batch_prompt(
    subject: &str,
    grade_level: &str,
    plan_item: &PracticeQuestionPlanItem,
    reference_questions: &str,
    existing_questions: &[PracticeQuestion],
    count: u32,
    requirements: Option<&str>,
) -> Vec<super::client::ChatMessage> {
    let system = format!(
        r#"你是一个经验丰富的小学{}教师。现在需要按指定知识点生成新的巩固练习题。

内容要求：
1. 本批题目只需要覆盖指定的目标知识点，不要为了“覆盖更多点”强行混入未指定知识点
2. 题目风格参考给出的原题，但不要重复原题
3. 题目适合{}学生
4. 每道题都必须包含题目内容、参考答案、知识点
5. 题目自然、真实、像正常教辅题，不要生硬拼接多个知识点
6. 如果目标知识点有 2 个，只有在同一题里自然融合时才可同时覆盖；否则以第 1 个为主
7. 本题规划题型：{}；材料形态：{}
8. 是否需要图片：{}；图片作用：{:?}；依赖模式：{:?}

输出格式要求（非常重要）：
1. 只输出 JSON，不要输出任何解释、说明、前后缀、标题、备注
2. 不要使用 markdown 代码块，不要输出 ```json
3. 输出必须是一个 JSON 对象，包含一个 \"questions\" 数组
4. 第一个字符必须是 {{，最后一个字符必须是 }}
5. \"questions\" 数组必须且只能包含 {} 个对象，不能多也不能少
6. 每个对象必须且只能包含以下 3 个字段：question、answer、knowledge_points
7. question 与 answer 中如需换行请使用 \n；双引号必须转义为 \"
8. knowledge_points 必须是字符串数组，且只包含本批目标知识点中实际使用到的点
9. 如果题目或答案中需要表格，必须使用标准 markdown 表格：第一行表头，第二行必须是 `| --- | --- |` 这种分隔行，后面再写数据行
10. 表格单元格内部禁止换行，禁止使用 `│` 这类竖线字符代替 `|`，禁止输出不完整表格
11. 不要输出任何 emoji、表情符号、贴纸风格符号或装饰性 pictograph"#,
        subject,
        grade_level,
        plan_item.question_form,
        plan_item.material_type,
        if plan_item.requires_image {
            "是"
        } else {
            "否"
        },
        plan_item.image_role,
        plan_item.dependency_mode,
        count
    );

    let existing_summary = existing_questions
        .iter()
        .take(6)
        .enumerate()
        .map(|(idx, q)| {
            format!(
                "{}. 知识点:{}；题目摘要:{}",
                idx + 1,
                q.knowledge_points.join("、"),
                q.question.chars().take(80).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let extra_requirements = requirements
        .map(|r| {
            format!(
                "\n\n额外要求：\n{}\n\n注意：额外要求不能改变题目数量、JSON 输出格式和必需字段。",
                r
            )
        })
        .unwrap_or_default();

    let duplicate_guard = if existing_summary.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n以下题目已经生成，禁止重复题型、场景和问法：\n{}",
            existing_summary
        )
    };

    let user = format!(
        "本批主知识点：{}\n本批辅知识点：{}\n避免重复/避免主题：{}\n\n参考题目风格：\n{}{}{}\n\n请生成 {} 道巩固练习题。再次确认：本批只围绕上述目标知识点出题，不要硬塞未指定知识点；最终回复必须是包含 \"questions\" 数组的合法 JSON 对象，且数组长度必须恰好为 {}。",
        plan_item.primary_point,
        if plan_item.secondary_points.is_empty() { "无".to_string() } else { plan_item.secondary_points.join("、") },
        if plan_item.avoid_topics.is_empty() { "无".to_string() } else { plan_item.avoid_topics.join("、") },
        reference_questions,
        duplicate_guard,
        extra_requirements,
        count,
        count
    );

    vec![
        super::client::ChatMessage::system(&system),
        super::client::ChatMessage::user_text(&user),
    ]
}

/// 当首次生成题量不足时，补生成剩余练习题
pub fn build_practice_fill_prompt(
    subject: &str,
    grade_level: &str,
    weak_points: &[String],
    existing_questions: &[PracticeQuestion],
    count: u32,
    requirements: Option<&str>,
) -> Vec<super::client::ChatMessage> {
    let system = format!(
        r#"你是一个经验丰富的小学{}教师。现在需要补生成剩余的巩固练习题。

内容要求：
1. 题目必须覆盖给定的薄弱知识点
2. 题目适合{}学生
3. 不要重复已有题目，不要改写已有题目
4. 优先保证题量足够；如果 token 紧张，请缩短题干和答案，不要少题

输出格式要求（非常重要）：
1. 只输出 JSON，不要输出任何解释、说明、前后缀、标题、备注
2. 不要使用 markdown 代码块，不要输出 ```json
3. 输出必须是一个 JSON 对象，包含一个 "questions" 数组
4. 第一个字符必须是 {{，最后一个字符必须是 }}
5. "questions" 数组必须且只能包含 {} 个对象，不能多也不能少
6. 每个对象必须且只能包含以下 3 个字段：question、answer、knowledge_points
7. question 与 answer 尽量简洁；如果需要换行请使用 \n；双引号必须转义为 \"
8. knowledge_points 必须是字符串数组
9. 不要输出任何 emoji、表情符号、贴纸风格符号或装饰性 pictograph，不要使用 ✅📚⭐🎯😀 等字符"#,
        subject, grade_level, count
    );

    let existing_json = serde_json::to_string(existing_questions).unwrap_or_default();
    let extra_requirements = requirements
        .map(|r| {
            format!(
                "\n\n额外要求：\n{}\n\n注意：额外要求不能改变题目数量、JSON 输出格式和必需字段。",
                r
            )
        })
        .unwrap_or_default();

    let user = format!(
        "薄弱知识点：{}\n\n以下题目已经生成，禁止重复：\n{}{}\n\n现在请只补生成剩余 {} 道新题。最终回复必须是包含 \"questions\" 数组的合法 JSON 对象，且数组长度必须恰好为 {}。",
        weak_points.join("、"),
        existing_json,
        extra_requirements,
        count,
        count
    );

    vec![
        super::client::ChatMessage::system(&system),
        super::client::ChatMessage::user_text(&user),
    ]
}

/// 构建阶段性总结信息图提示词
pub fn build_summary_infographic_prompt(
    subject: &str,
    grade_level: &str,
    summary: &crate::db::models::Summary,
    weak_points: &[String],
    extra_requirements: Option<&str>,
) -> String {
    let extra = extra_requirements
        .map(|v| format!("\n补充要求：{}", v))
        .unwrap_or_default();

    format!(
        r#"请生成一张适合{}学生记忆复习的{}教育信息图。

目标：帮助孩子巩固这阶段还没有完全掌握的知识点，方便记忆、复习和反复查看。

画面要求：
1. 整体风格温和、鼓励式、儿童友好，适合小学生
2. 中文排版清晰，标题醒目，信息分区明确
3. 使用图标、箭头、卡片、分区块帮助记忆
4. 内容聚焦“薄弱知识点 + 核心规则/口诀 + 易混淆点 + 记忆提醒”
5. 重点是帮助孩子记住知识点，不是分析错误过程，不要把画面做成教师批改报告
6. 可以适当加入简短示例、对比提示、步骤提醒，但必须简洁、直观、易记
7. 避免过多小字，避免复杂背景，确保可读性
8. 不要出现真人照片、品牌 logo、英文大段文字、血腥或成人元素
9. 输出为单张信息图，适合作为学习海报保存或直接打印复习

内容组织建议：
- 用 3~6 个小模块展示最需要巩固的知识点
- 每个模块优先展示：知识点名称、记忆口诀/规则、一个简短提醒
- 如果需要展示“易错点”，只保留一句简短提醒，例如“注意进位”“不要漏单位”“先审题再计算”
- 尽量减少大段“错误原因分析”文字

信息图内容依据：
- 科目：{}
- 共性错误原因（仅作弱参考，不要作为主体）：{}
- 共性改进建议：{}
- 薄弱知识点：{}
- 详细分析：{}
{}

请直接根据这些内容生成一张“知识点巩固型阶段学习信息图”。让孩子一眼能看懂、愿意看、看完能帮助记住关键知识点。"#,
        grade_level,
        subject,
        subject,
        summary.common_reasons,
        summary.common_suggestions,
        weak_points.join("、"),
        summary.detail,
        extra,
    )
}

// ═══════════════════════════════════════════════════════════════
// Multi-stage analysis prompts (vision → extraction → pedagogical)
// ═══════════════════════════════════════════════════════════════

/// Stage 1: Vision recognition prompt — OCR + layout + student/teacher mark separation.
///
/// Input: raw image. Output: JSON with recognized text, layout, student answer, teacher marks.
pub fn build_vision_recognition_prompt(
    request: &AnalysisRequest,
) -> Vec<super::client::ChatMessage> {
    let system = format!(
        r#"你是一个小学试卷/作业的视觉识别专家。你的唯一任务是：从给定的错题图片中，精确识别并分离出以下内容。

颜色定义：
- {teacher_color}：老师批改的内容（对号、叉号、分数、批注等）
- {correction_color}：学生订正的内容
- 其他颜色：学生做题使用的笔迹

注意：以上为默认颜色定义，如果让你分析的时候有特别说明，必须以当此的要求为准。

请严格以 JSON 格式输出，包含以下字段：
{{
  "recognized_text": "图片中所有识别到的原始文本（保持原样，包括学生手写内容）",
  "question_text_clean": "去掉所有批注、涂改、学生作答后的纯净题目文本",
  "layout_description": "版面描述：标题、题号、子题结构、配图位置等",
  "student_answer_text": "学生作答的文本内容（区分各小题）",
  "teacher_marks": [
    {{
      "mark_type": "对号|叉号|半对|分数|批注|圈画",
      "location_description": "在题目中的位置描述",
      "mark_content": "标记的具体内容"
    }}
  ],
  "image_regions": [[x1, y1, x2, y2]],
  "handwriting_confidence": 0.8
}}

其中：
- recognized_text: 保留图片中所有文字，包括印刷体和手写体
- question_text_clean: 只有原题内容，去除学生的答案和老师的批改标记
- layout_description: 描述题目的版面结构，例如"第3大题第2小题，包含一个选择题和一个配图"
- student_answer_text: 学生的答案文本
- teacher_marks: 老师批改标记的详细列表，每个标记包含类型、位置和内容
- image_regions: 配图在原图中的矩形坐标 [[x1, y1, x2, y2], ...]
- handwriting_confidence: 手写识别的整体置信度（0.0~1.0）

注意：只输出 JSON，不要输出其他内容。"#,
        teacher_color = request.color_teacher.as_deref().unwrap_or("红色"),
        correction_color = request.color_correction.as_deref().unwrap_or("蓝色"),
    );

    vec![super::client::ChatMessage::system(&system)]
}

/// Stage 1 user text (sent with image)
pub fn vision_recognition_user_text() -> &'static str {
    "请识别这张错题图片中的所有内容，按照系统提示的 JSON 格式输出。"
}

/// Stage 2: Structured extraction prompt — based on vision JSON, produce clean markdown + metadata.
///
/// Input: vision_recognition JSON output. Output: JSON with structured question + metadata.
pub fn build_structured_extraction_prompt(
    config: &AppConfig,
    request: &AnalysisRequest,
) -> Vec<super::client::ChatMessage> {
    let system = format!(
        r#"你是一个小学题目结构化分析专家。你的任务是基于视觉识别结果，生成标准化的题目结构。

年级：{grade}{subject}

请严格以 JSON 格式输出，包含以下字段：
{{
  "question_markdown_clean": "清洗后的完整题目 Markdown（去除所有批注和作答，可重新打印）",
  "question_structure": {{
    "question_type": "选择题|填空题|判断题|计算题|应用题|阅读理解|写作题|其他",
    "has_sub_questions": true,
    "sub_questions": [
      {{
        "index": 1,
        "type": "选择题",
        "stem": "小题题干",
        "options": ["A. ...", "B. ...", "C. ...", "D. ..."]
      }}
    ]
  }},
  "image_regions": [[x1, y1, x2, y2]],
  "difficulty": "容易|中等|较难|困难",
  "estimated_grade": "二年级"
}}

其中：
- question_markdown_clean: 完整的纯净题目，用 Markdown 格式，包括题号、题干、选项、配图标记等
- question_structure: 题目的结构化拆分
- image_regions: 从视觉识别结果继承的配图坐标
- difficulty: 基于题目内容和年级评估的难度等级
- estimated_grade: 预估适用的年级

注意：
1. question_markdown_clean 必须完整到可以重新用于打印
2. 如果有公共题干（如阅读理解的文章），必须包含
3. 只输出 JSON，不要输出其他内容"#,
        grade = request
            .grade_level
            .as_deref()
            .unwrap_or(&config.defaults.grade_level),
        subject = request
            .subject
            .as_deref()
            .map(|s| format!("\n科目：{}", s))
            .unwrap_or_default(),
    );

    vec![super::client::ChatMessage::system(&system)]
}

/// Stage 2 user text (wraps vision JSON output for the next model)
pub fn structured_extraction_user_text(vision_json: &str) -> String {
    format!(
        "以下是视觉识别阶段的结果，请基于此生成结构化题目：\n\n{}",
        vision_json
    )
}

/// Stage 3: Pedagogical analysis prompt — error cause, suggestions, knowledge points.
///
/// Input: structured extraction JSON. Output: JSON with analysis.
pub fn build_pedagogical_analysis_prompt(
    config: &AppConfig,
    request: &AnalysisRequest,
) -> Vec<super::client::ChatMessage> {
    let system = format!(
        r#"你是一个经验丰富的小学教师，专门分析学生的错题。

年级：{grade}
科目：{subject_or_auto}

请基于结构化题目信息和学生的作答，进行教学分析。

请严格以 JSON 格式输出，包含以下字段：
{{
  "subject": "科目",
  "classification": ["知识点标签1", "知识点标签2"],
  "error_type": "概念错误|计算错误|审题错误|表达错误|粗心错误|方法错误|其他",
  "error_subtype": "更细分的错误类型描述",
  "error_reason": "详细的错误原因分析",
  "suggestions": "可执行的改进建议",
  "root_cause_code": "CONCEPT|CALCULATION|READING|EXPRESSION|CARELESS|METHOD|OTHER",
  "confidence": {{
    "subject": 0.95,
    "error_type": 0.8,
    "error_reason": 0.85
  }}
}}

其中：
- subject: 科目（语文、数学等），如果输入有说明以输入为准
- classification: 知识点标签数组，如 ["带余数除法", "表内除法"]
- error_type: 错误大类
- error_subtype: 错误子类（更具体，如"进位加法忘记进位"）
- error_reason: 详细分析学生为什么做错
- suggestions: 具体可执行的改进建议
- root_cause_code: 根因编码
- confidence: 各字段的分析置信度

注意：
1. classification 要具体到可操作的知识点
2. error_reason 要结合学生实际答案分析，不要泛泛而谈
3. suggestions 要具体可执行，不要空泛建议
4. 只输出 JSON，不要输出其他内容"#,
        grade = request
            .grade_level
            .as_deref()
            .unwrap_or(&config.defaults.grade_level),
        subject_or_auto = request.subject.as_deref().unwrap_or("根据题目自动判断"),
    );

    vec![super::client::ChatMessage::system(&system)]
}

/// Stage 3 user text (wraps extraction JSON + student answer for the next model)
pub fn pedagogical_analysis_user_text(extraction_json: &str, student_answer: &str) -> String {
    format!(
        "以下是结构化题目信息：\n\n{}\n\n学生作答内容：\n{}\n\n请分析这道错题。",
        extraction_json,
        if student_answer.is_empty() {
            "（未识别到学生作答）"
        } else {
            student_answer
        }
    )
}
