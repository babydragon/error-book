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
| 生成阶段性总结信息图 | ✅ | ✅ |
| 生成巩固练习 | ✅ | ✅ |
| 输出练习 PDF | ✅ | ✅ |
| 从已存储练习集导出 PDF | ✅ | — |
| 级联删除总结（含关联信息图和练习集） | ✅ | ✅ |
| 数据回填（backfill） | ✅ | — |

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

### 推荐写法（provider+roles 优先）

所有 `provider/base_url/api_key/model` 集中在 `[llm.providers.*]` 中定义，再通过 `[llm.roles]` 绑定到业务角色。Legacy 段 (`llm.chat` / `llm.embedding` / `llm.image`) 仅作为 fallback 或额外字段容器。

```toml
# ── 具名 provider ──────────────────────────────────────────────
[llm.providers.gemini-pro]
provider = "openai"            # 协议: "openai" 或 "google"，默认 "openai"
base_url = "https://generativelanguage.googleapis.com/v1beta/openai"
api_key = "your-gemini-api-key"
model = "gemini-3.1-pro-preview"

[llm.providers.gemini-embedding]
provider = "google"
base_url = "https://generativelanguage.googleapis.com"
api_key = "your-gemini-api-key"
model = "gemini-embedding-2-preview"

[llm.providers.gemini-image]
provider = "google"
base_url = "https://generativelanguage.googleapis.com"
api_key = "your-gemini-api-key"
model = "gemini-3.1-flash-image-preview"

# ── 角色 → provider 绑定 ────────────────────────────────────────
[llm.roles]
vision_recognition = "gemini-pro"
structured_extraction = "gemini-pro"
pedagogical_analysis = "gemini-pro"
summary_synthesis = "gemini-pro"
infographic_planning = "gemini-pro"
practice_generation = "gemini-pro"
image_generation = "gemini-image"
embedding = "gemini-embedding"

# ── embedding 最小配置（仅额外字段）───────────────────────────────
# provider/base_url/api_key/model 从 role 获取，这里只需 dimensions。
[llm.embedding]
dimensions = 1536

# ── image 最小配置（仅额外字段）───────────────────────────────────
# provider/base_url/api_key/model 从 role 获取，这里只需图片相关参数。
# 完全省略时使用默认值 (image/png, 3:4)。
[llm.image]
mime_type = "image/png"
aspect_ratio = "3:4"
```

### Legacy fallback 写法（向后兼容）

如果不需要按角色分配不同模型，仍可使用旧的 `llm.chat` / `llm.embedding` / `llm.image` 作为主配置。此时 `[llm.chat]` 为必填。

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

