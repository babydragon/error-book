# Error Book (错题本)

基于 AI 的智能错题本，支持错题图片识别、错误原因分析、阶段性总结、巩固练习生成，并输出 PDF。

## 功能概览

| 功能 | CLI | MCP Server |
|------|-----|------------|
| 分析错题图片 | ✅ | ✅ |
| 查看错题详情 | ✅ | ✅ |
| 列出错题记录 | ✅ | ✅ |
| 语义搜索错题 | ✅ | ✅ |
| 生成阶段性总结 | ✅ | ✅ |
| 生成巩固练习 | ✅ | ✅ |
| 输出练习 PDF | ✅ | ✅ |
| 从已存储练习集导出 PDF | ✅ | — |

## 技术栈

- **语言**: Rust
- **大模型**: OpenAI 兼容 API（支持自定义 base_url / key）
- **Embedding**: 多模态 embedding（文本 + 图片）
- **数据库**: libsql（支持向量存储）
- **PDF**: Typst (typst + typst-pdf)
- **MCP**: rmcp（stdio 传输）

## 构建

```bash
cargo build --release
```

交叉编译（RISC-V）：

```bash
cargo zigbuild --target riscv64gc-unknown-linux-gnu --release
```

## 配置

创建 `config.toml`：

```toml
[llm.chat]
provider = "openai"
base_url = "https://your-chat-api-endpoint/v1"
api_key = "your-chat-api-key"
model = "gemini-3.1-pro-preview"

[llm.embedding]
provider = "google"
base_url = "https://your-embedding-api-endpoint"
api_key = "your-embedding-api-key"
model = "gemini-embedding-2-preview"
dimensions = 1536

[llm.retry]
max_attempts = 5
base_delay_ms = 500
max_delay_ms = 30000
retryable_status_codes = [429, 500, 502, 503, 504]

[database]
url = "./data/error_book.db"
# auth_token = "your-token"

[storage]
image_dir = "./data/images"
pdf_dir = "./data/pdfs"

[defaults]
grade_level = "二年级"

[pdf]
# 必填：Typst 渲染使用的字体文件，程序启动时会校验其存在性与有效性
font_path = "./fonts/NotoSansSC-Regular.ttf"

[search]
# 混合搜索时图片 embedding 的权重 (0.0~1.0)，文本权重 = 1.0 - image_weight
image_weight = 0.3

[logging]
# 默认日志级别
level = "info"
# 可选：日志文件路径。未配置时仅输出到 stderr
# file = "./data/error-book.log"
```

### 环境变量覆盖

| 环境变量 | 说明 |
|---------|------|
| `ERROR_BOOK_CHAT_API_KEY` | 覆盖 `llm.chat.api_key` |
| `ERROR_BOOK_EMBEDDING_API_KEY` | 覆盖 `llm.embedding.api_key` |
| `ERROR_BOOK_CHAT_BASE_URL` | 覆盖 `llm.chat.base_url` |
| `ERROR_BOOK_CHAT_PROVIDER` | 覆盖 `llm.chat.provider`（`google`/`openai`） |
| `ERROR_BOOK_EMBEDDING_BASE_URL` | 覆盖 `llm.embedding.base_url` |
| `ERROR_BOOK_EMBEDDING_PROVIDER` | 覆盖 `llm.embedding.provider`（`google`/`openai`） |
| `ERROR_BOOK_LLM_API_KEY` | 同时覆盖 `llm.chat.api_key` 和 `llm.embedding.api_key` |
| `ERROR_BOOK_LLM_BASE_URL` | 同时覆盖 `llm.chat.base_url` 和 `llm.embedding.base_url` |
| `ERROR_BOOK_DB_URL` | 覆盖 `database.url` |

说明：
- chat 和 embedding 现在可以分别配置不同来源的 `base_url`、`api_key` 和 `model`
- `llm.chat.provider` 用于选择 chat 协议，支持 `openai` / `google`
- `llm.embedding.provider` 用于选择 embedding 协议，目前支持 `google` / `openai`
- `llm.retry` 仍然是两者共用
- chat: `openai` 与 `google` 都已实现
- embedding: 当前仅 `provider = "google"` 已实现；`openai` 预留但暂未实现
- 当 `provider = "google"` 时，`base_url` 应填写 Google 原生接口根地址，而不是 `/openai/` 兼容地址
- `pdf.font_path` 为必填项；程序启动时会校验字体文件存在且可解析，不再使用默认回退字体
- 日志默认写入 stderr；可通过 `logging.level` 配置日志级别，并通过 `logging.file` 追加写入日志文件

