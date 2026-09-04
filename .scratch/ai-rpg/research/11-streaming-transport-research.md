# 前后端流式通信选型调研（SSE vs WebSocket）

Type: research
Status: done

## Question

调研 axum 后端与 Vue3 前端之间的流式通信选型：SSE vs WebSocket 的适用场景；axum 的 SSE/WebSocket 支持；Vue3 侧的消费方式（fetch ReadableStream / EventSource / native WebSocket）；断线重连与背压；与 LLM 流式输出的对接。

> 采信范围：仅官方文档、源码、规范等一手来源（docs.rs、MDN、WHATWG HTML 规范、OpenAI openapi 仓库、Anthropic SDK 仓库）。每条结论均标注出处 URL。

---

## 1. SSE vs WebSocket：适用场景

- **SSE 是单向的（server → client），WebSocket 是双向的。** MDN 原文："Unlike WebSockets, server-sent events are unidirectional; that is, data messages are delivered in one direction, from the server to the client"，并据此判定 SSE 适用于"无需以消息形式向服务端回传数据"的场景（如状态更新、新闻流、写入 IndexedDB/web storage）。
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/EventSource>

- **SSE 走普通 HTTP，WebSocket 走 HTTP Upgrade。** EventSource 通过一个持久 HTTP 连接接收 `text/event-stream` 格式事件；连接保持打开直到调用 `EventSource.close()`。
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/EventSource>

- **SSE 在 HTTP/1.1 下受浏览器每域名连接数限制（6 条），HTTP/2 下按多路复用流数（默认 100）。** MDN 明确警告：非 HTTP/2 时 SSE 受最大打开连接数限制，每浏览器+域名 6 条，Chrome/Firefox 标记为 "Won't fix"；HTTP/2 下同时 HTTP 流数由服务端与客户端协商（默认 100）。多标签页场景要格外注意。
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events/Using_server-sent_events>、<https://developer.mozilla.org/en-US/docs/Web/API/EventSource>

- **WebSocket 提供"创建和管理与服务器的连接，并在连接上收发数据"的 API。** 其 `send()` 入队数据、`message` 事件接收数据、`binaryType` 控制二进制类型，事件为 `open`/`message`/`error`/`close`。
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/WebSocket>

- **选型结论（事实综合，非来源原话）**：服务端单向推送（如 LLM token 流、演出剧本流）优先 SSE——无状态、可被代理/CDN 缓存/压缩、自动重连、协议更简单；需要客户端频繁主动发消息（如交互式操作协议、双向状态同步）才需要 WebSocket。

---

## 2. axum 的 SSE / WebSocket 支持现状

> 以下事实均取自 docs.rs 上 axum **0.8.9**（docs.rs "latest"）。

### 2.1 SSE：`axum::response::sse`

- 提供 `Event`、`Sse`、`KeepAlive`、`EventDataWriter` 等类型，模块描述为 "Server-Sent Events (SSE) responses"。SSE 模块**无需额外 feature**（模块页无 "crate feature" 标注）。
  出处：<https://docs.rs/axum/latest/axum/response/sse/index.html>

- **`Sse<S>` 包装一个事件流**，构造要求 `S: TryStream<Ok = Event> + Send + 'static`（`S::Error: Into<BoxError>`），实现 `IntoResponse`；`Sse::new(stream)` 创建，`keep_alive(KeepAlive)` 配置保活（**默认无保活**）。
  出处：<https://docs.rs/axum/latest/axum/response/sse/struct.Sse.html>

- **`Event` 字段方法**：`data()`（换行会自动拆成多个 `data:` 字段；**空 data 事件会被浏览器忽略**）、`event()`（对应 `EventSource.addEventListener` 的 type 参数；不设置则浏览器触发 `message`）、`id()`（对应 `MessageEvent.lastEventId`，用于重连续传）、`retry(Duration)`（重连等待提示，客户端可自行加长如指数退避）、`comment()`、`json_data()`（**feature `json`**）。`Event` 实现 `Display`/默认空事件。
  出处：<https://docs.rs/axum/latest/axum/response/sse/struct.Event.html>

- **`KeepAlive`**：`interval()` 默认 **15 秒**；`text()`/`event()` 定制保活内容（默认空注释）。
  出处：<https://docs.rs/axum/latest/axum/response/sse/struct.KeepAlive.html>

- 示例：`Sse::new(stream).keep_alive(KeepAlive::default())`，其中 `stream` 为 `stream::repeat_with(|| Event::default().data("hi!")).map(Ok)`。
  出处：<https://docs.rs/axum/latest/axum/response/sse/index.html>

### 2.2 WebSocket：`axum::extract::ws`

- 模块描述 "Handle WebSocket connections"，**需 crate feature `ws`**（模块页标注 "Available on crate feature ws only"）。底层依赖 tokio-tungstenite `^0.29.0`。
  出处：<https://docs.rs/axum/latest/axum/extract/ws/index.html>