[llm.image]
provider = "openai"
base_url = "https://api.openai.com/v1"
api_key = "your-image-api-key"
model = "gpt-image-1"
mime_type = "image/png"
aspect_ratio = "3:4"
```

### 配置优先级

| 优先级 | 来源 | 说明 |
|--------|------|------|
| 1 (最高) | `[llm.roles.<role>]` → `[llm.providers.*]` | 角色绑定的具名 provider |
| 2 | Legacy 段 | `llm.chat` / `llm.embedding` / `llm.image` |

- `[llm.chat]`：**fallback-only**，可省略。所有 chat-capable 角色未绑定时才使用。
- `[llm.embedding]`：可仅含 `dimensions`，其余从 role provider 获取。
- `[llm.image]`：可仅含 `mime_type` / `aspect_ratio` / 超时，其余从 role provider 获取。完全省略时使用内置默认值。
- `provider/base_url/api_key/model` 应主要放在 `[llm.providers.*]` 中。

### 统一 HTTP 配置

所有 LLM HTTP 客户端共享 transport 默认值，每个 service 可单独 override。

```toml
# [llm.http.defaults]               # 共享 transport 默认值（以下均为默认值，可省略）
# connect_timeout_secs = 30         # TCP 连接超时（秒）
# http1_only = true                 # 仅使用 HTTP/1.1
# connection_close = true           # 每次请求后关闭连接
# disable_compression = true        # 禁用压缩协商
# tcp_keepalive_secs = 30           # TCP keepalive（秒）
# tcp_nodelay = true                # 禁用 Nagle 算法
# user_agent = "error-book/0.1"     # User-Agent
#
# [llm.http.chat]                   # Chat 服务 override
# request_timeout_secs = 120        # Chat 请求整体超时（秒）
#
# [llm.http.embedding]              # Embedding 服务 override
# request_timeout_secs = 60         # Embedding 请求整体超时（秒）
#
# [llm.http.image]                  # 图片生成服务 override
# request_timeout_secs = 600        # 图片生成请求整体超时（秒），默认 10 分钟
#
# [llm.http.image_download]         # 图片下载服务 override
# request_timeout_secs = 120        # 下载已生成图片的超时（秒）
```

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
generated_image_dir = "./data/generated-images"

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
| `ERROR_BOOK_IMAGE_API_KEY` | 覆盖 `llm.image.api_key` |
| `ERROR_BOOK_IMAGE_BASE_URL` | 覆盖 `llm.image.base_url` |
| `ERROR_BOOK_IMAGE_PROVIDER` | 覆盖 `llm.image.provider` |
| `ERROR_BOOK_IMAGE_MODEL` | 覆盖 `llm.image.model` |
| `ERROR_BOOK_LLM_API_KEY` | 同时覆盖 `llm.chat.api_key`、`llm.embedding.api_key` 和 `llm.image.api_key` |
| `ERROR_BOOK_LLM_BASE_URL` | 同时覆盖 `llm.chat.base_url`、`llm.embedding.base_url` 和 `llm.image.base_url` |
| `ERROR_BOOK_DB_URL` | 覆盖 `database.url` |

说明：
- `llm.chat` 是 **fallback-only**，可省略。所有 chat-capable 角色优先使用 role 绑定的 provider。
- `llm.embedding` 可仅含 `dimensions`，其余字段（provider/base_url/api_key/model）从 role provider 获取。
- `llm.image` 可仅含 `mime_type` / `aspect_ratio` / 超时，其余从 role provider 获取。完全省略时使用默认值。
- `provider/base_url/api_key/model` 应主要放在 `[llm.providers.*]` 中。
- chat: `openai` 与 `google` 都已实现
- embedding: 当前仅 `provider = "google"` 已实现；`openai` 预留但暂未实现
- image: `provider = "openai"` 时可使用 OpenAI Images API（如 `gpt-image-1`）；`provider = "google"` 时支持 Gemini 图片模型和 Imagen 模型
- 当 `provider = "google"` 时，`base_url` 应填写 Google 原生接口根地址，而不是 `/openai/` 兼容地址
- 当 `provider = "openai"` 时，图片生成走独立的 Images API，而不是 chat completions
- 统一 HTTP 配置放在 `[llm.http.*]` 下：`defaults` 设置共享 transport 参数，`chat` / `embedding` / `image` / `image_download` 可分别 override `request_timeout_secs`。默认值：connect=30s，chat=120s，embedding=60s，image=600s，image_download=120s
- `llm.image.connect_timeout_secs / request_timeout_secs / download_timeout_secs` 仅作为 legacy fallback 保留，优先级低于 `llm.http.*`
- `storage.generated_image_dir` 用于保存生成的阶段性总结信息图
- `pdf.font_path` 为必填项；程序启动时会校验字体文件存在且可解析
- 日志默认写入 stderr；可通过 `logging.level` 配置日志级别，并通过 `logging.file` 追加写入日志文件

#### 多角色模型配置（role-based providers）

除 legacy 配置外，支持按业务角色分配不同的模型 provider。**推荐采用此方式**。

配置模型：

1. 在 `[llm.providers.*]` 中定义具名 provider（包含 provider/base_url/api_key/model）
2. 在 `[llm.roles]` 中将角色绑定到 provider 名称
3. Legacy 段 (`llm.chat` / `llm.embedding` / `llm.image`) 仅作为 fallback 或额外字段容器

```toml
# 定义具名 provider
[llm.providers.gemini-pro]
provider = "openai"            # 协议: "openai" 或 "google"，默认 "openai"
base_url = "https://generativelanguage.googleapis.com/v1beta/openai"
api_key = "your-gemini-api-key"
model = "gemini-3.1-pro-preview"

[llm.providers.gemini-embedding]
provider = "google"
base_url = "https://generativelanguage.googleapis.com"
api_key = "your-gemini-api-key"
model = "gemini-embedding-2-preview"

# 将角色绑定到 provider
[llm.roles]
vision_recognition = "gemini-pro"
structured_extraction = "gemini-pro"
pedagogical_analysis = "gemini-pro"
summary_synthesis = "gemini-pro"
infographic_planning = "gemini-pro"
practice_generation = "gemini-pro"
image_generation = "gemini-pro"
embedding = "gemini-embedding"