Chat 配置示例：

```toml
# OpenAI 兼容模式（默认）
[llm.chat]
provider = "openai"
base_url = "https://your-openai-compatible-endpoint/v1"
api_key = "your-chat-api-key"
model = "gemini-3.1-pro-preview"

# Google AI Studio 原生模式
[llm.chat]
provider = "google"
base_url = "https://generativelanguage.googleapis.com"
api_key = "your-google-api-key"
model = "gemini-3.1-pro-preview"
```

### 字体准备

PDF 生成需要中文字体，请将字体文件放到 `fonts/` 目录：

- 推荐：`NotoSansSC-Regular.ttf`
- 或：`Alibaba-PuHuiTi-Regular.otf`

## CLI 用法

所有命令通过 `--config` 指定配置文件路径，默认为当前目录下的 `config.toml`。

### 分析错题图片

```bash
error-book analyze <图片路径> [选项]
```

| 选项 | 说明 |
|------|------|
| `-s, --subject <科目>` | 指定科目（不指定则由 AI 判断） |
| `-g, --grade <年级>` | 指定年级（默认从配置读取） |
| `--color-teacher <颜色>` | 老师批改颜色（默认红色） |
| `--color-correction <颜色>` | 订正颜色（默认蓝色） |

示例：

```bash
# 分析单张错题图片
error-book analyze ./homework/math_error.jpg

# 指定科目和年级
error-book analyze ./homework/math_error.jpg -s 数学 -g 三年级

# 使用自定义配置
error-book --config /path/to/config.toml analyze ./error.png
```

### 查看错题详情

```bash
error-book show <记录ID>
```

示例：

```bash
error-book show abc12345-xxxx-xxxx-xxxx-xxxxxxxxxxxx
```

### 列出错题记录

```bash
error-book list [选项]
```

| 选项 | 说明 |
|------|------|
| `-s, --subject <科目>` | 按科目筛选 |
| `--from <日期>` | 起始日期 (YYYY-MM-DD) |
| `--to <日期>` | 结束日期 (YYYY-MM-DD) |
| `-l, --limit <数量>` | 返回条数限制（默认 20） |

示例：

```bash
# 列出所有错题
error-book list

# 查看数学错题
error-book list -s 数学

# 查看指定时间范围
error-book list --from 2025-03-01 --to 2025-03-31 -s 数学
```

### 列出总结记录

```bash
error-book list-summaries [选项]
```

| 选项 | 说明 |
|------|------|
| `-s, --subject <科目>` | 按科目筛选 |
| `-l, --limit <数量>` | 返回条数限制（默认 20） |

示例：

```bash
error-book list-summaries
error-book list-summaries -s 数学 -l 10
```

### 列出练习题记录

```bash
error-book list-practices [选项]
```

| 选项 | 说明 |
|------|------|
| `-s, --subject <科目>` | 按科目筛选 |
| `--summary-id <ID>` | 按总结记录筛选 |
| `-l, --limit <数量>` | 返回条数限制（默认 20） |

示例：

```bash
error-book list-practices
error-book list-practices -s 语文 --summary-id abc12345-... -l 10
```

### 语义搜索错题

支持三种搜索模式：纯文本、纯图片、混合搜索（文本+图片）。

```bash
error-book search [选项]
```

| 选项 | 说明 |
|------|------|
| `-q, --query <文本>` | 搜索文本 |
| `-i, --image <图片>` | 搜索图片路径 |
| `--with-image` | 开启混合搜索（需配合 `--image`，同时使用文本和图片向量） |
| `-s, --subject <科目>` | 按科目筛选 |
| `-l, --limit <数量>` | 返回条数限制（默认 10） |

示例：

```bash
# 纯文本语义搜索
error-book search -q "分数加减法"

# 纯图片搜索（找相似错题）
error-book search -i ./similar_error.jpg

# 混合搜索（文本+图片加权融合）
error-book search -q "分数加减法" -i ./error.jpg --with-image
```

### 生成阶段性总结

```bash
error-book summary [选项]
```

| 选项 | 说明 |
|------|------|
| `-s, --subject <科目>` | 科目（必填） |
| `--from <日期>` | 起始日期，YYYY-MM-DD（必填） |
| `--to <日期>` | 结束日期，YYYY-MM-DD（必填） |
| `-t, --period-type <类型>` | 总结类型（默认 `week`） |