- **`WebSocketUpgrade` 提取器**：对 HTTP/1.1 要求方法为 **GET**（后续版本用 CONNECT，故建议配合 `routing::any`）；`ws.on_upgrade(|socket| async { ... })` 返回 `Response` 完成升级。支持 `on_failed_upgrade` 回调、`protocols()` 子协议协商（按优先级递减，回显 `Sec-WebSocket-Protocol`）。
  出处：<https://docs.rs/axum/latest/axum/extract/ws/struct.WebSocketUpgrade.html>

- **`WebSocket`** 是消息流：`recv().await` / `send(msg).await`；需并发读写时用 `StreamExt::split` 拆成 sender/receiver。
  出处：<https://docs.rs/axum/latest/axum/extract/ws/index.html>

- **`Message` 枚举**：`Text(Utf8Bytes)`、`Binary(Bytes)`、`Ping(Bytes)`、`Pong(Bytes)`、`Close(Option<CloseFrame>)`。**Ping 由服务端自动应答**；收到 Ping 时 **Pong 自动回给客户端**；`Close` 描述优雅关闭协议（收到关闭帧后 axum 自动回关闭帧）。
  出处：<https://docs.rs/axum/latest/axum/extract/ws/enum.Message.html>

- **缓冲区与背压相关配置（`WebSocketUpgrade`）**：`read_buffer_size`（默认 128 KiB）、`write_buffer_size`（默认 128 KiB）、`max_write_buffer_size`（默认**无上限**；文档明确"Setting this can provide backpressure in the case the write buffer is filling up due to write errors"）、`max_message_size`（默认 64 MB）、`max_frame_size`（默认 16 MB）、`accept_unmasked_frames`（默认 false）。
  出处：<https://docs.rs/axum/latest/axum/extract/ws/struct.WebSocketUpgrade.html>

---

## 3. Vue3 侧消费流式数据的方式

### 3.1 fetch + ReadableStream（通用、可解码任意流）

- **`Response.body` 即 `ReadableStream`**："The Request.body and Response.body properties are available, which are getters exposing the body contents as a readable stream."
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/Streams_API/Using_readable_streams>

- **读取范式**：`response.body.getReader()` 锁定流（同时只能有一个 reader），`reader.read()` 返回 `{ done, value }` 的 Promise（有数据 → `{value, done:false}`；流关闭 → `{value:undefined, done:true}`；出错 → reject），循环 `read()` 直到 `done`。配 `TextDecoder` 解码 UTF-8 字节。
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/Streams_API/Using_readable_streams>

- 同时支持**异步迭代**消费（`for await (const chunk of response.body)`，见该页 "Consuming a fetch() using asynchronous iteration" 章节）。
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/Streams_API/Using_readable_streams>

- 关键区分：**"push 源"（如 TCP/WebSocket）持续推送，需主动 start/pause/cancel；"pull 源"（如 fetch 文件）需显式请求数据**。fetch 的 body 属于 ready-made readable stream。
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/Streams_API/Concepts>

### 3.2 EventSource（原生 SSE，自带重连）

- `new EventSource(url)` 打开持久连接；跨域需 `{ withCredentials: true }`。监听 `message`（无 `event` 字段）或 `addEventListener("<event名>", ...)`（有 `event` 字段）。
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events/Using_server-sent_events>

- `readyState`：`CONNECTING(0)`/`OPEN(1)`/`CLOSED(2)`；`close()` 关闭连接；事件 `open`/`message`/`error`。
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/EventSource>

### 3.3 native WebSocket

- `new WebSocket(url)`、`send()`、`message`/`open`/`error`/`close` 事件、`binaryType`、`bufferedAmount`（已排队未发送字节数）。
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/WebSocket>

---

## 4. 断线重连与背压

### 4.1 断线重连

- **EventSource 自动重连（WHATWG HTML 规范）**："reestablish the connection" 时置 `readyState` 为 `CONNECTING`、触发 `error` 事件、**等待等于 reconnection time 的延迟**（若上次失败，UA 可引入**指数退避**；无网络时可等 OS 报告网络恢复）、然后**携带 `Last-Event-ID` 头**重新 fetch。而 "fail the connection" 则置 `CLOSED`、触发 `error`、**不再重连**。
  出处：<https://html.spec.whatwg.org/multipage/server-sent-events.html>

- **reconnection time 初值由实现定义（通常几秒）**，可被事件流中的 `retry` 字段改写；`Last-Event-ID` 头上报 last event ID 字符串（UTF-8，不含 NULL/LF/CR），用于断点续传。
  出处：<https://html.spec.whatwg.org/multipage/server-sent-events.html>