# embedding 的向量维度从 legacy 段读取（仅需 dimensions）
[llm.embedding]
dimensions = 1536
```

**兼容策略**：
- 新配置 `[llm.providers.*]` + `[llm.roles]` **优先**
- 每个 provider 可通过 `provider` 字段指定协议（`"openai"` 或 `"google"`），默认 `"openai"`
- 如果某角色未绑定或绑定的 provider 不存在，自动 fallback 到 legacy 配置：
  - `embedding` → `[llm.embedding]`
  - `image_generation` → `[llm.image]`
  - 其余角色 → `[llm.chat]`
- `embedding` 混合配置：`provider/base_url/api_key/model` 可来自 role 绑定；`dimensions` 仍来自 `[llm.embedding]`
- `image_generation` 混合配置：`provider/base_url/api_key/model` 可来自 role 绑定；`mime_type` / `aspect_ratio` 仍来自 `[llm.image]`

#### Thinking / Reasoning 控制

部分 OpenAI 兼容模型（如 DeepSeek V4 Pro）支持 `thinking` 和 `reasoning_effort` 参数，用于控制推理深度。

**方式一：扁平字段（推荐）**

```toml
[llm.providers.deepseek-think]
provider = "openai"
base_url = "https://api.deepseek.com/v1"
api_key = "your-deepseek-api-key"
model = "deepseek-v4-pro"
thinking_enabled = true        # true → thinking: { type: "enabled" }
reasoning_effort = "max"       # "high" | "max"（别名: low/medium→high, xhigh→max）

[llm.roles]
pedagogical_analysis = "deepseek-think"
```

**方式二：嵌套配置**

```toml
[llm.providers.deepseek-think]
provider = "openai"
base_url = "https://api.deepseek.com/v1"
api_key = "your-deepseek-api-key"
model = "deepseek-v4-pro"

[llm.providers.deepseek-think.thinking]
thinking_enabled = true
reasoning_effort = "max"

[llm.roles]
pedagogical_analysis = "deepseek-think"
```

**说明**：
- 扁平字段和嵌套配置均可使用，嵌套 `thinking` 优先级更高
- `thinking_enabled`：设为 `true` 时在请求中注入 `thinking: { type: "enabled" }`
- `reasoning_effort`：支持 `high` 和 `max` 两个核心值
- 这些字段仅在 `provider = "openai"` 时注入到请求体中；Google 原生协议请求会自动忽略
- 如果 role 绑定了具名 provider，使用该 provider 的 thinking 配置；否则 fallback 到 `[llm.chat]` 的配置

#### OpenAI 兼容性开关

某些 OpenAI 兼容中继（relay）不完全支持标准协议的所有特性。以下开关用于绕过已知的不兼容问题，仅在 `provider = "openai"` 时生效。

##### `openai_no_system_role`

当中继拒绝 `system` 角色（返回错误或不支持）时，启用此选项会将所有 `system` 消息合并到第一条 `user` 消息的开头，以指令块形式传递。

适用场景：某些 DeepSeek V4 Pro 中继虽然广告 OpenAI 兼容，但实际不处理 `system` role。

##### `structured_json_output`

统一的结构化 JSON 输出开关：
- OpenAI 兼容 provider：注入 `response_format: { "type": "json_object" }`
- Google provider：注入 `responseMimeType: "application/json"`，并在需要时附带 `responseSchema`

适用场景：DeepSeek 结构化输出、Gemini 结构化输出、JSON-only 工作流。

##### 配置示例

**具名 provider（推荐）：**

```toml
[llm.providers.deepseek-relay]
provider = "openai"
base_url = "https://your-relay.example.com/v1"
api_key = "your-relay-api-key"
model = "deepseek-v4-pro"
openai_no_system_role = true
structured_json_output = true

[llm.roles]
structured_extraction = "deepseek-relay"
pedagogical_analysis = "deepseek-relay"
```

**Legacy fallback：**

```toml
[llm.chat]
provider = "openai"
base_url = "https://your-relay.example.com/v1"
api_key = "your-relay-api-key"
model = "deepseek-v4-pro"
openai_no_system_role = true
structured_json_output = true
```

**说明**：
- 两个开关均默认 `false`，不影响现有行为
- `openai_no_system_role` 仅在 `provider = "openai"` 时生效
- `structured_json_output` 同时支持 `openai` 和 `google`
- `openai_no_system_role` 的合并逻辑：收集所有 `system` 消息文本 → 作为 `[System Instructions]` 块前缀到第一条 `user` 消息 → 若无 `user` 消息则自动创建一条
- 可在 `[llm.providers.*]` 或 `[llm.chat]` 中配置；role 绑定的 provider 优先

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

### 基于总结生成记忆信息图

```bash
error-book summary-image --summary-id <ID> [选项]
```

| 选项 | 说明 |
|------|------|
| `--summary-id <ID>` | 总结记录 ID（必填） |
| `-r, --requirements <文本>` | 补充要求，如配色、版式、风格 |

示例：

```bash
# 为指定总结生成帮助孩子记忆的信息图
error-book summary-image --summary-id abc12345-...

