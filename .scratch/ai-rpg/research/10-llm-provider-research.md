# 可插拔 LLM Provider 生态调研

> 调研对象：Rust 生态里可插拔 LLM Provider 的方案（async-openai、Anthropic SDK、rig 等）；多 Provider 统一抽象（trait）的现成做法；function calling / structured output 在各家（OpenAI / Anthropic / DeepSeek 等）的支持差异与统一建模方式；流式输出处理。
>
> 本调研只采信一手来源：官方文档、仓库源码、crates.io / docs.rs 的 crate 数据。每条结论在文末标注出处 URL，正文用行内链接指向同一来源。
>
> 采集方式说明：本环境无法访问 Anthropic 官方文档站（docs.anthropic.com / docs.claude.com 均返回 “App unavailable in region”），Anthropic 侧的 API 事实改由 Anthropic 一方 SDK 源码（anthropic-sdk-typescript 的 messages.ts，即官方 SDK 的权威类型定义）采信，并在相应条目中注明。

## 摘要（TL;DR）

1. **没有官方的 Anthropic / DeepSeek Rust SDK。** Anthropic 只维护 Python / TypeScript SDK，Rust 侧是一堆早期社区 crate（如 `anthropic` 0.0.8、`anthropic-sdk-rust` 0.1.1）。DeepSeek 同样无官方 Rust SDK，但它同时兼容 OpenAI 与 Anthropic 两种线上协议，因此生态里普遍用 OpenAI 协议客户端或通用框架来对接它。
2. **多 Provider 统一抽象有三条成熟路线**：async-openai 的 `Config` trait（只抽象「传输/鉴权/端点」，不抽象协议，OpenAI 协议专用）；rig 的 `CompletionModel` trait（最完整：把各家 wire 协议归一成统一的请求/响应/工具/流类型）；genai 的「统一 API + 按 Provider 原生协议适配」路线。langchain-rust 则是 LangChain 的 Rust 移植，用 `LLM`/`Chain` trait 组合。
3. **function calling 三家的建模本质相同、细节不同**：都是「工具名 + 描述 + JSON Schema 入参」，但 OpenAI/DeepSeek 用 `tools[]` + `tool_choice` + 返回 `tool_calls`（arguments 是 JSON 字符串），Anthropic 用 `tools[]` + `tool_choice`（auto/any/tool/none）+ 返回 `tool_use` 内容块（input 是结构化 JSON）。统一抽象（rig 的做法）是把二者归一成 `ToolDefinition { name, description, parameters }` 与 `ToolCall { id, name, arguments }`。
4. **structured output 是分歧最大的点。** OpenAI 是 `response_format: {type: json_schema, ...}` + `strict: true`（受限 schema 子集）；Anthropic 是 `output_config.format: {type: json_schema, schema}`（也是 json_schema）；DeepSeek 的 Chat Completions 只有 `json_object` 模式（无 schema 校验），但它的 tool calls 有 strict 模式（Beta），且 Responses API 支持 `text.format`。统一抽象层（rig）用 `output_schema: schemars::Schema` 承载，并在各 provider 侧做 schema 清洗与降级。
5. **流式都是 SSE，但事件语义不同。** OpenAI Chat Completions 与 DeepSeek Chat 是「data-only SSE + `data: [DONE]` 结束」的增量 chunk（`choices[].delta`）；OpenAI Responses 与 DeepSeek Responses 是「语义事件流」（`response.output_text.delta` / `response.function_call_arguments.delta` / `response.completed`）；Anthropic 是「命名事件流」（`message_start` / `content_block_delta` / `message_delta` / `message_stop`）。工具调用的参数在流中是「分片 JSON 增量」（OpenAI/Anthropic/DeepSeek 都是 partial_json / arguments 增量），需要客户端自行拼接。

## 1. Rust 生态方案现状

### 1.1 async-openai（OpenAI 协议客户端）

