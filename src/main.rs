use anyhow::{Context, Result};
use chrono::NaiveDateTime;
use clap::Parser;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::sync::Arc;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use error_book::cli::commands::{Cli, Command};
use error_book::config::AppConfig;
use error_book::db::migration;
use error_book::llm::client::{ChatClient, EmbeddingClient};
use error_book::storage::image::ImageStorage;
use error_book::analysis::analyzer::Analyzer;
use error_book::db::repository::Repository;
use error_book::db::models::{ErrorRecord, ErrorRecordWithScore};
use error_book::summary::generator::SummaryGenerator;
use error_book::summary::image_generator::SummaryImageGenerator;
use error_book::practice::generator::PracticeGenerator;
use error_book::mcp::server::{McpHandler, run_mcp_server};

fn format_timestamp(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| ts.to_string())
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}...", truncated)
    }
}

async fn open_database(config: &AppConfig) -> Result<libsql::Database> {
    let db = libsql::Builder::new_local(&config.database.url)
        .build()
        .await
        .with_context(|| format!("打开数据库失败: {}", config.database.url))?;
    Ok(db)
}

fn init_logging(config: &AppConfig) -> Result<()> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.logging.level.clone()));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_ansi(true);

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer);

    if let Some(path) = &config.logging.file {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("打开日志文件失败: {}", path.display()))?;
        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(file)
            .with_ansi(false);
        registry.with(file_layer).init();
    } else {
        registry.init();
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // 加载配置
    let config = AppConfig::load(&cli.config)?;
    config.ensure_dirs()?;
    init_logging(&config)?;

    // 初始化数据库
    let db = Arc::new(open_database(&config).await?);
    migration::run_migration(&db).await?;

    // 初始化组件
    let repository = Repository::new(Arc::clone(&db));
    let chat_client = ChatClient::new(&config);
    let embedding_client = EmbeddingClient::new(&config);
    let image_storage = ImageStorage::new(config.storage.image_dir.clone());
    let generated_image_storage = ImageStorage::new(config.storage.generated_image_dir.clone());

    let search_config = config.search.clone();
    let pdf_config = config.pdf.clone();

    match cli.command {
        Command::Analyze { .. } => {
            let request = cli.command.to_analysis_request()
                .expect("Analyze command should produce request");
            let analyzer = Analyzer::new(
                config,
                chat_client,
                embedding_client,
                image_storage,
                repository,
            );
            let record = analyzer.analyze(request).await?;

            // 输出结果
            println!("════════════════════════════════════════");
            println!("✅ 错题分析完成");
            println!("════════════════════════════════════════");
            println!("ID:     {}", record.id);
            println!("科目:   {}", record.subject);
            println!("年级:   {}", record.grade_level);
            println!("知识点: {}", record.classification);
            println!();
            println!("── 原题 ──");
            println!("{}", record.original_question);
            println!();
            println!("── 错误原因 ──");
            println!("{}", record.error_reason);
            println!();
            println!("── 改进建议 ──");
            println!("{}", record.suggestions);
        }

        Command::Show { id } => {
            let record = repository.get_error_record(&id).await?;
            match record {
                Some(r) => {
                    let date = format_timestamp(r.created_at);
                    println!("════════════════════════════════════════");
                    println!("ID:     {}", r.id);
                    println!("科目:   {}", r.subject);
                    println!("年级:   {}", r.grade_level);
                    println!("知识点: {}", r.classification);
                    println!("时间:   {}", date);
                    if !r.image_path.is_empty() {
                        println!("图片:   {}", image_storage.full_path(&r.image_path).display());
                    }
                    println!();
                    println!("── 原题 ──");
                    println!("{}", r.original_question);
                    println!();
                    println!("── 错误原因 ──");
                    println!("{}", r.error_reason);
                    println!();
                    println!("── 改进建议 ──");
                    println!("{}", r.suggestions);
                }
                None => {
                    println!("未找到记录: {}", id);
                }
            }
        }

        Command::List { subject, from, to, limit } => {
            let from_dt: Option<NaiveDateTime> = from.as_deref()
                .map(|s: &str| Command::parse_date(s))
                .transpose()?
                .map(|d: chrono::NaiveDate| d.and_hms_opt(0, 0, 0).unwrap());

            let to_dt: Option<NaiveDateTime> = to.as_deref()
                .map(|s: &str| Command::parse_date(s))
                .transpose()?
                .map(|d: chrono::NaiveDate| d.and_hms_opt(23, 59, 59).unwrap());

            let records: Vec<ErrorRecord> = repository.list_error_records(
                subject.as_deref(),
                from_dt,
                to_dt,
                Some(limit),
            ).await?;

            if records.is_empty() {
                println!("没有找到错题记录");
            } else {
                println!("共 {} 条错题记录：\n", records.len());
                for r in &records {
                    let date = chrono::DateTime::from_timestamp(r.created_at, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_default();
                    let tags: Vec<String> = serde_json::from_str(&r.classification).unwrap_or_default();
                    let tags_display = tags.join("、");
                    println!("{} | {} | {} | {}", r.id, r.subject, truncate(&tags_display, 24), date);
                }
            }
        }

        Command::ListSummaries { subject, limit } => {
            let summaries = repository.list_summaries(subject.as_deref(), Some(limit)).await?;

            if summaries.is_empty() {
                println!("没有找到总结记录");
            } else {
                println!("共 {} 条总结记录：\n", summaries.len());
                for s in &summaries {
                    let created_at = format_timestamp(s.created_at);
                    let from_str = chrono::DateTime::from_timestamp(s.period_start, 0)
                        .map(|dt| dt.format("%Y-%m-%d").to_string())
                        .unwrap_or_default();
                    let to_str = chrono::DateTime::from_timestamp(s.period_end, 0)
                        .map(|dt| dt.format("%Y-%m-%d").to_string())
                        .unwrap_or_default();
                    println!(
                        "{} | {} | {} | {}~{} | {}",
                        s.id,
                        s.subject,
                        s.period_type,
                        from_str,
                        to_str,
                        created_at
                    );
                }
            }
        }

        Command::ListPractices { subject, summary_id, limit } => {
            let practices = repository
                .list_practice_sets(subject.as_deref(), summary_id.as_deref(), Some(limit))
                .await?;

            if practices.is_empty() {
                println!("没有找到练习题记录");
            } else {
                println!("共 {} 条练习题记录：\n", practices.len());
                for p in &practices {
                    let created_at = format_timestamp(p.created_at);
                    let questions: Vec<error_book::analysis::parser::PracticeQuestion> =
                        serde_json::from_str(&p.questions).unwrap_or_default();
                    let pdf_status = p.pdf_path.as_deref().unwrap_or("-");
                    println!(
                        "{} | {} | {} | {}题 | {} | {} | {}",
                        p.id,
                        truncate(&p.summary_id, 12),
                        p.subject,
                        questions.len(),
                        truncate(p.requirements.as_deref().unwrap_or("-"), 20),
                        truncate(pdf_status, 24),
                        created_at
                    );
                }
            }
        }

        Command::Search { query, image, with_image, subject, limit } => {
            // query 和 image 至少要提供一个
            if query.is_none() && image.is_none() {
                anyhow::bail!("搜索需要提供 --query 文本或 --image 图片（或两者同时提供）");
            }

            let query_text = query.as_deref().unwrap_or("");

            if image.is_some() && !embedding_client.supports_image_embedding() {
                anyhow::bail!(
                    "当前 embedding provider 不支持图片搜索；请将 llm.embedding.provider 设置为 google"
                );
            }

            // 有图片输入时，读取图片
            let image_data: Option<(String, &'static str)> = if let Some(ref image_path) = image {
                let path = Path::new(image_path);
                if !path.exists() {
                    anyhow::bail!("图片文件不存在: {}", image_path.display());
                }
                let image_bytes = tokio::fs::read(path).await
                    .with_context(|| format!("读取搜索图片失败: {}", image_path.display()))?;
                let image_base64 = base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD, &image_bytes
                );
                let media_type = detect_media_type(path);
                Some((image_base64, media_type))
            } else {
                None
            };

            // 决定搜索模式
            let results: Vec<ErrorRecordWithScore> = if with_image && image_data.is_some() {
                // 混合搜索模式：文本向量 + 图片向量加权融合
                let (ref img_b64, img_media) = image_data.unwrap();
                println!("🔍 混合搜索（文本+图片）...");
                let text_emb = if !query_text.is_empty() {
                    Some(embedding_client.embed(query_text).await?)
                } else {
                    None
                };
                let image_emb = Some(embedding_client.embed_image_only(img_b64, img_media).await?);
                let image_weight = search_config.image_weight;
                let text_weight = 1.0 - image_weight;
                println!("   权重: 文本={:.1}, 图片={:.1}", text_weight, image_weight);
                repository.search_mixed(
                    text_emb.as_deref(),
                    image_emb.as_deref(),
                    text_weight,
                    limit,
                    subject.as_deref(),
                ).await.context("混合搜索失败")?
            } else if let Some((ref img_b64, img_media)) = image_data {
                // 纯图片搜索（搜 text_embedding 列）
                println!("🔍 图片搜索相似错题...");
                let image_emb = embedding_client.embed_image_only(img_b64, img_media).await?;
                repository.search_by_text_vector(&image_emb, limit, subject.as_deref())
                    .await.context("向量搜索失败")?
            } else {
                // 纯文本搜索（搜 text_embedding 列）
                println!("🔍 语义搜索: \"{}\"", query_text);
                let text_emb = embedding_client.embed(query_text).await?;
                repository.search_by_text_vector(&text_emb, limit, subject.as_deref())
                    .await.context("向量搜索失败")?
            };

            if results.is_empty() {
                println!("没有找到匹配的错题");
            } else {
                println!("共 {} 条匹配结果（按相似度排序）:", results.len());
                println!("────────────────────────────────────────────────");
                for r in &results {
                    let record = &r.record;
                    let similarity = r.similarity();
                    let question_one_line = record.original_question.replace('\n', " ");
                    let reason_one_line = record.error_reason.replace('\n', " ");
                    let tags: Vec<String> = serde_json::from_str(&record.classification).unwrap_or_default();
                    let tags_display = tags.join("、");

                    println!("{} | {:.4} | {} | {}", record.id, similarity, record.subject, truncate(&tags_display, 20));
                    if !record.image_path.is_empty() {
                        println!("    图片: {}", image_storage.full_path(&record.image_path).display());
                    }
                    println!("    原题: {}", truncate(&question_one_line, 80));
                    println!("    原因: {}", truncate(&reason_one_line, 80));
                }
            }
        }

        Command::Summary { subject, from, to, period_type } => {
            let from_date = Command::parse_date(&from)?;
            let to_date = Command::parse_date(&to)?;
            let from_dt = from_date.and_hms_opt(0, 0, 0).unwrap();
            let to_dt = to_date.and_hms_opt(23, 59, 59).unwrap();

            let generator = SummaryGenerator::new(
                config,
                chat_client,
                repository,
            );
            println!("正在生成阶段性总结...");
            let summary = generator.generate(
                &subject,
                from_dt,
                to_dt,
                &period_type,
            ).await?;

            let from_str = from_date.format("%Y-%m-%d");
            let to_str = to_date.format("%Y-%m-%d");

            println!("════════════════════════════════════════");
            println!("✅ 阶段性总结生成完成");
            println!("════════════════════════════════════════");
            println!("总结 ID:  {}", summary.id);
            println!("科目:     {}", summary.subject);
            println!("时间段:   {} ~ {}", from_str, to_str);
            println!("总结类型: {}", summary.period_type);
            println!();
            println!("── 共性错误原因 ──");
            println!("{}", summary.common_reasons);
            println!();
            println!("── 共性改进建议 ──");
            println!("{}", summary.common_suggestions);
            println!();
            println!("── 薄弱知识点 ──");
            let weak_points: Vec<String> = serde_json::from_str(&summary.weak_points)
                .unwrap_or_default();
            for (i, wp) in weak_points.iter().enumerate() {
                println!("  {}. {}", i + 1, wp);
            }
            println!();
            println!("── 详细分析 ──");
            println!("{}", summary.detail);
        }

        Command::SummaryImage { summary_id, requirements } => {
            let generator = SummaryImageGenerator::new(
                config,
                repository,
                generated_image_storage,
            );
            println!("正在生成阶段性总结信息图...");
            let image = generator.generate(&summary_id, requirements.as_deref()).await?;

            println!("════════════════════════════════════════");
            println!("✅ 阶段性总结信息图生成完成");
            println!("════════════════════════════════════════");
            println!("图片 ID:   {}", image.record.id);
            println!("总结 ID:   {}", image.record.summary_id);
            println!("图片格式: {}", image.record.mime_type);
            println!("相对路径: {}", image.record.image_path);
            println!("完整路径: {}", image.full_path.display());
        }

        Command::Practice { summary_id, count, requirements, output } => {
            let generator = PracticeGenerator::new(
                config,
                chat_client,
                repository,
            );
            println!("正在生成巩固练习题...");
            let pdf_path_str = output.as_ref().map(|p| p.to_string_lossy().into_owned());
            let practice = generator.generate(
                &summary_id,
                count,
                requirements.as_deref(),
                pdf_path_str.as_deref(),
            ).await?;

            let questions: Vec<error_book::analysis::parser::PracticeQuestion> =
                serde_json::from_str(&practice.questions).unwrap_or_default();

            println!("════════════════════════════════════════");
            println!("✅ 巩固练习生成完成");
            println!("════════════════════════════════════════");
            println!("练习 ID:  {}", practice.id);
            println!("总结 ID:  {}", practice.summary_id);
            println!("科目:     {}", practice.subject);
            println!("题目数:   {}", questions.len());
            if let Some(req) = practice.requirements.as_deref() {
                println!("额外要求: {}", req);
            }
            println!();

            for (i, q) in questions.iter().enumerate() {
                println!("── 第 {} 题 ──", i + 1);
                println!("{}", q.question);
                println!("答案: {}", q.answer);
                println!("知识点: {}", q.knowledge_points.join("、"));
                println!();
            }

            // 生成 PDF（如果指定了 --output）
            if let Some(ref output_path) = output {
                let pdf_output = error_book::pdf::generate_pdf(
                    &practice,
                    &pdf_config,
                    &output_path.to_string_lossy(),
                )?;
                println!("📄 PDF 已生成: {}", pdf_output.path);
            }
        }

        Command::PracticePdf { id, output } => {
            let practice = repository.get_practice_set(&id).await?
                .ok_or_else(|| anyhow::anyhow!("练习集不存在: {}", id))?;

            let questions: Vec<error_book::analysis::parser::PracticeQuestion> =
                serde_json::from_str(&practice.questions).unwrap_or_default();

            println!("正在从已存储的练习集生成 PDF...");
            println!("练习 ID:  {}", practice.id);
            println!("科目:     {}", practice.subject);
            println!("题目数:   {}", questions.len());

            let pdf_output = error_book::pdf::generate_pdf(
                &practice,
                &pdf_config,
                &output.to_string_lossy(),
            )?;
            println!("📄 PDF 已生成: {}", pdf_output.path);

            // 更新数据库中的 pdf_path
            repository.update_practice_set_pdf_path(&id, &pdf_output.path).await?;
        }

        Command::Mcp => {
            let handler = McpHandler::new(
                config,
                db,
                chat_client,
                embedding_client,
                image_storage,
            );
            run_mcp_server(handler).await?;
        }
    }

    Ok(())
}

fn detect_media_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}