# 指定额外风格要求
error-book summary-image --summary-id abc12345-... -r "颜色更活泼，分区更明显，适合贴在书桌前"
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

### 级联删除总结

删除指定的阶段性总结，同时级联删除其关联的总结信息图记录、练习集记录。会尝试删除 `storage.generated_image_dir` 下的生成图片文件。**不会自动删除练习集的 PDF 文件**（因为 PDF 可能输出到用户指定的任意路径）。

```bash
error-book cascade-delete-summary <总结ID>
```

示例：

```bash
# 级联删除总结及其关联记录
error-book cascade-delete-summary abc12345-xxxx-xxxx-xxxx-xxxxxxxxxxxx
```

### 数据回填（Backfill）

用于升级历史记录的 `data_version`，支持两类模式：

- **本地确定性 backfill**：不调用 LLM，适合做规范化/补全旧数据
- **LLM 驱动 backfill**：重新分析历史图片，补齐新的结构化字段

运行 `error-records-analysis` 前需满足：

- 数据库配置正常（`database.url`）
- 原始错题图片仍存在于 `storage.image_dir` 下
- 已配置可用的分析模型：
  - 推荐：通过 `[llm.roles]` 绑定 `vision_recognition` / `structured_extraction` / `pedagogical_analysis`
  - 或者保留可用的 legacy `[llm.chat]` fallback

> **关于 `--scope all`**：当前 `all` 仅运行无需人工介入的确定性 scope（目前为 `error-records`）。`error-records-analysis` 属于 LLM 驱动 scope，必须显式执行；summaries、practice-sets、summary-images 等用户生成产物也暂不纳入自动 backfill。

```bash
error-book backfill [选项]
```

| 选项 | 说明 |
|------|------|
| `-s, --scope <scope>` | 范围：`error-records`（默认）、`error-records-analysis`、`all` |
| `--dry-run` | 仅模拟运行，不实际修改数据 |
| `-l, --limit <n>` | 最多处理的记录数 |
| `--fail-fast` | 遇到错误立即停止 |

示例：