- 定位：非官方、基于 OpenAI OpenAPI spec 的异步 Rust 库，crates.io 版本 0.41.3，约 780 万下载。[crates.io](https://crates.io/crates/async-openai) · [仓库 README](https://github.com/64bit/async-openai)
- 自带：指数退避重试、builder 模式的请求对象、SSE 流式、细粒度 feature flags、WASM、tower 中间件。[README](https://github.com/64bit/async-openai/blob/main/async-openai/README.md)
- **协议边界**：只实现 OpenAI 协议。对「OpenAI 兼容」的第三方（Azure、DeepSeek、Groq、Ollama、Together 等）通过改 `base_url`/header/query 来接入，官方 README 明确写了「OpenAI compatible providers」：`byot`（bring-your-own-types，用 `serde_json::Value` 替换请求/响应类型）、按请求或全局覆盖 path/query/header、`Client<Box<dyn Config>>` 动态分发。[README](https://github.com/64bit/async-openai/blob/main/async-openai/README.md)
- **它对 Anthropic 无能为力**：Anthropic Messages API 是另一套协议（content blocks、tool_use、output_config），async-openai 不覆盖。

### 1.2 rig / rig-core（多 Provider 框架）

- 定位：「an opinionated library for building LLM powered applications」，crates.io 版本 0.42.0，约 251 万下载。[crates.io](https://crates.io/crates/rig-core) · [仓库](https://github.com/0xPlaygrounds/rig)
- 内置 26+ 个 provider：Anthropic、Azure OpenAI、ChatGPT/Copilot、Cohere、DeepSeek、Gemini、Groq、Hugging Face、Hyperbolic、llama.cpp、MiniMax、Mira、Mistral、Moonshot、Ollama、OpenAI、OpenRouter、Perplexity、Together、Venice、Voyage AI、xAI、Xiaomi MiMo、Z.ai 等。[providers/mod.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/providers/mod.rs)
- **对 OpenAI 兼容协议有通用适配器**：providers/mod.rs 的「provider 实现清单」写明——OpenAI-chat 兼容的 API 由 `GenericCompletionModel` 驱动，DeepSeek / Groq / Mistral / Ollama / xAI 等复用同一适配器，各自只提供端点与鉴权。[providers/mod.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/providers/mod.rs)
- DeepSeek 在 rig 中按 OpenAI 兼容协议处理，`base_url = https://api.deepseek.com`。[providers/deepseek.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/providers/deepseek.rs)

### 1.3 genai（rust-genai，多 Provider 原生协议适配）

- 定位：「A Native-Protocol Multi-AI Provider Library for Rust」，crates.io 版本 0.7.0-beta.21，约 34.5 万下载。[crates.io](https://crates.io/crates/genai) · [仓库](https://github.com/jeremychone/rust-genai)
- 一套 Rust API 覆盖：OpenAI、OpenAI Responses、Anthropic、Gemini、Ollama、Groq、xAI、DeepSeek、Cohere、Together、Fireworks、Nebius、Mimo、Zai、BigModel、Aliyun、Google Vertex、GitHub Copilot 等。[README](https://github.com/jeremychone/rust-genai)
- 设计哲学：**provider 有原生协议就用原生协议，只有 OpenAI 兼容层的就用兼容层**；「在 adapter 层管理协议差异，比维护多套 SDK 更简单、更可扩展」。[README Library Focus](https://github.com/jeremychone/rust-genai#library-focus)
- 只聚焦 text chat / vision / function calling，不追求完整客户端 API（README 明说完整 API 请用 async-openai）。[README](https://github.com/jeremychone/rust-genai#library-focus)

### 1.4 langchain-rust

- 定位：LangChain 的 Rust 移植，crates.io 版本 4.6.0，约 15.3 万下载。[crates.io](https://crates.io/crates/langchain-rust) · [仓库](https://github.com/Abraxas-365/langchain-rust)
- 抽象：`Chain` trait + `LLM` trait（`language_models::llm::LLM`），`LLMChainBuilder` 组装；provider 有 OpenAI、Anthropic Claude 等；工具侧有「Open AI Compatible Tools Agent」。[README](https://github.com/Abraxas-365/langchain-rust)

### 1.5 各家官方 Rust SDK 情况

- **Anthropic：无官方 Rust SDK。** crates.io 上的 `anthropic`（0.0.8，约 3 万下载）、`threatflux-anthropic-sdk`（0.3.0）、`anthropic-sdk-rust`（0.1.1）、`adk-anthropic`（ADK-Rust 的专属客户端，2.2.0）均为社区 crate，成熟度参差。[crates.io 搜索 anthropic](https://crates.io/search?q=anthropic)
- **DeepSeek：无官方 Rust SDK。** 社区有 `deepseek-sdk`（0.4.0）、`deepseek-api`（0.1.1）、`just-deepseek`（0.2.0）等，下载量都很低；因为 DeepSeek 官方即声明兼容 OpenAI/Anthropic 协议，生态默认用 OpenAI 协议客户端或通用框架对接。[crates.io 搜索 deepseek](https://crates.io/search?q=deepseek) · [DeepSeek 首页](https://api-docs.deepseek.com/)

## 2. 多 Provider 统一抽象（trait）的现成做法

### 2.1 rig 的 CompletionModel trait（最完整的归一化抽象）

rig-core 的核心边界是 `CompletionModel`，把各 provider 的 wire 协议归一成统一的请求/响应/流类型（provider 自己的 wire 类型留在 provider 一侧）。trait 签名如下（来源：completion/request.rs）：

```text
pub trait CompletionModel: WasmCompatSend + WasmCompatSync {
    fn completion(&self, request: CompletionRequest)
        -> impl Future<Output = Result<CompletionResponse, CompletionError>> + WasmCompatSend;
    fn stream(&self, request: CompletionRequest)
        -> impl Future<Output = Result<StreamingCompletionResponse, CompletionError>> + WasmCompatSend;
    fn completion_request(&self, prompt: impl Into<Message>) -> CompletionRequestBuilder<Self>;
    fn capabilities(&self) -> ProviderCapabilities;
}
```

[completion/request.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/completion/request.rs)

要点：
- 用 RPITIT（return-position impl trait）返回 future，`Arc<M>` 有 blanket impl；另有 `ModelHandle` 做类型擦除（`dyn ErasedModel`），供 agent / registry 在运行时替换模型而不改变 Rust 类型。[completion/handle.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/completion/handle.rs)
- `capabilities()` 声明 provider 行为差异，供运行时在准备请求时决策。当前唯一能力位是 `composes_native_output_with_tools`——「原生 structured output 是否能与工具调用在同一个多轮请求里共存」，OpenAI / Anthropic 置 true，其余保守 false。[completion/request.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/completion/request.rs)
- `CompletionRequest` 是规范请求表示，provider 模块把它翻译成各自请求体，再把响应转回 `CompletionResponse`。字段：`model`、`chat_history`、`documents`、`tools: Vec<ToolDefinition>`、`temperature`、`max_tokens`、`tool_choice`、`additional_params`（provider 专属透传）、`output_schema: Option<schemars::Schema>`、`record_telemetry_content`。[completion/request.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/completion/request.rs)

### 2.2 async-openai 的 Config trait（传输抽象，非协议抽象）

async-openai 的可插拔点在**传输层**而非协议层：

```text
pub trait Config: Send + Sync {
    fn headers(&self) -> HeaderMap;
    fn url(&self, path: &str) -> String;
    fn query(&self) -> Vec<(&str, &str)>;
    fn api_base(&self) -> &str;
    fn api_key(&self) -> &SecretString;
}
```

[config.rs](https://github.com/64bit/async-openai/blob/main/async-openai/src/config.rs)

- 内置实现只有 `OpenAIConfig`、`AzureConfig`；`Box<dyn Config>` / `Arc<dyn Config>` 已有 blanket impl，支持动态分发。README 的「Dynamic Dispatch」示例就是 `Client<Box<dyn Config>>`。[config.rs](https://github.com/64bit/async-openai/blob/main/async-openai/src/config.rs) · [README](https://github.com/64bit/async-openai/blob/main/async-openai/README.md)
- 结论：它抽象的是「同一个 OpenAI 协议打到不同端点」，**不是**「不同协议」的统一。换 provider 若仍是 OpenAI 兼容（改 base_url 即可）没问题；换到 Anthropic 协议则无解。

### 2.3 genai 的适配器路线

- 统一入口 `Client` + `ChatRequest` / `ChatMessage` / `ChatResponse`，通过 `AdapterKind` 按 model 名解析到具体 provider 适配器（名字前缀映射，如 gpt→OpenAI、claude→Anthropic，可用 `AdapterKindResolver` 自定义）。[README](https://github.com/jeremychone/rust-genai)
- 每个 provider 一个 adapter，把统一 `ChatRequest` 翻译成该 provider 的原生协议（或 OpenAI 兼容协议）；`ChatOptions` 的参数（temperature / max_tokens / top_p）在 README 里有逐 provider 的映射表（OpenAI 兼容 / Anthropic / Gemini / Cohere 各列）。[README ChatOptions](https://github.com/jeremychone/rust-genai#chatoptions)
- 与 rig 的差别：genai 更轻（chat/vision/function calling 为限），暴露的是「统一高层 API」，归一化程度和结构化输出/工具的建模不如 rig 深。

### 2.4 对比小结

| 方案 | 抽象层次 | 协议覆盖 | 工具/结构化输出建模 | 流式归一化 | 适合 |
|---|---|---|---|---|---|
| async-openai | 传输（Config trait） | 仅 OpenAI 协议 | OpenAI 原生类型（FunctionObject/ResponseFormat） | OpenAI chunk 类型 | 只对接 OpenAI/兼容协议，要完整 API |
| rig-core | 语义（CompletionModel trait） | 26+ provider，含 Anthropic 原生 | 统一 ToolDefinition/ToolCall + output_schema | 统一 StreamedAssistantContent | 要多 Provider 且要工具/结构化输出归一 |
| genai | 高层 API + adapter | OpenAI/Anthropic/Gemini 等原生 | 有 tool/structured（较轻） | 统一 ChatStream | 要轻量多 Provider 聊天 |
| langchain-rust | Chain/LLM trait | OpenAI/Anthropic 等 | tools agent | 有 | 要 LangChain 生态/链式编排 |

## 3. function calling / structured output 支持差异与统一建模

### 3.1 OpenAI

**function calling**：请求 `tools[]` 里每个工具是 `{type: "function", function: {name, description, parameters(JSON Schema), strict}}`；`tool_choice` 取值 auto / required / none / 指定函数 / allowed_tools；`parallel_tool_calls: false` 禁止并行。模型返回 `tool_calls[{id, function:{name, arguments(JSON 字符串)}}]`，arguments 可能不是合法 JSON，官方要求客户端自行校验。[function calling 指南](https://platform.openai.com/docs/guides/function-calling)

**structured output**：有两种载体——(a) function calling 的 `strict: true` 工具；(b) `response_format: {type: "json_schema", json_schema: {name, strict: true, schema}}`（Chat Completions）或 Responses 的 `text: {format: {type: "json_schema", strict, schema}}`。旧式 `{type: "json_object"}` 是 JSON mode（只保证合法 JSON，不保证 schema 一致）。[structured outputs 指南](https://platform.openai.com/docs/guides/structured-outputs)

strict 模式下支持的 JSON Schema 是**受限子集**：类型限 string/number/boolean/integer/object/array/enum/anyOf；string 属性支持 pattern、format（date-time/date/duration/email/hostname/ipv4/ipv6/uuid）；number 支持 multipleOf/max/min；array 支持 minItems/maxItems；**根对象必须是 object（不能顶层 anyOf）**；**所有字段必须 required**；**additionalProperties 必须 false**；可选字段用「union with null」模拟。[structured outputs 指南 · Supported schemas](https://platform.openai.com/docs/guides/structured-outputs#supported-schemas)

### 3.2 Anthropic

（Anthropic 官方文档站被区域拦截，以下采信一方 SDK 源码 anthropic-sdk-typescript 的 messages.ts，即 Anthropic 官方 TS SDK 的权威类型定义。）

**tool use**：请求 `tools[]` 里每个工具是 `{name, description, input_schema(JSON Schema, draft 2020-12)}`；`tool_choice` 取值 auto / any / tool / none，均可带 `disable_parallel_tool_use`。模型返回 content block `{type: "tool_use", id, name, input(结构化 JSON)}`；工具结果作为 user 消息里的 `{type: "tool_result", tool_use_id, content}` 回传。[messages.ts](https://github.com/anthropics/anthropic-sdk-typescript/blob/main/src/resources/messages/messages.ts) · 官方指南 https://docs.claude.com/en/docs/build-with-claude/tool-use（区域不可访问）

**structured output**：顶层参数 `output_config: {format: {type: "json_schema", schema}, effort}`（JSONOutputFormat）。SDK 的 `messages.parse()` / `messages.stream()` 会用 `zodOutputFormat(...)` 自动把响应解析到 `parsed_output` 字段。[messages.ts](https://github.com/anthropics/anthropic-sdk-typescript/blob/main/src/resources/messages/messages.ts) · 指南 https://platform.claude.com/docs/en/build-with-claude/structured-outputs（SDK 文档内链接）

**与 OpenAI 的关键差异**：工具入参 schema 用 `input_schema`（而非 OpenAI 的 `function.parameters`）；工具调用结果是结构化 `input`（而非 OpenAI 的 arguments 字符串）；tool_choice 枚举集不同（any/tool 而非 required）；structured output 走独立的 `output_config`（而非 response_format）。

### 3.3 DeepSeek

DeepSeek 同时提供三套线上协议（同一批模型）：OpenAI 兼容 Chat Completions、OpenAI 兼容 Responses、Anthropic 兼容 Messages。[DeepSeek 首页](https://api-docs.deepseek.com/)

**Chat Completions（OpenAI 协议）**：
- 工具与 OpenAI 同构：`tools: [{type: "function", function: {name, description, parameters}}]`，最多 128 个，仅支持 function；`tool_choice` 取 none/auto/required/指定函数；返回 `tool_calls[{id, function:{name, arguments}}]`，工具结果用 `role: "tool"` + `tool_call_id` 回传。[tool_calls 指南](https://api-docs.deepseek.com/guides/tool_calls) · [create-chat-completion 参考](https://api-docs.deepseek.com/api/create-chat-completion)
- **strict 模式（Beta）**：`base_url` 换成 `https://api.deepseek.com/beta`，所有 function 设 `strict: true`，服务端校验 schema。支持的 JSON Schema 类型：object/string/number/integer/boolean/array/enum/anyOf，外加 `$ref`/`$def`；要求所有属性 required + additionalProperties:false。[tool_calls 指南](https://api-docs.deepseek.com/guides/tool_calls)
- **structured output 有限**：Chat Completions 的 `response_format` 只有 `[text, json_object]` 两值，即只有 JSON mode（`{type: "json_object"}`，需在 prompt 里带 json 字样 + 示例），**没有 OpenAI 式的 json_schema 校验**。[create-chat-completion 参考](https://api-docs.deepseek.com/api/create-chat-completion) · [json_mode 指南](https://api-docs.deepseek.com/guides/json_mode)

**Responses API（OpenAI 协议）**：`tools` 支持 function / web_search；`tool_choice` 取 none/auto/required/指定工具；`text.format`（即结构化输出）**完全支持**；`parallel_tool_calls` 恒为 true（参数被忽略）。[responses_api 指南](https://api-docs.deepseek.com/guides/responses_api)

**Anthropic API**：`base_url = https://api.deepseek.com/anthropic`，支持 `tools`（name/input_schema/description）、`tool_choice`（none/auto/any/tool，disable_parallel_tool_use 被忽略）、`system`、`temperature`、`thinking`、`stop_sequences`、`stream`；工具返回 `tool_use`/`tool_result` 内容块；模型名映射：claude-opus→deepseek-v4-pro、claude-haiku/sonnet→deepseek-v4-flash。[anthropic_api 指南](https://api-docs.deepseek.com/guides/anthropic_api)

### 3.4 统一建模方式（rig 的实际做法，可直接参考）

rig 把三家归一为：
- 工具定义：`ToolDefinition { name, description, parameters: serde_json::Value }`（parameters 就是 JSON Schema，对应 OpenAI 的 function.parameters、Anthropic 的 input_schema、DeepSeek 的 function.parameters）。[completion/request.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/completion/request.rs)
- 工具调用：`ToolCall { id, provider, function: ToolFunction, signature, additional_params }`，其中 `id` 是 rig 自己 mint 的相关句柄、`provider` 是 provider 原发 id（支持 OpenAI Responses 的 item_id+call_id 双 id）。[completion/message.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/completion/message.rs)
- 工具选择：`ToolChoice { Auto, None, Required, Specific { function_names } }`——把 OpenAI 的 required、Anthropic 的 any 都归一成 `Required`。[completion/message.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/completion/message.rs)
- 结构化输出：`CompletionRequest.output_schema: Option<schemars::Schema>`。OpenAI 适配器里用 `sanitize_schema` 把任意 schema 清洗成 OpenAI strict 子集（强制 additionalProperties:false、补全 required），并从 schema 的 title 取结构化输出名。[providers/openai/mod.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/providers/openai/mod.rs)
- 归一化 finish_reason：`FinishReason` 统一各家的 finish_reason / stop_reason / stopReason，未知值保留为 `Other`。[completion/request.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/completion/request.rs)

## 4. 流式输出处理

### 4.1 各家流式协议差异

| Provider / 端点 | 传输 | 事件形态 | 文本 | 工具参数 | 结束信号 |
|---|---|---|---|---|---|
| OpenAI Chat Completions | data-only SSE | 增量 chunk | `choices[].delta.content` | `choices[].delta.tool_calls[].function.arguments`（分片） | `data: [DONE]` |
| OpenAI Responses | 语义 SSE | 类型化事件 | `response.output_text.delta` | `response.function_call_arguments.delta` | `response.completed`（无 [DONE]） |
| Anthropic Messages | 命名事件 SSE | 类型化事件 | `content_block_delta`(text_delta) | `content_block_delta`(input_json_delta, partial_json) | `message_stop`，usage/stop_reason 在 `message_delta` |
| DeepSeek Chat | data-only SSE | 增量 chunk | `choices[].delta.content` | `choices[].delta.tool_calls[].function.arguments`（分片） | `data: [DONE]` |
| DeepSeek Responses | 语义 SSE | 类型化事件 | `response.output_text.delta` | `response.function_call_arguments.delta` | `response.completed` / `incomplete` / `failed`（无 [DONE]） |

来源：[OpenAI streaming 指南](https://platform.openai.com/docs/guides/streaming-responses) · [DeepSeek responses_api](https://api-docs.deepseek.com/guides/responses_api) · [DeepSeek create-chat-completion](https://api-docs.deepseek.com/api/create-chat-completion) · [Anthropic messages.ts](https://github.com/anthropics/anthropic-sdk-typescript/blob/main/src/resources/messages/messages.ts) · [Anthropic streaming 参考](https://docs.anthropic.com/en/api/streaming)

要点：
- OpenAI Chat 与 DeepSeek Chat 同构（都是 OpenAI 兼容 SSE + [DONE]）；OpenAI Responses 与 DeepSeek Responses 同构（都是语义事件 + response.completed/incomplete/failed，且带单调递增 sequence_number）。[responses_api](https://api-docs.deepseek.com/guides/responses_api)
- **工具参数在流里都是分片 JSON 增量**（OpenAI 的 arguments 分片、Anthropic 的 input_json_delta.partial_json、DeepSeek 的 arguments 分片），客户端必须自行按 index/id 拼接。OpenAI Chat 与 DeepSeek Chat 用 `stream_options: {include_usage: true}` 才能在最后一个 chunk 拿到 usage。[create-chat-completion](https://api-docs.deepseek.com/api/create-chat-completion)
- OpenAI 官方推荐用 Responses API 流式（「designed with streaming in mind」，类型安全事件）。[streaming 指南](https://platform.openai.com/docs/guides/streaming-responses)

### 4.2 Rust 侧的流式抽象

- **async-openai**：`StreamResponse<T> = Pin<Box<dyn futures::Stream<Item = Result<T, OpenAIError>> + Send>>`，Chat 流式类型是 `CreateChatCompletionStreamResponse`（含 `choices[].delta.tool_calls` 分片与可选 usage）。[types/stream.rs](https://github.com/64bit/async-openai/blob/main/async-openai/src/types/stream.rs) · [types/chat/chat_.rs](https://github.com/64bit/async-openai/blob/main/async-openai/src/types/chat/chat_.rs)
- **rig**：`StreamingCompletionResponse` 实现 `Stream<Item = Result<StreamedAssistantContent, CompletionError>>`，把各家的流事件归一成 `StreamedAssistantContent::{Text, ToolCall, ToolCallDelta{content: Name|Delta}, Reasoning, ReasoningDelta, Final(StreamFinal), Unknown}`。它还内置：工具调用的分片组装（`ToolInputEnd` 收尾）、`PauseControl` 暂停/恢复、以及明确的结束语义——区分「传输错误 / 可恢复的坏帧 / 截断（EOF 无终止记录）」，并要求消费者排空到 None 而非见到第一个 Err 就停。[streaming/mod.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/streaming/mod.rs)
- **genai**：`exec_chat_stream` 返回统一 ChatStream，附 `print_chat_stream` 打印工具。[README](https://github.com/jeremychone/rust-genai)

## 5. 对本项目的启示

1. **选型**：本项目的「AI→引擎 = 结构化工具调用 + 可插拔 Provider + 流式演出」需求，rig-core 是贴合度最高的现成抽象——它把工具、结构化输出、流式都归一了，且内置 DeepSeek/OpenAI/Anthropic。若坚持轻量、只要 chat+tool，genai 是可替代项；async-openai 适合「只绑定 OpenAI 协议家族」时拿来当单一后端，但需要自己在它之上包一层 trait 才能换协议。
2. **不要假设每家都有 json_schema 结构化输出**：DeepSeek 的 Chat Completions 只有 json_object 模式（无 schema 校验），要拿到严格结构化输出只能走「strict 工具调用（Beta）」或「Responses API 的 text.format」。统一抽象层应把「结构化输出」实现成可降级能力（rig 用 `ProviderCapabilities` + provider 侧 schema 清洗来应对，可借鉴）。
3. **把工具调用建模为「arguments 是 JSON 字符串」还是「结构化 input」是分水岭**：OpenAI/DeepSeek(Chat) 给的是 JSON 字符串（可能非法），Anthropic 给的是结构化 input。统一抽象应像 rig 那样在 provider 边界做反序列化归一，并保留 provider 原发 id 用于回传 tool_result。
4. **流式要按协议分叉**：OpenAI/DeepSeek Chat 的 [DONE] 结束、Responses 的语义事件、Anthropic 的命名事件，三套都要各自解析；工具参数分片需按 id 拼接。若用 rig，这些差异已被 `StreamingCompletionResponse` 抹平。
5. **Anthropic 侧靠一方 SDK 源码采信即可，但接入时要实测**：本环境无法访问 Anthropic 官方文档站，且 Anthropic 无官方 Rust SDK——接入 Anthropic 协议时，要么复用 rig/genai 的 adapter，要么对着 anthropic-sdk-typescript 的 messages.ts 自己实现，风险与维护成本都不低。

## 附录：主要来源 URL

框架 / crate：
- https://github.com/64bit/async-openai （含 async-openai/README.md、src/config.rs、src/types/chat/chat_.rs、src/types/shared/*.rs、src/types/stream.rs）
- https://docs.rs/async-openai
- https://github.com/0xPlaygrounds/rig （含 crates/rig-core/src/completion/*.rs、tool/mod.rs、providers/mod.rs、providers/deepseek.rs、providers/openai/mod.rs、streaming/mod.rs）
- https://docs.rs/rig-core
- https://github.com/jeremychone/rust-genai
- https://github.com/Abraxas-365/langchain-rust
- https://crates.io/crates/async-openai · /rig-core · /genai · /langchain-rust · https://crates.io/search?q=anthropic · https://crates.io/search?q=deepseek

官方文档 / 一方 SDK：
- OpenAI function calling：https://platform.openai.com/docs/guides/function-calling
- OpenAI structured outputs：https://platform.openai.com/docs/guides/structured-outputs
- OpenAI streaming：https://platform.openai.com/docs/guides/streaming-responses
- Anthropic 一方 TS SDK 类型：https://github.com/anthropics/anthropic-sdk-typescript/blob/main/src/resources/messages/messages.ts
- Anthropic 指南（区域不可访问，作参考）：https://docs.claude.com/en/docs/build-with-claude/tool-use · https://platform.claude.com/docs/en/build-with-claude/structured-outputs · https://docs.anthropic.com/en/api/streaming
- DeepSeek：https://api-docs.deepseek.com/ · /guides/tool_calls · /guides/json_mode · /guides/anthropic_api · /guides/responses_api · /api/create-chat-completion
