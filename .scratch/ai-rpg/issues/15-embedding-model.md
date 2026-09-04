# Embedding 模型选型

Type: research
Status: resolved
Blocked by: 05

## Question

为 DuckDB 向量检索选型 embedding 模型。需要调研：
① 轻量级中文 embedding 模型候选（text2vec-base-chinese / bge-small-zh / m3e-base 等），对比维度：推理速度、向量维度、中文语义质量、本地部署难度。
② 是否使用统一的 LLM Provider 的 embedding API（如 OpenAI text-embedding-3-small）vs 本地模型 vs 混合策略。
③ 与 Provider 可插拔策略的对齐——如果 Provider 自带 embedding API，是否优先用它生成向量。
④ 向量维度与 DuckDB 存储的兼容性（ARRAY 类型对维度的限制、距离函数选择：cosine / L2 / inner product）。
## Answer

**推荐 bge-small-zh-v1.5（512 维）作为默认 embedding 模型**，bge-m3（1024 维）作为可选升级。

- 本地部署：通过 rig-fastembed（fastembed-rs + ONNX Runtime）加载，一行代码集成，~95MB 首次自动下载。
- 架构：EmbeddingBackend 枚举（Local/Provider），对外固定 512 维，Provider 路径强制降维到 512 以保持 DuckDB schema 一致。
- 理由：bge-small-zh-v1.5 是 sweet spot——4 层 Transformer、极快 CPU 推理、专门中文优化、fastembed-rs 内置、MIT 许可证、4.7M 下载量。
- 不选 m3e-base/text2vec/stella：不在 fastembed-rs 内置列表，集成成本高。
- DuckDB：FLOAT[512] 固定维度，余弦相似度（array_cosine_similarity），VSS HNSW 索引（量级大时启用）。

详见 [research/15](./research/15-embedding-model.md)。
