## 目的
本项目是一个基于AI的错题本，用于解析、分析、总结错题。

## 功能
核心业务功能包含：
* 错题解析：
  * 错题图片中题目的识别
  * 错题中错误原因分析
  * 错误的改进建议
* 错题保存：
  * 错题原图保存
  * 题目标签保存
  * 错题原题markdown格式保存
  * 错误原因内容保存
  * 错误改进建议内容保存
  * 原图+原题+题目标签+所有分析内容，embedding后向量保存
* 错题总结：同一学科内容在特定时间段内总结，例如一周，一个月，半个学期等。总结输入来自时间段内的原题、分析内容，汇总出共性的原因和共性的改进方式。并需要存档。
* 未掌握知识点巩固：针对前面的阶段性总结，给出需要巩固知识点的题目，汇总成新的题目，输出成PDF。

## 使用技术栈

* 语言： `rust`
* 大模型：
  * 支持openai格式API, 支持自定义base url和key
  * 支持修改使用的模型
  * 目前调试错题分析使用的是`gemini-3.1-pro-preview`
  * embedding模型使用`gemini-embedding-2-preview`，支持多模态embedding。
* 数据库使用`libsql`，支持向量存储。

## 其他要求

* 最终产物需要同时支持cli和mcp/skill
  * CLI：可以通过命令行和配置文件，针对单图片，进行分析。支持通过指定时间段、科目和题量等，生成测试题。
  * MCP/skill：将基础功能暴露给其他大模型客户端，例如openclaw等，能够支持通过大模型调度来触发分析、总结、出题等功能。

## 交叉编译要求

* 目标平台包含 RISC-V，使用 `cargo zigbuild` 进行交叉编译
* 尽量避免引入 `-sys` 包（包含 C 代码编译），已知的必要 `-sys` 包（如 libsql-sys）通过 zigbuild 处理
* reqwest 使用 `rustls-tls` 而非 `native-tls`（避免 openssl-sys）

## Rust/reqwest 编码注意事项

以下规则适用于所有调用 LLM / 图片生成 / 外部 HTTP 接口的场景：

1. **禁止对长耗时接口直接使用 `reqwest::Client::new()`。** 默认 client 没有显式超时，在 OpenAI 兼容聚合平台场景下连接行为不稳定，容易挂起。必须用 `ClientBuilder` 显式配置。
2. **按场景拆分 client。** 图片生成请求（可能数分钟）和图片 URL 下载（几秒）使用独立 client 实例，分别设置 `connect_timeout`、`timeout`。
3. **必须显式配置的内容：**
   - `connect_timeout`、`timeout`（请求整体超时）
   - `http1_only()`、`Connection: close` header、明确 `User-Agent`、`Accept-Encoding: identity`、`tcp_keepalive`
   - 日志：请求开始/结束/耗时/状态码必须打 log，方便排查
4. **对返回 URL 的资源下载增加短退避重试。** CDN/对象存储可能短暂未就绪（404），做 2-3 次短间隔重试（1s、2s）。
5. **遇到 reqwest 超时先验证 curl。** 在 OpenAI 兼容聚合平台场景下，先确认同一请求用 `curl` 能正常返回，再决定是调整 reqwest 配置还是平台本身的问题。不要假设是 reqwest 内置超时限制。
6. **流式（streaming）非银弹。** 流式可以改善长等待体验，但不是所有 OpenAI 兼容平台都可靠支持，需要按实际平台能力决定是否启用。