总结类型可自定义描述，如 `week`、`month`、`half-term` 等。

示例：

```bash
# 本周数学总结
error-book summary -s 数学 --from 2025-03-03 --to 2025-03-09

# 月度总结
error-book summary -s 数学 --from 2025-03-01 --to 2025-03-31 -t month
```

### 生成巩固练习

```bash
error-book practice [选项]
```

| 选项 | 说明 |
|------|------|
| `--summary-id <ID>` | 总结记录 ID（必填） |
| `-n, --count <数量>` | 题目数量（默认 10） |
| `-r, --requirements <文本>` | 额外要求，如题型、难度、特殊限制 |
| `-o, --output <路径>` | PDF 输出路径（不指定则仅输出到终端） |

示例：

```bash
# 生成练习题（终端输出）
error-book practice --summary-id abc12345-...

# 生成并导出 PDF
error-book practice --summary-id abc12345-... -n 15 -o ./practice.pdf

# 生成指定题型/难度的练习题
error-book practice --summary-id abc12345-... -r "偏重阅读理解，难度中等，不要选择题"
```

说明：通过 `--requirements` 提供的额外要求会参与出题提示词，并随该次练习记录一起保存到数据库中。

### 从已存储练习集生成 PDF

从已保存的练习集重新生成或导出 PDF，无需调用 LLM。

```bash
error-book practice-pdf [选项]
```

| 选项 | 说明 |
|------|------|
| `--id <练习集ID>` | 练习集 ID（必填） |
| `-o, --output <路径>` | PDF 输出路径（必填） |

示例：

```bash
# 从已存储的练习集生成 PDF
error-book practice-pdf --id abc12345-... -o ./practice.pdf
```

## MCP Server

启动 MCP Server（stdio 模式），供支持 MCP 协议的客户端（如 Claude Desktop、OpenClaw 等）调用：

```bash
error-book mcp
```

MCP Server 通过 stdin/stdout 通信，提供以下工具：

| 工具 | 说明 |
|------|------|
| `analyze_error` | 分析错题图片 |
| `show_error` | 查看错题详情 |
| `show_summary` | 查看总结详情 |
| `show_practice` | 查看练习详情 |
| `list_errors` | 列出错题记录 |
| `list_summaries` | 列出已生成的总结记录 |
| `list_practices` | 列出已生成的练习题记录 |
| `list_jobs` | 列出后台任务 |
| `get_job_status` | 查询后台任务状态 |
| `get_job_result` | 获取后台任务结果 |
| `search_errors` | 语义搜索错题 |
| `generate_summary` | 提交阶段性总结任务 |
| `generate_practice` | 提交巩固练习题任务（支持额外要求） |
| `generate_practice_pdf` | 按已有练习集 ID 导出 PDF |

说明：以上 MCP 工具现在统一返回 **JSON 字符串**，顶层结构为：

```json
{
  "ok": true,
  "data": { ... }
}
```

失败时返回：

```json
{
  "ok": false,
  "error": "..."
}
```

项目内还提供了两个最小工作流 skill：

- `skills/error-intake/SKILL.md`
- `skills/summary-practice-coach/SKILL.md`

适合 OpenClaw 一类需要工作流提示的客户端，分工如下：

1. **错题录入 / 单题分析**
   - `analyze_error`
   - `show_error`

2. **阶段性总结 / 生成练习 / 导出 PDF**
   - `generate_summary` -> `get_job_status` / `get_job_result` -> `show_summary`
   - `generate_practice` -> `get_job_status` / `get_job_result` -> `show_practice`
   - `generate_practice_pdf`

这样可以避免在刚录入少量错题、样本不足时，客户端过早进入“总结并生成练习”的流程。

### 在客户端中配置

以 Claude Desktop 为例，在 `claude_desktop_config.json` 中添加：

```json
{
  "mcpServers": {
    "error-book": {
      "command": "/path/to/error-book",
      "args": ["--config", "/path/to/config.toml", "mcp"]
    }
  }
}
```

## 典型工作流

```
1. 拍照/扫描错题
   └─→ error-book analyze error.jpg -s 数学

2. 查看历史错题
   └─→ error-book list -s 数学 --from 2025-03-01 --to 2025-03-07

3. 搜索相似错题
   └─→ error-book search -q "两位数乘法"
4. 阶段性总结（如每周末）
   └─→ error-book summary -s 数学 --from 2025-03-03 --to 2025-03-09
5. 生成巩固练习 + PDF
    └─→ error-book practice --summary-id <ID> -o ./练习.pdf
```

