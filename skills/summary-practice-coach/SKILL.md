---
name: summary-practice-coach
version: 1
description: Generate summaries, create practice sets, and export PDFs once enough wrong-answer records have accumulated.
---

# Summary and Practice Coach

Use this skill when the user wants to review accumulated wrong answers, generate a summary, create practice questions, or export PDF.

## Primary tools

- `generate_summary`
- `get_job_status`
- `get_job_result`
- `show_summary`
- `generate_practice`
- `show_practice`
- `generate_practice_pdf`

## Discovery / resume tools

- `list_errors`
- `list_jobs`
- `list_summaries`
- `list_practices`
- `search_errors`

## Canonical workflow

1. Generate a summary with `generate_summary(subject, from, to, period_type)`.
2. Poll `get_job_status(job_id)` until the job is `succeeded` or `failed`.
3. Call `get_job_result(job_id)` to obtain the final summary payload, then use `show_summary(summary_id)` as the authoritative stored content.
4. If the user wants practice, call `generate_practice(summary_id, count, requirements)`.
5. Poll `get_job_status(job_id)` until the job is `succeeded` or `failed`.
6. Call `get_job_result(job_id)` to obtain the final practice payload, then use `show_practice(practice_id)` as the authoritative stored content.
7. Only call `generate_practice_pdf(practice_id, output_path)` when the user explicitly wants file export or re-export.

## Rules

- Use this skill only when there are enough accumulated records for review.
- Use `list_*` only to discover existing IDs or resume unfinished jobs.
- Prefer `show_summary` / `show_practice` over parsing list output.
- If the user already provides `summary_id`, skip summary generation.
- If the user already provides `practice_id`, skip practice generation and go straight to `show_practice` or `generate_practice_pdf`.
- If the user already provides `job_id`, prefer `get_job_status` / `get_job_result` over submitting a duplicate task.
- Avoid regenerating summary/practice if an existing ID already satisfies the request.
