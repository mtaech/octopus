# Embedding 模型选型：DuckDB 向量检索

> 调研问题：为 DuckDB 向量检索选型 embedding 模型，用于事件日志语义搜索（每次 AI 请求前检索 top 5 相关事件）。
>
> 约束：Rust 后端（axum）、本地部署、中文为主、独立开发者、单次会话数千条事件。

---

## 1. 候选模型对比

### 1.1 模型规格一览

| 模型 | 参数量 | 向量维度 | 层数 | 最大 Token | 基础架构 | 模型大小(约) | HF 下载量 |
|---|---|---|---|---|---|---|---|
| **bge-small-zh-v1.5** | ~24M | 512 | 4 | 512 | BERT | ~95 MB | 4.7M |
| **bge-base-zh-v1.5** | ~110M | 768 | 12 | 512 | BERT | ~420 MB | — |
| **bge-large-zh-v1.5** | ~326M | 1024 | 24 | 512 | BERT | ~1.3 GB | — |
| **bge-m3** | ~568M | 1024 | 24 | 8192 | XLM-RoBERTa | ~2.2 GB | 37M |
| **text2vec-base-chinese** | ~110M | 768 | 12 | 512 | BERT (macbert) | ~420 MB | 1.1M |
| **m3e-base** | ~110M | 768 | 12 | 512 | BERT (roberta-wwm-ext) | ~420 MB | 220K |
| **stella-base-zh-v3-1792d** | ~110M | 1792† | 12 | 512 | BERT | ~420 MB | 924 |

† stella 使用 MRL（Matryoshka Representation Learning），可截断到 256/512/768/1024/1792 维，灵活适配。

