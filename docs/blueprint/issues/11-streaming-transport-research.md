# 前后端流式通信选型调研

Type: research
Status: resolved

## Question

调研 axum 后端与 Vue3 前端之间的流式通信选型：SSE vs WebSocket 的适用场景；axum 的 SSE/WebSocket 支持；Vue3 侧的消费方式（fetch stream / EventSource / native WebSocket）；断线重连与背压；与 LLM 流式输出的对接。为工程形态与「游玩界面设计」的流式演出提供事实依据。

## Answer

见 [research/11-streaming-transport-research.md](../research/11-streaming-transport-research.md)。

结论：默认 SSE——服务端→前端单向流（LLM token、演出文本）用 axum::response::sse + Vue3 原生 EventSource（自动重连 + Last-Event-ID）；双向交互（打断/指令/状态同步）才用 WebSocket（axum::extract::ws，需自行实现重连与背压）；LLM 流式输出（OpenAI/Anthropic）本质是 SSE，端到端 SSE 转发最自然；注意 HTTP/1.1 每域名 6 连接上限，多流需启用 HTTP/2。