## 系统设计

### 架构总览

```
┌──────────────────────────────────────────────────────────────┐
│                        用户交互层                              │
│   ┌───────────┐                        ┌──────────────────┐  │
│   │    CLI    │                        │   MCP Server     │  │
│   │  (clap)   │                        │    (rmcp)        │  │
│   └─────┬─────┘                        └────────┬─────────┘  │
│         └──────────────┬────────────────────────┘             │
│                        ▼                                      │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │                    业务服务层                             │  │
│  │  ┌───────────┐  ┌───────────┐  ┌────────────────────┐  │  │
│  │  │  分析服务  │  │  总结服务  │  │   巩固练习服务      │  │  │
│  │  │ Analyzer  │  │ Generator │  │ PracticeGenerator  │  │  │
│  │  └─────┬─────┘  └─────┬─────┘  └─────────┬──────────┘  │  │
│  │        └──────────────┼──────────────────┘              │  │
│  │                       ▼                                  │  │
│  │  ┌───────────────────────────────────────────────────┐  │  │
│  │  │               LLM 客户端层                         │  │  │
│  │  │   ┌───────────┐          ┌─────────────┐          │  │  │
│  │  │   │  Chat API │          │  Embedding   │          │  │  │
│  │  │   │  Client   │          │   Client     │          │  │  │
│  │  │   └───────────┘          └─────────────┘          │  │  │
│  │  └───────────────────────────────────────────────────┘  │  │
│  │                       ▼                                  │  │
│  │  ┌───────────────────────────────────────────────────┐  │  │
│  │  │                 数据持久化层                        │  │  │
│  │  │   ┌──────────┐  ┌──────────┐  ┌──────────────┐   │  │  │
│  │  │   │  libsql   │  │  文件存储  │  │   PDF 输出    │   │  │  │
│  │  │   │ (向量 DB) │  │  (图片)   │  │  (Typst)     │   │  │  │
│  │  │   └──────────┘  └──────────┘  └──────────────┘   │  │  │
│  │  └───────────────────────────────────────────────────┘  │  │
│  └─────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────┘
```

### 模块结构

```
src/
├── main.rs                  # 入口：CLI 子命令分发
├── config.rs                # 配置加载与环境变量覆盖
├── pdf.rs                   # PDF 渲染输出 (Typst)
├── analysis/
│   ├── analyzer.rs          # 错题分析编排（图片→LLM→解析→embedding→入库）
│   └── parser.rs            # LLM 响应解析（markdown + JSON 容错提取）
├── summary/
│   └── generator.rs         # 阶段性总结生成
├── practice/
│   └── generator.rs         # 巩固题目生成
├── db/
│   ├── models.rs            # 数据模型（ErrorRecord / Summary / PracticeSet）
│   ├── migration.rs         # 数据库 Schema（内联 SQL）
│   └── repository.rs        # 数据访问层（CRUD + 向量搜索）
├── llm/
│   ├── client.rs            # Chat + Embedding 客户端（OpenAI 兼容 / Google 原生）
│   ├── embedding.rs         # Embedding 辅助逻辑
│   └── prompts.rs           # Prompt 模板
├── storage/
│   └── image.rs             # 图片文件存储管理
├── cli/
│   └── commands.rs          # CLI 命令定义 (clap derive)
└── mcp/
    └── server.rs            # MCP Server 工具定义与实现 (rmcp)
```

### 数据模型

```
ErrorRecord (错题记录)
├── id               String (UUID)
├── image_path       String (原图存储路径)
├── subject          String (科目)
├── grade_level      String (年级)
├── original_question String (原题 markdown)
├── image_regions    JSON   (配图坐标 [[x1,y1,x2,y2],...])
├── classification   JSON   (知识点标签 ["加法","应用题"])
├── error_reason     String (错误原因)
├── suggestions      String (改进建议)
├── text_embedding   F32_BLOB(1536) (文本向量)
├── image_embedding  F32_BLOB(1536) (图片向量)
└── created_at       Integer (Unix timestamp)
        │ 1:N
        ▼
Summary (阶段性总结)
├── id               String (UUID)
├── subject          String
├── period_type      String (week / month / semester)
├── period_start     Integer
├── period_end       Integer
├── common_reasons   String (共性错误原因)
├── common_suggestions String (共性改进建议)
├── weak_points      JSON   (薄弱知识点 ["知识点1",...])
├── detail           String (详细分析)
├── related_error_ids JSON  (关联错题 ["id1","id2",...])
└── created_at       Integer
        │ 1:N
        ▼
PracticeSet (巩固练习)
├── id               String (UUID)
├── summary_id       String (FK → summaries)
├── subject          String
├── questions        JSON   ([{question, answer, knowledge_points}])
├── pdf_path         String? (PDF 文件路径)
└── created_at       Integer

ClassificationTag (分类标签子表)
├── error_id         String (FK → error_records)
└── tag              String (知识点标签)
```

