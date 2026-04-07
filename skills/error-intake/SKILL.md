# Error Intake

Use this skill when the user is currently录入/分析新的错题，而不是做阶段性总结或生成练习。

## Primary tools

- `analyze_error`
- `show_error`

## Discovery tools

- `list_errors`
- `search_errors`

## Workflow

1. For each new wrong-answer image, call `analyze_error`.
2. If the user wants to revisit an existing record, use `show_error(id)`.
3. Use `list_errors` or `search_errors` only to discover or recall existing records.

## Rules

- This skill is for intake and single-record review.
- Do not generate summaries or practice unless the user explicitly switches to a review/study phase.
- Prefer `show_error` over relying on list output.
- Preserve returned `error_record.id` for later summary workflows.