来源：
- bge-small-zh-v1.5 config.json：`hidden_size=512, num_hidden_layers=4` [HF](https://huggingface.co/BAAI/bge-small-zh-v1.5/resolve/main/config.json)
- bge-m3 config.json：`hidden_size=1024, num_hidden_layers=24, max_position_embeddings=8194` [HF](https://huggingface.co/BAAI/bge-m3/resolve/main/config.json)
- text2vec-base-chinese config.json：`hidden_size=768, num_hidden_layers=12` [HF](https://huggingface.co/shibing624/text2vec-base-chinese/resolve/main/config.json)
- m3e-base config.json：`hidden_size=768, num_hidden_layers=12` [HF](https://huggingface.co/moka-ai/m3e-base/resolve/main/config.json)
- stella-base-zh-v3-1792d config.json：`hidden_size=768, num_hidden_layers=12` [HF](https://huggingface.co/infgrad/stella-base-zh-v3-1792d/resolve/main/config.json)
- bge-base-zh-v1.5 config.json：`hidden_size=768, num_hidden_layers=12` [HF](https://huggingface.co/BAAI/bge-base-zh-v1.5/resolve/main/config.json)
- bge-large-zh-v1.5 config.json：`hidden_size=1024, num_hidden_layers=24` [HF](https://huggingface.co/BAAI/bge-large-zh-v1.5/resolve/main/config.json)
- fastembed-rs 模型维度：BGESmallZHV15=512, BGELargeZHV15=1024, BGEM3=1024 [fastembed-rs](https://github.com/Anush008/fastembed-rs/blob/main/src/models/text_embedding.rs)

### 1.2 中文语义质量对比

| 模型 | s2s Acc | s2p ndcg@10 | 说明 |
|---|---|---|---|
| **m3e-base** | 0.6157 | 0.8004 | 中文检索最优（base 级） |
| openai-ada-002 | 0.5956 | 0.7786 | 闭源 baseline |
| text2vec-base-chinese | 0.5755 | 0.6346 | 检索能力弱于 m3e |
| m3e-small | 0.5834 | 0.7262 | 轻量但检索仍不错 |

> 注：s2s = sentence-to-sentence（同质文本相似度），s2p = sentence-to-passage（异质文本检索，即本项目的核心需求）。数据来自 m3e 模型卡自评。

- BGE 系列在 C-MTEB（中文 MTEB）benchmark 上 v1.5 版本排名靠前，bge-large-zh-v1.5 在发布时（2023.09）是 C-MTEB 榜首。
- bge-m3 是多语言模型（100+ 语言），在 MIRACL（多语言检索）和 MKQA（跨语言 QA）上达到 SOTA，中文也包含在内。
- stella-base-zh-v3-1792d 是较新的蒸馏模型，使用 MRL 训练，在多个 C-MTEB 子任务上表现优秀，且支持维度灵活截断。

来源：
- m3e 模型卡对比表：[m3e-base README](https://huggingface.co/moka-ai/m3e-base/raw/main/README.md)
- BGE 系列 C-MTEB 榜首声明：[FlagEmbedding README](https://raw.githubusercontent.com/FlagOpen/FlagEmbedding/master/README.md)
- bge-m3 技术报告：[arXiv 2402.03216](https://arxiv.org/pdf/2402.03216.pdf)
- Stella 模型卡：[stella-base-zh-v3-1792d README](https://huggingface.co/infgrad/stella-base-zh-v3-1792d/raw/main/README.md)

### 1.3 推理速度对比

| 模型 | 层数 | 维度 | ONNX 支持 | 预估 QPS (CPU) | 备注 |
|---|---|---|---|---|---|
| bge-small-zh-v1.5 | 4 | 512 | ✅ fastembed-rs | 最高 | 参数量最小，4 层 Transformer |
| m3e-base / text2vec / bge-base | 12 | 768 | 需自行导出 | 中等 | 12 层 BERT，约 3000 QPS（text2vec 自报） |
| bge-large-zh-v1.5 | 24 | 1024 | 需自行导出 | 较慢 | 24 层 BERT，模型 1.3GB |
| bge-m3 | 24 | 1024 | ✅ fastembed-rs | 最慢 | 24 层 XLM-RoBERTa，模型 2.2GB，8192 token |
| stella-base-zh-v3-1792d | 12 | 768→1792 | 需自行导出 | 中等 | MRL 输出，可截断加速 |

- **ONNX Runtime 推理**：fastembed-rs 使用 ONNX Runtime，比 PyTorch 快 1.5-3x，且无需 GPU、无需安装 PyTorch。
- text2vec-base-chinese 自报 QPS 约 3008（PyTorch 环境），来源：[text2vec README](https://huggingface.co/shibing624/text2vec-base-chinese/raw/main/README.md)
- bge-small-zh-v1.5 因只有 4 层，即使是 PyTorch 也显著快于 12 层模型；ONNX 下更快。

### 1.4 本地部署难度

| 模型 | fastembed-rs 内置 | 部署方式 | 首次启动 |
|---|---|---|---|
| bge-small-zh-v1.5 | ✅ | `rig-fastembed` 一行代码 | 自动下载 ONNX 模型 (~95MB) |
| bge-m3 | ✅ | `rig-fastembed` 一行代码 | 自动下载 ONNX 模型 (~2.2GB) |
| bge-large-zh-v1.5 | ✅ | `rig-fastembed` 一行代码 | 自动下载 ONNX 模型 (~1.3GB) |
| text2vec-base-chinese | ❌ | 需自行：candle / ort / pytorch | 需手动集成 |
| m3e-base | ❌ | 需自行：candle / ort / pytorch | 需手动集成 |
| stella-base-zh-v3-1792d | ❌ | 需自行：candle / ort / pytorch | 需手动集成 |
| OpenAI API | — | HTTP 调用 | 无本地模型 |

- **rig-fastembed** 是 rig-core 生态的本地 embedding 方案，底层用 fastembed-rs（ONNX Runtime），自动处理模型下载、tokenizer 加载、推理。支持 `rig_fastembed::Client::new().embedding_model(&FastembedModel::BGESmallZHV15)` 一行创建。
- 非内置模型需要用 `candle`（Rust 原生）、`ort`（Rust ONNX Runtime binding）或 `pyo3` 桥接 Python。对独立开发者来说，**优先选择 fastembed-rs 已内置的模型可以大幅降低集成成本**。

来源：
- fastembed-rs 模型列表：[fastembed-rs text_embedding.rs](https://github.com/Anush008/fastembed-rs/blob/main/src/models/text_embedding.rs)
- rig-fastembed 文档：[rig-fastembed lib.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-fastembed/src/lib.rs)

---

## 2. 本地模型 vs API vs 混合策略

### 2.1 OpenAI Embedding API

| 模型 | 默认维度 | 可缩减 | MTEB | 最大 Token | 价格 |
|---|---|---|---|---|---|
| text-embedding-3-small | 1536 | ✅ (可降到 512) | 62.3% | 8192 | $0.02/1M tokens |
| text-embedding-3-large | 3072 | ✅ (可降到 256) | 64.6% | 8192 | $0.13/1M tokens |
| text-embedding-ada-002 | 1536 | ❌ | 61.0% | 8192 | $0.10/1M tokens |

来源：[OpenAI Embeddings 指南](https://platform.openai.com/docs/guides/embeddings)

### 2.2 策略对比

| 维度 | 本地 ONNX 模型 | OpenAI API | 混合策略 |
|---|---|---|---|
| 延迟 | ~5-50ms (CPU) | ~100-500ms (网络) | 按 provider 不同 |
| 成本 | 0（本地推理） | 数千条事件约 $0.001-0.01/次 | 仅 API provider 付费 |
| 隐私 | 完全本地 | 数据出境 | 取决于 provider |
| 离线 | ✅ | ❌ | 取决于 provider |
| 中文质量 | 好（专门中文模型） | 中等（多语言，非中文专门优化） | 取决于 provider |
| 部署复杂度 | 一次集成 | 零部署 | 需维护两套逻辑 |
| 维度灵活 | 固定（除非 MRL 模型） | ✅ 可动态指定 | 取决于 provider |

### 2.3 推荐策略

**推荐：纯本地模型（bge-small-zh-v1.5 作为默认，bge-m3 作为可选升级）。**

理由：
1. **独立开发者、本地运行、不依赖云服务**——这是项目的核心约束。本地模型天然契合。
2. **事件日志量级（数千条）**——用 512 维向量，512 × 4 bytes × 5000 = ~10 MB 存储，完全不是问题。
3. **每次 AI 请求前检索 top 5**——检索频率与 AI 请求频率一致（每秒最多 1-2 次），即使是 12 层模型也完全够用；4 层 bge-small 更是绰绰有余。
4. **中文语义质量**——bge-small-zh-v1.5 是专门的中文模型，对比 OpenAI 的多语言模型（1536 维但中文非专门优化），在中文场景下可能更优。
5. **零网络依赖**——本地推理没有网络延迟、没有 API 限流、没有费用。

**Provider 自带 embedding API 的场景**：如果用户配置了 OpenAI 作为 LLM provider，可以考虑用 text-embedding-3-small 并将维度降到 512 来匹配本地模型。但**不建议**作为默认——因为项目定位是「本地优先」。

---

## 3. 与 Provider 可插拔策略的对齐

### 3.1 rig-core 的 EmbeddingModel trait

已知项目使用 rig-core 的 `CompletionModel` trait 做 LLM provider 抽象。rig-core 同时提供了 `EmbeddingModel` trait：

```rust
// rig-core 的 embedding 抽象
pub trait EmbeddingModel {
    fn embed(&self, request: EmbeddingRequest)
        -> impl Future<Output = Result<EmbeddingResponse, EmbeddingError>>;
    fn embed_batch(&self, requests: Vec<EmbeddingRequest>)
        -> impl Future<Output = Result<Vec<EmbeddingResponse>, EmbeddingError>>;
}
```

来源：[rig-core embeddings/embedding.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/embeddings/embedding.rs)

### 3.2 两条路径

**路径 A：rig-fastembed（本地模型）**
- `rig-fastembed` 把 fastembed-rs 适配到 `EmbeddingModel` trait
- 使用：`fastembed_client.embedding_model(&FastembedModel::BGESmallZHV15)`
- 内置中文模型：BGESmallZHV15 (512d)、BGELargeZHV15 (1024d)、BGEM3 (1024d)

**路径 B：Provider 的 Embedding API（如 OpenAI）**
- rig-core 的 OpenAI provider 也实现了 `EmbeddingModel` trait
- 使用：`openai_client.embedding_model("text-embedding-3-small")`

### 3.3 推荐架构

```rust
/// 统一 embedding 接口，内部可切换本地/远程
pub enum EmbeddingBackend {
    /// 本地 ONNX 模型（默认）
    Local(FastembedModel),
    /// 使用 LLM Provider 的 embedding API
    Provider(/* rig-core EmbeddingModelHandle */),
}

impl EmbeddingBackend {
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        match self {
            Self::Local(model) => model.embed(text).await,
            Self::Provider(handle) => handle.embed(text).await,
        }
    }

    pub fn dimension(&self) -> usize {
        match self {
            Self::Local(FastembedModel::BGESmallZHV15) => 512,
            Self::Local(FastembedModel::BGEM3) => 1024,
            Self::Provider(_) => 512, // 强制 provider 降维到 512
        }
    }
}
```

**策略规则**：
1. **默认**：使用本地 bge-small-zh-v1.5（512 维），通过 rig-fastembed 加载
2. **如果 Provider 自带 embedding API 且用户明确选择**：优先使用 Provider 的 API（需要网络），但强制维度降到 512 以保持 DuckDB schema 一致
3. **维度统一**：无论后端是什么，对外暴露的向量维度固定为 512（或由配置决定），避免 DuckDB ARRAY 类型需要 DDL 变更

**注意**：DuckDB 的 ARRAY 类型在创建表时需要指定维度，如 `embedding FLOAT[512]`。如果切换维度不同的模型，需要 ALTER TABLE 或重建表。因此**建议在配置中固定维度**，所有后端统一输出该维度。

---

## 4. DuckDB 存储兼容性

### 4.1 ARRAY 类型与维度

DuckDB 原生支持固定长度 ARRAY 类型：

```sql
CREATE TABLE event_embeddings (
    event_id BIGINT,
    embedding FLOAT[512],  -- 512 维向量
    created_at TIMESTAMP
);
```

- **无显式维度上限**：文档声明 "The arrays can have any size as long as the size is the same for both arguments"（在距离函数中）。
- **FLOAT[512] 存储**：512 × 4 bytes = 2 KB/行。5000 条事件 = 10 MB。

来源：[DuckDB Array Functions](https://duckdb.org/docs/current/sql/functions/array.html)

### 4.2 距离函数

DuckDB 内置以下向量距离函数：

| 函数 | 用途 |
|---|---|
| `array_cosine_distance(a, b)` | 余弦距离（0 = 完全相同，2 = 完全相反） |
| `array_cosine_similarity(a, b)` | 余弦相似度（1 = 完全相同，-1 = 完全相反） |
| `array_distance(a, b)` | 欧几里得距离（L2） |
| `array_inner_product(a, b)` | 内积/点积 |
| `array_dot_product(a, b)` | 点积（同 inner_product） |
| `array_negative_inner_product(a, b)` | 负内积 |

来源：[DuckDB Array Functions](https://duckdb.org/docs/current/sql/functions/array.html)

### 4.3 VSS 扩展（HNSW 索引）

DuckDB 提供 VSS（Vector Similarity Search）扩展，支持 HNSW 索引加速：

```sql
INSTALL vss;
LOAD vss;

CREATE TABLE items (
    id BIGINT,
    embedding FLOAT[512]
);

-- 创建 HNSW 索引
CREATE INDEX idx_embedding ON items USING HNSW (embedding);

-- 检索 top 5
SELECT id, array_cosine_distance(embedding, ?::FLOAT[512]) AS dist
FROM items
ORDER BY dist
LIMIT 5;
```

**关键特性**：
- 默认距离度量：`l2sq`（L2 平方），可通过 `metric` 选项改为 `cosine` 或 `inner_product`
- 支持 `min_by` 语法做一次性最近邻搜索
- 可创建多个索引（不同列、不同距离度量）
- **持久化**：默认仅内存，需设置 `SET hnsw_enable_experimental_persistence = true` 才能持久化到磁盘（实验性功能）

**本项目的适用性**：
- 事件日志量级（数千条）：**暴力扫描已经足够快**（数千次 512 维余弦距离计算在 DuckDB 内是毫秒级），HNSW 索引是锦上添花而非必需。
- 如果量级增长到 10 万+，再启用 VSS 扩展。

来源：[DuckDB VSS Extension](https://duckdb.org/docs/current/core_extensions/vss.html)

### 4.4 距离函数选择建议

**推荐：余弦相似度（`array_cosine_similarity`）**

理由：
1. **BGE 模型用余弦相似度训练**：BGE 系列模型使用对比学习训练，向量空间是余弦空间，官方推荐用余弦相似度检索。模型卡中明确写了 query instruction：`为这个句子生成表示以用于检索相关文章：`
2. **对向量范数不敏感**：余弦相似度只关心方向，不受嵌入向量绝对大小影响——这在混合不同来源的文本（对话、旁白、系统消息）时尤其重要。
3. **DuckDB 原生支持**：`array_cosine_similarity` 和 `array_cosine_distance` 都可直接使用，且 VSS 索引支持 `metric: 'cosine'`。

**如果使用 OpenAI 模型**：OpenAI 官方推荐余弦相似度，且返回的向量已归一化（所以余弦相似度=内积）。

来源：[OpenAI Embeddings 指南](https://platform.openai.com/docs/guides/embeddings#limitations-risks)

---

## 5. 推荐方案

### 最终推荐：bge-small-zh-v1.5（512 维）作为默认，bge-m3（1024 维）作为可选升级

| 考量 | bge-small-zh-v1.5 | bge-m3（可选） |
|---|---|---|
| 维度 | 512 | 1024 |
| 模型大小 | ~95 MB (ONNX) | ~2.2 GB (ONNX) |
| 推理速度 | 极快（4 层） | 较慢（24 层） |
| 中文质量 | 优秀（专门中文模型） | 优秀（多语言 SOTA） |
| 长文本 | 512 token | 8192 token |
| 部署 | fastembed-rs 内置 | fastembed-rs 内置 |
| 存储/行 | 2 KB | 4 KB |

### 推荐理由

1. **bge-small-zh-v1.5 是 sweet spot**：
   - 512 维在质量与存储间取得最优平衡
   - 4 层 Transformer，CPU 推理极快（< 5ms/条）
   - fastembed-rs 内置，一行代码加载
   - 下载量 4.7M，社区验证充分
   - MIT 许可证

2. **bge-m3 作为升级路径**：
   - 如果用户需要更长的文本上下文（8192 token vs 512）
   - 或者需要多语言混合检索
   - 同样在 fastembed-rs 中内置，切换只需改一行枚举值

3. **不建议 m3e-base / text2vec-base-chinese**：
   - 不在 fastembed-rs 内置列表中，需要额外集成工作
   - 参数规模与 bge-base-zh-v1.5 相同（110M / 768d），但 bge-base 也在 fastembed-rs 中
   - 对独立开发者来说，fastembed-rs 的免集成优势远大于 benchmark 上的微小差距

4. **不建议 stella-base-zh-v3-1792d**：
   - MRL 灵活维度是很好的特性，但不在 fastembed-rs 中
   - 下载量仅 924，社区验证不足
   - 可以作为后续自定义集成时的候选

### 架构总结

```
┌─────────────────────────────────────────────┐
│                 应用层                        │
│  EventEmbedder::embed(text) -> Vec<f32>     │
└──────────────────┬──────────────────────────┘
                   │
         ┌─────────▼─────────┐
         │  EmbeddingBackend │
         │  (统一 trait)     │
         └────────┬──────────┘
                  │
     ┌────────────┼────────────┐
     │                         │
┌────▼──────┐          ┌───────▼──────┐
│  Local    │          │  Provider    │
│  (默认)   │          │  (可选)      │
│           │          │              │
│ bge-small │          │ OpenAI       │
│ 512d ONNX │          │ 512d API     │
│ rig-      │          │ rig-core     │
│ fastembed │          │ embedding    │
└────┬──────┘          └───────┬──────┘
     │                         │
     └────────┬────────────────┘
              │
     ┌────────▼────────┐
     │    DuckDB        │
     │  FLOAT[512]      │
     │  + HNSW index    │
     │  (VSS extension) │
     └─────────────────┘
```

---

## 6. 来源索引

### 模型规格
- bge-small-zh-v1.5 config.json：[HF](https://huggingface.co/BAAI/bge-small-zh-v1.5/resolve/main/config.json) — `hidden_size=512, num_hidden_layers=4`
- bge-base-zh-v1.5 config.json：[HF](https://huggingface.co/BAAI/bge-base-zh-v1.5/resolve/main/config.json) — `hidden_size=768, num_hidden_layers=12`
- bge-large-zh-v1.5 config.json：[HF](https://huggingface.co/BAAI/bge-large-zh-v1.5/resolve/main/config.json) — `hidden_size=1024, num_hidden_layers=24`
- bge-m3 config.json：[HF](https://huggingface.co/BAAI/bge-m3/resolve/main/config.json) — `hidden_size=1024, num_hidden_layers=24, max_position_embeddings=8194`
- text2vec-base-chinese config.json：[HF](https://huggingface.co/shibing624/text2vec-base-chinese/resolve/main/config.json) — `hidden_size=768, num_hidden_layers=12`
- m3e-base config.json：[HF](https://huggingface.co/moka-ai/m3e-base/resolve/main/config.json) — `hidden_size=768, num_hidden_layers=12`
- stella-base-zh-v3-1792d config.json：[HF](https://huggingface.co/infgrad/stella-base-zh-v3-1792d/resolve/main/config.json) — `hidden_size=768, num_hidden_layers=12`

### Benchmark 数据
- m3e 模型对比表：[m3e-base README](https://huggingface.co/moka-ai/m3e-base/raw/main/README.md) — s2s/s2p 评测数据
- BGE 系列 C-MTEB 榜首：[FlagEmbedding README](https://raw.githubusercontent.com/FlagOpen/FlagEmbedding/master/README.md)
- bge-m3 技术报告：[arXiv 2402.03216](https://arxiv.org/pdf/2402.03216.pdf)
- text2vec 自评 QPS 约 3008：[text2vec README](https://huggingface.co/shibing624/text2vec-base-chinese/raw/main/README.md)
- Stella 模型卡与 C-MTEB 分数：[stella-base-zh-v3-1792d README](https://huggingface.co/infgrad/stella-base-zh-v3-1792d/raw/main/README.md)

### 部署工具
- fastembed-rs 模型列表与维度：[fastembed-rs text_embedding.rs](https://github.com/Anush008/fastembed-rs/blob/main/src/models/text_embedding.rs)
- rig-fastembed 文档：[rig-fastembed lib.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-fastembed/src/lib.rs)
- rig-core EmbeddingModel trait：[rig-core embeddings/embedding.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/embeddings/embedding.rs)
- rig-core EmbeddingModelHandle：[rig-core embeddings/handle.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/embeddings/handle.rs)

### DuckDB
- DuckDB Array Functions：[DuckDB Docs](https://duckdb.org/docs/current/sql/functions/array.html) — `array_cosine_distance`, `array_cosine_similarity`, `array_distance`, `array_inner_product`
- DuckDB VSS Extension：[DuckDB Docs](https://duckdb.org/docs/current/core_extensions/vss.html) — HNSW 索引，支持 cosine/l2sq/inner_product

### OpenAI
- OpenAI Embeddings 指南：[OpenAI Docs](https://platform.openai.com/docs/guides/embeddings) — 模型规格、维度、定价、推荐余弦相似度