- **axum 侧对应**：`Event::id()` 设置事件 ID（映射 `lastEventId`），`Event::retry(Duration)` 设置重连提示（文档注明仅是提示，客户端可加长）。
  出处：<https://docs.rs/axum/latest/axum/response/sse/struct.Event.html>

- **native WebSocket 无内建自动重连**：其事件模型只有 `open`/`message`/`error`/`close`，断线后由应用自行监听 `close`/`error` 并重建连接（规范/文档未提供类似 EventSource 的自动重连机制）。
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/WebSocket>

### 4.2 背压

- **WebSocket 无背压（MDN 明示）**："The WebSocket API has no way to apply backpressure, therefore when messages arrive faster than the application can process them, the application will either fill up the device's memory by buffering those messages, become unresponsive due to 100% CPU usage, or both." 并指向可自动背压的 `WebSocketStream`。
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/WebSocket>

- **发送侧可用 `bufferedAmount` 做手动背压信号**（已入队待发送字节数）。
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/WebSocket>

- **Streams API（fetch）原生背压**：背压=流/管道链调节读写速度；`ReadableStreamDefaultController.desiredSize` 过低时源应停止发送，消费者恢复后由 `pull` 继续供数；内部队列按 queuing strategy（highWaterMark）管理。
  出处：<https://developer.mozilla.org/en-US/docs/Web/API/Streams_API/Concepts>

- **axum WebSocket 侧背压**：`max_write_buffer_size` 限制写缓冲上限（默认无上限），"in the case the write buffer is filling up due to write errors" 时提供背压；`write_buffer_size`（128 KiB）为落盘前目标缓冲大小。
  出处：<https://docs.rs/axum/latest/axum/extract/ws/struct.WebSocketUpgrade.html>

---

## 5. 与 LLM 流式输出的对接

- **OpenAI 用 SSE（`text/event-stream`）做流式**：Chat Completions 文档描述 "Returns a chat completion object, or a streamed sequence of chat completion chunk objects if the request is streamed"，其 200 响应 content 同时定义 `application/json` 与 `text/event-stream`（→ `CreateChatCompletionStreamResponse`）。Responses API 的 `stream` 参数文档明写 "streamed ... using server-sent events"。
  出处：<https://github.com/openai/openai-openapi>（`openapi.yaml`，main 分支，行 2143/2157/18624）

- **OpenAI 流以 `data: [DONE]` 终止**：Assistants streaming 示例以 `event: done` / `data: [DONE]` 结尾；chunk 为 `{"object":"chat.completion.chunk",...,"choices":[{"delta":{"content":...},...}]}` 形态。
  出处：<https://github.com/openai/openai-openapi>（`openapi.yaml`，行 2530/19342）

- **Anthropic 用 SSE + 具名事件**：其官方 Python SDK 把流事件建模为 `RawMessageStreamEvent` 联合类型（`message_start`/`message_delta`/`message_stop`/`content_block_start`/`content_block_delta`/`content_block_stop`，以 `type` 判别）；`_streaming.py` 通过 SSE 解码器（`ServerSentEvent`）按 `sse.event == "message_start"` 等分发。
  出处：<https://github.com/anthropics/anthropic-sdk-python>（`src/anthropic/types/raw_message_stream_event.py`、`src/anthropic/_streaming.py`，main 分支）

- **对接结论（事实综合，非来源原话）**：主流 LLM Provider 的流式输出均为 SSE。axum 后端代理 LLM 流时最自然的形态是 **SSE 端到端**（后端把 Provider 的 SSE chunk 转成 `axum::response::sse::Event` 直接转发给浏览器）；若前端还需要双向交互（如中途打断/注入指令），再引入 WebSocket，并显式处理其缺失的自动重连与背压。

---

## 6. 关键结论汇总

1. **默认选 SSE**：服务端→前端的单向流（LLM token、演出文本）用 SSE；axum 原生支持（`axum::response::sse`，无需 feature），Vue3 用原生 `EventSource` 即得自动重连 + `Last-Event-ID` 续传。
2. **双向交互才用 WebSocket**：需要客户端主动发消息（打断/指令/状态同步）时用 `axum::extract::ws`（feature `ws`），但必须自行实现重连与背压（native WebSocket 无背压、无自动重连）。
3. **axum 均已内置，能力完整**：SSE 提供 `Event`（data/event/id/retry/json_data）、`KeepAlive`（默认 15s）；WebSocket 提供 buffer/背压参数（`max_write_buffer_size` 等）与子协议协商。
4. **LLM 流 = SSE**：OpenAI/Anthropic 流式输出都是 SSE，axum 侧应做 SSE 转发（或按需桥接 WebSocket）。
5. **连接数陷阱**：SSE 在 HTTP/1.1 下每域名 6 连接上限，多标签页/多流场景应启用 HTTP/2（默认 100 流）。
