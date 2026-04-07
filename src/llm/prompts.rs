use crate::config::AppConfig;
use crate::db::models::AnalysisRequest;

use crate::analysis::parser::PracticeQuestion;

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

要求：
1. 归纳共性错误原因（按频次排序）
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
3. 输出必须是一个 JSON 数组
4. 第一个字符必须是 [，最后一个字符必须是 ]
5. 必须且只能输出 {} 个对象，不能多也不能少
6. 每个对象必须且只能包含以下 3 个字段：
   - question: 字符串，题目内容，允许使用 markdown，但必须作为 JSON 字符串输出
   - answer: 字符串，参考答案
   - knowledge_points: 字符串数组，表示该题考查的知识点
7. 所有字段名必须完全一致，不能增加其他字段，不能省略字段
8. 如果 question 中需要换行，请使用 \n；如果内容中出现英文双引号，必须正确转义为 \"
9. knowledge_points 必须是 JSON 字符串数组，即使只有 1 个知识点也必须输出数组
10. 不要输出任何 emoji、表情符号、贴纸风格符号或装饰性 pictograph，不要使用 ✅📚⭐🎯😀 等字符

输出示例：
[
  {{
    "question": "1. 计算：12 ÷ 3 = ?",
    "answer": "4",
    "knowledge_points": ["表内除法"]
  }}
]"#,
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
        "薄弱知识点：{}\n\n参考题目风格：\n{}{}\n\n请生成 {} 道巩固练习题。请再次确认：最终回复只能是合法 JSON 数组，且数组长度必须恰好为 {}。",
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
3. 输出必须是一个 JSON 数组
4. 第一个字符必须是 [，最后一个字符必须是 ]
5. 必须且只能输出 {} 个对象，不能多也不能少
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
        "薄弱知识点：{}\n\n以下题目已经生成，禁止重复：\n{}{}\n\n现在请只补生成剩余 {} 道新题。最终回复只能是合法 JSON 数组，且数组长度必须恰好为 {}。",
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
