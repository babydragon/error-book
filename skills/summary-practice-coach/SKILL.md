# Summary and Practice Coach

Use this skill when the user wants to review accumulated wrong answers, generate a summary, create practice questions, or export PDF.

## Primary tools

- `generate_summary`
- `show_summary`
- `generate_practice`
- `show_practice`
- `generate_practice_pdf`

## Discovery / resume tools

- `list_errors`
- `list_summaries`
- `list_practices`
- `search_errors`

## Canonical workflow

1. Generate a summary with `generate_summary(subject, from, to, period_type)`.
2. Immediately call `show_summary(summary_id)` and use that as the authoritative summary content.
3. If the user wants practice, call `generate_practice(summary_id, count, requirements)`.
4. Immediately call `show_practice(practice_id)` and use that as the authoritative practice content.
5. Only call `generate_practice_pdf(practice_id, output_path)` when the user explicitly wants file export or re-export.

## Rules

- Use this skill only when there are enough accumulated records for review.
- Use `list_*` only to discover existing IDs.
- Prefer `show_summary` / `show_practice` over parsing list output.
- If the user already provides `summary_id`, skip summary generation.
- If the user already provides `practice_id`, skip practice generation and go straight to `show_practice` or `generate_practice_pdf`.
- Avoid regenerating summary/practice if an existing ID already satisfies the request.
