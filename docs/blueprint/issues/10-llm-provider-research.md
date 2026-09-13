# 可插拔 LLM Provider 生态调研

Type: research
Status: resolved

## Question

调研 Rust 生态里可插拔 LLM Provider 的方案：async-openai、anthropic SDK、rig 等框架；多 Provider 统一抽象（trait）的现成做法；function calling / structured output 在各家的支持差异与统一建模方式；流式输出的处理。为 Provider 抽象与「AI→引擎动作协议」提供事实依据。

## Answer

见 [research/10-llm-provider-research.md](../research/10-llm-provider-research.md)。

结论：无官方 Anthropic/DeepSeek Rust SDK，多 Provider 统一抽象三条路线中 rig-core 的 CompletionModel trait 最贴合（归一工具/结构化输出/流式）；三家 function calling 本质同构（name+description+JSON Schema），分歧在参数载体与 tool_choice 枚举；structured output 分歧最大（OpenAI strict json_schema 受限子集 / Anthropic json_schema / DeepSeek 仅 json_object）；DeepSeek 双协议兼容（OpenAI + Anthropic）；流式全 SSE 但语义不同，rig 已抹平。
