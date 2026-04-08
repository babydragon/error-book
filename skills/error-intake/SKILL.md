---
name: error-intake
version: 1
description: Analyze and review newly recorded wrong-answer items without entering summary/practice workflows too early.
---

# Error Intake

Use this skill when the user is currently录入/分析新的错题，而不是做阶段性总结或生成练习。

## Primary tools

- `analyze_error`
- `get_job_status`
- `get_job_result`
- `show_error`

## Discovery tools

- `list_errors`
- `list_jobs`
- `search_errors`

## Workflow

1. For each new wrong-answer image, call `analyze_error` and record the returned `job_id`.
2. Poll `get_job_status(job_id)` until the job is `succeeded` or `failed`.
3. Call `get_job_result(job_id)` to obtain the final error record payload, then use `show_error(id)` as the authoritative stored content.
4. If the user wants to revisit an existing record, use `show_error(id)`.
5. Use `list_errors`, `list_jobs`, or `search_errors` only to discover or recall existing records/tasks.

## Rules

- This skill is for intake and single-record review.
- Do not generate summaries or practice unless the user explicitly switches to a review/study phase.
- Prefer `show_error` over relying on list output.
- Prefer `get_job_result` + `show_error` over waiting on a single long MCP call.
- Preserve returned `error_record.id` for later summary workflows.