```bash
# 查看将升级的记录（不实际修改）
error-book backfill --dry-run

# 执行 error-records backfill
error-book backfill

# 使用 LLM 重新分析历史图片，只回填结构化字段
error-book backfill --scope error-records-analysis --dry-run

# 处理所有 scope
error-book backfill --scope all

# 限制处理条数并遇到错误即停止
error-book backfill --limit 100 --fail-fast
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
| `generate_summary_image` | 根据已有总结生成记忆信息图 |
| `generate_practice` | 提交巩固练习题任务（支持额外要求） |
| `generate_practice_pdf` | 按已有练习集 ID 导出 PDF |
| `cascade_delete_summary` | 级联删除总结（含关联信息图和练习集记录） |

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
   - `generate_summary_image` -> `get_job_status` / `get_job_result` -> `show_summary`
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
5. 生成阶段性总结信息图
   └─→ error-book summary-image --summary-id <ID>
6. 生成巩固练习 + PDF
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
│  │  │  分析服务  │  │  总结服务  │  │ 信息图服务 │  │ 巩固练习服务 │  │
│  │  │ Analyzer  │  │ Generator │  │ ImageGen  │  │ PracticeGen │  │
│  │  └─────┬─────┘  └─────┬─────┘  └────┬─────┘  └────┬──────┘  │
│  │        └──────────────┼─────────────┼──────────────┘         │
│  │                       ▼                                  │  │
│  │  ┌───────────────────────────────────────────────────┐  │  │
│  │  │               LLM 客户端层                         │  │  │
│  │  │   ┌───────────┐   ┌─────────────┐   ┌───────────┐ │  │  │
│  │  │   │  Chat API │   │ Embedding   │   │ Image API │ │  │  │
│  │  │   │  Client   │   │   Client    │   │  Client   │ │  │  │
│  │  │   └───────────┘   └─────────────┘   └───────────┘ │  │  │
│  │  └───────────────────────────────────────────────────┘  │  │
│  │                       ▼                                  │  │
│  │  ┌───────────────────────────────────────────────────┐  │  │
│  │  │                 数据持久化层                        │  │  │
│  │  │   ┌──────────┐  ┌──────────┐  ┌──────────────┐   │  │  │
│  │  │   │  libsql   │  │ 文件存储 │  │   PDF 输出    │   │  │  │
│  │  │   │ (向量 DB) │  │(原图/信息图)│ │  (Typst)     │   │  │  │
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
├── backfill/
│   ├── mod.rs               # 模块入口
│   ├── engine.rs            # Backfill 引擎（scope 路由、运行记录管理）
│   └── error_records.rs     # error_records 表的 backfill 逻辑
├── summary/
│   ├── generator.rs         # 阶段性总结生成
│   ├── image_generator.rs   # 总结信息图生成
│   └── cascade_delete.rs    # 总结级联删除（共用逻辑）
├── practice/
│   └── generator.rs         # 巩固题目生成
├── db/
│   ├── models.rs            # 数据模型（ErrorRecord / Summary / SummaryImage / PracticeSet / BackfillRun）
│   ├── migration.rs         # 数据库 Schema（内联 SQL + 补丁式 migration）
│   └── repository.rs        # 数据访问层（CRUD + 向量搜索 + backfill run）
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
├── created_at       Integer (Unix timestamp)
├── [结构化预留字段]  (Phase 1: 全部 nullable TEXT)
│   ├── question_markdown_clean   清洗后题目 Markdown
│   ├── question_structure_json   题目结构化 JSON
│   ├── student_answer_text       学生作答文本
│   ├── teacher_marks_json        老师批注 JSON
│   ├── question_type             题目分类
│   ├── difficulty                难度等级
│   ├── error_type                错误大类
│   ├── error_subtype             错误子类
│   ├── root_cause_code           根因编码
│   ├── confidence_json           置信度 JSON
│   ├── pipeline_version          分析管线版本
│   └── model_trace_json          模型调用追踪 JSON
│       │ 1:N
│       ▼
AnalysisArtifact (分析产物)
├── id               String (UUID)
├── error_id         String (FK → error_records, ON DELETE CASCADE)
├── stage            String (管线阶段名称)
├── schema_version   String (产物 schema 版本)
├── model_name       String? (使用的模型)
├── payload_json     String (JSON 产物)
└── created_at       Integer
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
SummaryImage (总结信息图)
├── id               String (UUID)
├── summary_id       String (FK → summaries)
├── prompt           String
├── image_path       String (生成图片路径)
├── mime_type        String
└── created_at       Integer
        │ N:1
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

> **管线中间产物**：当前错题分析虽然仍是单次模型调用，但内部已拆分为 `input_context` / `legacy_combined_raw` / `legacy_combined_parsed` 三类 artifact 落入 `analysis_artifacts` 表，后续可基于这些中间产物继续拆分成视觉识别、结构化抽取、教学分析等多阶段调用。

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

#### 总结信息图生成

```
总结 ID + 可选补充要求
        │
        ▼
   读取 summaries 表中的总结内容
        │
        ▼
   构建中文信息图 Prompt → 调用 Google 图片生成接口
        │
        ├──→ 保存图片到 generated_image_dir
        └──→ 存入 summary_images 表
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

#### Schema Migration vs Data Backfill

本项目区分两种数据库升级方式：

| | Schema Migration | Data Backfill |
|---|---|---|
| **时机** | 程序启动时自动执行 | 需显式执行命令 |
| **内容** | 添加表、列、索引（DDL） | 升级历史数据的 data_version |
| **是否可逆** | 仅加列（不删列），向后兼容 | 幂等（已升级的记录自动跳过） |
| **是否调用 LLM** | 否 | 视 scope 而定 |

**Schema Migration**：程序启动时自动为新旧字段补列（`ALTER TABLE ... ADD COLUMN`），所有新列都有默认值，旧 DB 可以直接启动，无需手动操作。

**Data Backfill**：当需要对历史数据做规范化、补全、升级 data_version 时，需要显式执行 `backfill` 命令。当前支持：

