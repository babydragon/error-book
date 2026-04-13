use std::path::PathBuf;

use anyhow::Result;
use chrono::NaiveDate;
use clap::{Parser, Subcommand};

use crate::db::models::AnalysisRequest;

#[derive(Parser)]
#[command(
    name = "error-book",
    version,
    about = "基于AI的错题本：解析、分析、总结错题"
)]
pub struct Cli {
    /// 配置文件路径
    #[arg(short, long, default_value = "config.toml", global = true)]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// 分析错题图片
    Analyze {
        /// 错题图片路径
        image: PathBuf,
        /// 科目（可选，不指定则由AI判断）
        #[arg(short, long)]
        subject: Option<String>,
        /// 年级（可选，默认从配置读取）
        #[arg(short, long)]
        grade: Option<String>,
        /// 老师批改颜色（默认红色）
        #[arg(long)]
        color_teacher: Option<String>,
        /// 订正颜色（默认蓝色）
        #[arg(long)]
        color_correction: Option<String>,
    },

    /// 查看错题详情
    Show {
        /// 错题记录 ID
        id: String,
    },

    /// 列出错题记录
    List {
        /// 按科目筛选
        #[arg(short, long)]
        subject: Option<String>,
        /// 起始日期 (YYYY-MM-DD)
        #[arg(long)]
        from: Option<String>,
        /// 结束日期 (YYYY-MM-DD)
        #[arg(long)]
        to: Option<String>,
        /// 返回条数限制
        #[arg(short, long, default_value = "20")]
        limit: u32,
    },

    /// 列出阶段性总结
    ListSummaries {
        /// 按科目筛选
        #[arg(short, long)]
        subject: Option<String>,
        /// 返回条数限制
        #[arg(short, long, default_value = "20")]
        limit: u32,
    },

    /// 列出已生成的练习题
    ListPractices {
        /// 按科目筛选
        #[arg(short, long)]
        subject: Option<String>,
        /// 按总结记录筛选
        #[arg(long)]
        summary_id: Option<String>,
        /// 返回条数限制
        #[arg(short, long, default_value = "20")]
        limit: u32,
    },

    /// 语义搜索错题
    Search {
        /// 搜索文本
        #[arg(short, long)]
        query: Option<String>,
        /// 搜索图片（通过图片搜索相似错题，与 query 二选一或组合使用）
        #[arg(short, long)]
        image: Option<PathBuf>,
        /// 同时搜索图片向量（需配合 --image 使用，开启混合搜索模式）
        #[arg(long)]
        with_image: bool,
        /// 按科目筛选
        #[arg(short = 's', long)]
        subject: Option<String>,
        /// 返回条数限制
        #[arg(short, long, default_value = "10")]
        limit: u32,
    },

    /// 生成阶段性总结
    Summary {
        /// 科目
        #[arg(short, long)]
        subject: String,
        /// 起始日期 (YYYY-MM-DD)
        #[arg(long)]
        from: String,
        /// 结束日期 (YYYY-MM-DD)
        #[arg(long)]
        to: String,
        /// 总结类型
        #[arg(short = 't', long, default_value = "week")]
        period_type: String,
    },

    /// 基于阶段性总结生成帮助记忆的信息图
    SummaryImage {
        /// 总结记录 ID
        #[arg(long)]
        summary_id: String,
        /// 补充要求（如风格、配色、版式等）
        #[arg(short = 'r', long)]
        requirements: Option<String>,
    },

    /// 生成巩固练习
    Practice {
        /// 总结记录 ID
        #[arg(long)]
        summary_id: String,
        /// 题目数量
        #[arg(short = 'n', long, default_value = "10")]
        count: u32,
        /// 额外要求（如题型、难度、特殊限制等）
        #[arg(short = 'r', long)]
        requirements: Option<String>,
        /// PDF 输出路径
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// 从已存储的练习集重新生成 PDF（不调用 LLM）
    PracticePdf {
        /// 练习集 ID
        #[arg(long)]
        id: String,
        /// PDF 输出路径
        #[arg(short, long)]
        output: PathBuf,
    },

    /// 启动 MCP Server (stdio 模式)
    Mcp,
}

/// 将 CLI Analyze 命令转换为 AnalysisRequest
impl Command {
    pub fn to_analysis_request(&self) -> Option<AnalysisRequest> {
        match self {
            Command::Analyze {
                image,
                subject,
                grade,
                color_teacher,
                color_correction,
            } => Some(AnalysisRequest {
                image_path: image.to_string_lossy().to_string(),
                subject: subject.clone(),
                grade_level: grade.clone(),
                color_teacher: color_teacher.clone(),
                color_correction: color_correction.clone(),
            }),
            _ => None,
        }
    }

    pub fn parse_date(s: &str) -> Result<NaiveDate> {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|e| anyhow::anyhow!("日期格式错误 (需要 YYYY-MM-DD): {}", e))
    }
}