**数据库特殊设计**：
- **双向量列**：每条错题同时存储 `text_embedding`（科目+知识点+原题+原因+建议拼接）和 `image_embedding`（原图），支持三种搜索模式：纯文本、纯图片、混合加权融合
- **分类标签子表**：`classification` 字段用 JSON 数组存储，同时通过 `error_classification_tags` 子表建立 B-tree 索引，支持高效的按知识点精确查询
- **向量索引**：通过 `libsql_vector_idx` 对双向量列分别建索引，支持余弦相似度检索

### 核心流程

#### 错题分析

```
图片 + 参数(科目/年级/颜色)
        │
        ▼
   读取图片 → base64 编码
        │
        ▼
   构建 Prompt（图片 + 角色 + 指令）→ 调用 LLM Chat API
        │
        ▼
   解析响应（markdown 原题 + JSON 字段，支持代码块包裹和裸 JSON 两种格式）
        │
        ├──→ 保存图片到存储目录
        ├──→ 生成文本 embedding（拼接文本内容）
        ├──→ 生成图片 embedding（原图 base64）
        └──→ 存入 DB（含双向量列 + 分类标签子表）
        │
        ▼
   返回分析结果
```

#### 阶段性总结

```
科目 + 时间范围 + 总结类型
        │
        ▼
   查询时间段内该科目所有错题
        │
        ▼
   构建总结 Prompt（所有原题 + 原因 + 建议）→ 调用 LLM
        │
        ▼
   解析总结结果 → 存入 summaries 表
```

#### 巩固练习 + PDF

```
总结 ID + 题目数量 + 输出路径
        │
        ▼
   读取总结（薄弱知识点 + 原题参考）
        │
        ▼
   构建出题 Prompt → 调用 LLM 生成题目
        │
        ├──→ 存入 practice_sets 表
        └──→ 渲染 PDF（Typst + 中文字体）
```

### 关键设计决策

#### Embedding 策略

采用**双向量**方案：文本 embedding + 图片 embedding 分列存储。

- **文本向量**：拼接 `科目 + 知识点 + 原题 + 原因 + 建议` 生成，覆盖语义搜索场景
- **图片向量**：对原图生成，支持"以图搜图"的相似错题检索
- **混合搜索**：文本和图片向量按可配置权重融合（默认文本 0.7 + 图片 0.3）
- **维度选择**：1536 维（gemini-embedding-2-preview 的 MRL 特性，相比 3072 维几乎无损）

#### LLM 调用重试

指数退避 + 随机抖动，可重试状态码：429, 5xx，网络超时/连接错误。默认 5 次重试，基础延迟 500ms，最大延迟 30s。Chat 和 Embedding 共享同一重试逻辑。

#### LLM 响应解析

LLM 返回 markdown + JSON 混合格式，解析策略：
1. 先提取 markdown 部分（`## 原题` 到 JSON 代码块之间）
2. 再提取 JSON（支持 `\`\`\`json` 代码块包裹和裸 JSON 两种格式）
3. 解析失败时记录原始响应到日志，便于调试

#### Embedding API 兼容性

Chat 和 Embedding 使用不同 API 格式：
- **Chat**：标准 OpenAI `/v1/chat/completions` 格式
- **Embedding**：由 `llm.embedding.provider` 决定；当前已实现 `google`，使用 Google AI Studio 原生 `embedContent` 格式（支持多模态输入）

#### 图片存储

原始图片复制到配置的存储目录，以 `{uuid}.{ext}` 命名，数据库存相对路径。避免原始图片被移动/删除后丢失。

#### CLI 与 MCP 代码复用

业务逻辑全部在 `analysis/`、`summary/`、`practice/` 模块中，CLI 和 MCP 只是调用入口不同，共用同一套服务层。

#### 交叉编译

避免引入 `-sys` 包（C 代码编译），`reqwest` 使用 `rustls-tls` 而非 `native-tls`，支持通过 `cargo zigbuild` 交叉编译到 RISC-V 等目标平台。