- `error-records` scope（v1→v2）：补全 `pipeline_version`（为空时填 `legacy-backfill-v1`）、回填 `question_markdown_clean`（为空时使用 `original_question`），并将 `data_version` 从 1 升级到 2
- `error-records-analysis` scope：对历史 `error_records` 重新运行多阶段分析管线，**只更新结构化字段**（如 `question_type`、`difficulty`、`error_type`、`student_answer_text` 等）、`pipeline_version`、`model_trace_json` 和 `data_version`，**不覆盖** legacy 核心字段（`original_question`、`classification`、`error_reason`、`suggestions`）

其中：

- `error-records` 是本地确定性 backfill，不调用 LLM
- `error-records-analysis` 会调用 LLM，并要求历史图片文件仍可从 `storage.image_dir` 读取

```bash
# 查看将升级的记录（不实际修改）
error-book backfill --dry-run

# 执行 error-records backfill
error-book backfill --scope error-records

# 先用 dry-run 查看哪些历史记录会被重新分析
error-book backfill --scope error-records-analysis --dry-run

# 执行 LLM 驱动的结构化分析回填
error-book backfill --scope error-records-analysis

# 处理所有 unattended-safe scope（目前等同于 error-records）
error-book backfill --scope all

# 限制处理条数
error-book backfill --limit 100

# 遇到错误立即停止
error-book backfill --fail-fast
```

后续版本的 backfill 逻辑（summaries / practice-sets / summary-images 的数据升级）将逐步添加。

#### Embedding 策略

采用**双向量**方案：文本 embedding + 图片 embedding 分列存储。

- **文本向量**：拼接 `科目 + 知识点 + 原题 + 原因 + 建议` 生成，覆盖语义搜索场景
- **图片向量**：对原图生成，支持"以图搜图"的相似错题检索
- **混合搜索**：文本和图片向量按可配置权重融合（默认文本 0.7 + 图片 0.3）
- **维度选择**：1536 维（gemini-embedding-2-preview 的 MRL 特性，相比 3072 维几乎无损）

#### LLM 调用重试

指数退避 + 随机抖动，可重试状态码：429, 5xx，网络超时/连接错误。默认 5 次重试，基础延迟 500ms，最大延迟 30s。Chat、Embedding、Image 共用同一套重试配置。

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

生成的信息图会保存到 `storage.generated_image_dir`，并在 `summary_images` 表中记录与总结的关联关系。

#### CLI 与 MCP 代码复用

业务逻辑全部在 `analysis/`、`summary/`、`practice/` 模块中，CLI 和 MCP 只是调用入口不同，共用同一套服务层。

#### 交叉编译

避免引入 `-sys` 包（C 代码编译），`reqwest` 使用 `rustls-tls` 而非 `native-tls`，支持通过 `cargo zigbuild` 交叉编译到 RISC-V 等目标平台。

#### 长耗时图片生成 API 踩坑记录

本项目调用 OpenAI 兼容 Images API（如 GPT Image）生成图片时，曾遇到请求超时问题，以下是经验总结：

**不要用默认 `reqwest::Client::new()` 直接请求长耗时接口。** 默认 client 没有显式超时设置，在某些 OpenAI 兼容聚合平台上连接行为兼容性较差，容易卡在连接建立或等待响应阶段。之前遇到的超时并不是简单的"reqwest 内置 155s 超时"，而是默认连接行为（连接池复用、keep-alive、HTTP/2 协商等）在聚合平台代理层表现不稳定，导致请求挂起。

**必须做的配置：**

1. **显式设置三类超时**：`connect_timeout`（TCP 连接）、`timeout`（请求整体超时，图片生成通常需要数分钟）、下载返回图片时的独立 `timeout`
2. **贴近 curl 行为的 client 配置**：`http1_only()`（避免 HTTP/2 协商问题）、`Connection: close`（不依赖 keep-alive）、明确 `User-Agent`、`Accept-Encoding: identity`（避免压缩协商）、设置 `tcp_keepalive`
3. **按场景拆分 client**：图片生成请求（长超时）和图片 URL 下载请求（短超时）使用不同的 client 实例，避免超时配置互相干扰

**图片 URL 下载需要短退避重试。** 生成 API 返回的图片 URL 指向 CDN/对象存储，资源可能短暂未就绪（404 或空响应），需要做 2-3 次短间隔重试（如 1s、2s）。

**关于流式（streaming）的说明：** 如果后续支持 SSE/streaming，流式更适合改善用户长等待体验（逐步返回进度），但并非所有 OpenAI 兼容平台都可靠支持流式，需要按平台能力决定。
