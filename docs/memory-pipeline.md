# 记忆管线（M2–M4）实现规格

> 来源：设计书 `docs/blueprint/` 的 [#05](blueprint/issues/05-context-memory.md) / [#15](blueprint/issues/15-embedding-model.md) / [#27](blueprint/issues/27-storage-single-db.md)，以及决策日志第 2 条。
> 前置：M1（fastembed 本地 EmbeddingBackend，512d）在进行中。
> 已拍板：**摘要由 AI 生成，且回合微摘要搭在同一轮意图里不额外调用**；**DuckDB 用 bundled crate**；**维度取真实值 + 变更时重建**。

## 1. 不变量（先立规矩）

| 规矩 | 说明 |
|---|---|
索引是**派生数据** | 向量库 / FTS5 / 摘要都**不参与权威**；命令日志仍是唯一事实来源 |
索引**失败不阻断**权威路径 | 写索引失败只记 warn；回合照常结算 |
索引**可重建** | 删掉向量库、清空 FTS5、丢摘要，都能从命令日志重放重建 |
重放**不依赖**索引 | `replay()` 不读向量库/摘要，行为与今天一致 |

## 2. M2：派生索引层

### 2.1 端口与实现位置

```rust
// crates/octopus-engine/src/ports.rs
#[async_trait]
pub trait VectorIndex: Send + Sync {
    /// 写入/覆盖一条向量（save_id + seq 唯一）。
    async fn upsert(&self, save_id: &str, seq: i64, round: u32, kind: &str, text: &str, embedding: &[f32]) -> Result<(), EngineError>;
    /// 余弦 top-K；返回 (seq, score)。
    async fn search(&self, save_id: &str, query: &[f32], k: usize) -> Result<Vec<(i64, f32)>, EngineError>;
    /// 维度不符时重建（换模型用）。
    async fn dimension(&self) -> Result<usize, EngineError>;
    async fn rebuild_from(&self, save_id: &str, rows: &[(i64, u32, String, String)]) -> Result<(), EngineError>;
}
```

- 实现放**新 crate `crates/octopus-index`**（DuckDB 依赖不污染 engine）：`DuckDbVectorIndex`。
- FTS5 属于**权威库**（SQLite），写在 `octopus-engine/src/storage.rs` + 迁移里，不走这个端口。

### 2.2 DuckDB 向量库

- 文件：app 数据目录下 `octopus-vectors.duckdb`（与 `octopus.db` 同级）。
- 表：
  ```sql
  CREATE TABLE IF NOT EXISTS event_vectors (
    save_id VARCHAR, seq BIGINT, round INTEGER, kind VARCHAR, text VARCHAR,
    embedding FLOAT[512]        -- 维度由 meta.dim 决定，变更即重建
  );
  CREATE TABLE IF NOT EXISTS meta (k VARCHAR PRIMARY KEY, v VARCHAR);
  ```
- 检索：`array_cosine_similarity(embedding, ?::FLOAT[512])` 排序取 K；数据量小（数千条）暴力扫描即可，**VSS/HNSW 先不做**（设计调研结论同此）。
- **维度校验**：启动时比对 `meta.dim` 与当前模型维度，不一致 → `DROP TABLE event_vectors` 后重建。
- 依赖：`duckdb = { features = ["bundled"] }`（C++ 编译，几分钟；已确认可接受）。

### 2.3 FTS5（权威库内，关键词兜底）

- 迁移新增：`CREATE VIRTUAL TABLE events_fts USING fts5(save_id UNINDEXED, seq UNINDEXED, text, tokenize='unicode61');`
  （中文分词：unicode61 对中文按字切，够用作兜底；要更好可后续换 trigram。）
- 写入：叙事事件（narrate/dialogue/emote）落库时**同事务**写一行（同事务是权威库的，安全）。
- 查询：`WHERE save_id = ? AND events_fts MATCH ?`，BM25 排序取 K。

### 2.4 重建入口

```rust
// engine 暴露：读某存档叙事事件 → 调 VectorIndex::rebuild_from
pub async fn rebuild_memory_index(&self, save_id: &str) -> Result<usize, EngineError>;
```
- 触发时机：启动时维度不符、换 embedding 模型、玩家点「重建索引」（后续可给 UI 入口）。

## 3. M3：记忆管线（#05）

### 3.1 写入侧（best-effort）

```
叙事事件落库（权威） → 写 FTS5（同事务）
                     → 异步 embed → 向量库 upsert（失败只 warn）
```

### 3.2 回合微摘要（不额外调用）

- 新增意图 `summary { text }`：**主线 AI 在同一轮里顺手输出**一句本回合摘要（提示词里加一条要求）。
- 引擎结算：不产生叙事事件、不改状态；写派生表 `round_summaries(save_id, round, text)`（可重建，丢了也能重跑该回合得到，或退化为空）。
- 不放进命令日志的权威事件流（保持「摘要非权威」），但**允许它随回合的其它事件一起被索引**（摘要文本也进 FTS5/向量库，检索时可命中）。
  > 取舍说明：若实现上更简单，把 `summary` 作为一条**事件**记入日志也符合「可重放」，只是与 #06「摘要非权威」的措辞相左；本文按派生表实现。

### 3.3 场景压缩

- `advance_scene` 结算成功时：把该场景期间所有 `round_summaries` 合成一条 `scene_summaries(save_id, scene_id, text)`（用 AI 压缩，走一次便宜的 `pair`/`summary` 角色调用；失败则退化为直接拼接）。
- 场景摘要同样进 FTS5/向量库。

### 3.4 检索注入

```
回合开始：query = 玩家输入（+ 当前场景标题）
  → embed(query)
  → 向量 top-K  ∪  FTS5 top-K   （按 seq 去重，向量优先）
  → 取前 K=5 条 → 组装【相关往事】槽位注入提示词
```

- **新槽位**：`NarrativeView`/`TurnContext` 增加 `memories: Vec<MemoryHit>`，提示词在「场景」之后注入一段【相关往事】。
- **只给主线 AI**：角色 AI 的「所见即所知」（#16）意味着不该把它角色的未知往事喂给它；v1 先只注入主线 AI，角色 AI 仍只拿本回合 `story_narration`。
- 检索失败（无向量库/维度不符/embedding 挂了）→ **静默降级为 FTS5 单路**，再不行就不注入（不得影响回合）。

### 3.5 预算

- #05 要求「上下文 50% / 意图 50%」；现状只有 lore 的字符裁剪。
- 本阶段先做**记忆段独立上限**（如 `min(K*单条上限, token_budget*50%)`），把整块 50/50 重构留给后续（它会牵动 lore/persona/叙事契约的优先级，属独立议题）。

## 4. M4：回合 409

- `submit_round`：spawn 前判断会话是否 busy（`Session::is_busy()`，用现有 `busy` 原子量），busy → `409 { code: "round_in_progress" }`。
- 前端：非 2xx 时 toast 提示「上一回合仍在进行」。
- （幂等 `request_id` 已有，保留。）

## 5. 测试清单

1. FTS5：迁移后写入 + MATCH 查询命中；按 save_id 隔离。
2. DuckDB：upsert → search 往返；维度不符 → 自动重建；空库/缺文件 → 优雅报错。
3. 重建：`rebuild_memory_index` 从命令日志重建出与增量写入一致的条数。
4. 注入：给定假 `VectorIndex`，`TurnContext.memories` 出现在提示词【相关往事】里；检索失败时不注入且回合照常。
5. 摘要：`summary` 意图不产生叙事事件、不改状态；场景压缩在 `advance_scene` 后落表。
6. 409：busy 时提交返回 409 round_in_progress；空闲时正常 202。

## 6. 分期

| 阶段 | 内容 | 依赖 |
|---|---|---|
| M1 | fastembed 本地 EmbeddingBackend | 无（进行中） |
| M2 | 端口 + DuckDB 实现 + FTS5 + 重建 | M1 |
| M3 | 写入侧 + 微摘要 + 场景压缩 + 检索注入 | M2 |
| M4 | 409 round_in_progress | 无（可随时插队） |
