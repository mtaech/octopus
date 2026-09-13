# 通宵执行报告（M1–M4 + 差距补齐）

> 目标见会话 goal：对照 `docs/blueprint/` 设计书（map.md + issues 01–27 + decision-log）补齐未实现项。
> 本文件在每阶段收口后追加，**早晨按此验收**。
> 差距分析的原始结论在 `docs/gap-analysis.md`（锚点 `4b5b3c4`），文首加了「通宵后更新」注记。

## 一句话结论

今晚完成 **13 项**（全部经我独立复验，不是 subagent 自述）：**M1** fastembed 本地嵌入、**B** 存档级模型、**M4** 409、**M2** DuckDB 向量库 + FTS5 + WAL、**M3a** 索引 + 混合检索 + 中文修复、**M3b** 微摘要 + 场景压缩、**#14** 升级链路 + 新原点、**#06** RNG 持久化 + 快照、**#04a** 判定器/intent_id/interact+query_world/actor 约束、**#04b** 回合内 3 轮续跑、**#04c** 角色 AI 并行 + scope 过滤、**#01** 声明区校验 + 命名统一、**嵌入配置修复**（含两道保险）。

**最终质量门**：`cargo test --workspace` → engine **181** / api **30** / types **92** / ai **20**（1 ignored）/ octopus-index **3** / migration **1**，零失败；`pnpm -C frontend build` 通过。

**我没有提交任何东西**（subagent 也没有）——工作区仍是未提交状态，等你验收后决定怎么分组提交。

**你需要做的一件事**：**重启后端**。今晚新增了 5 个迁移（`000013` 模型分工 / `000014` FTS5 / `000015` 摘要表 / `000016` legacy / `000017` 快照），不重启不会生效。

### 明确没做（按剩余缺口大小排）

| 缺口 | 说明 |
|---|---|
| **#04 剩余** | 3 轮 tool-call continue 循环、角色 AI 按角色并行、按 AI 的查询 scope 过滤、`opponent_id`/`target_value`、Lua `QueryWorld` 回灌、`interact` 执行物件挂载技能 |
| **#19 演出模板** | 三模板 + 切换 + `--tpl-*` token 换肤**整体未做**（设计里已降级到 v1.1，只做 CSS 换肤；现在连换肤都没有） |
| **#13 / #26** | `advance_scene` 语义细节；Provider 重试/降级/成本护栏 |
| **#27 导出契约** | DuckDB/FTS5/WAL 已做；导出包契约、导入上限未收口 |
| **#06 Player 实体** | 多人预留，未做 |
| **#14 的 8 条 caveat** | 见 §#14 末尾（`format_version`、golden fixtures、冻结角色规则未合并、受控 PC 离场不重指、dry-run 不深 diff、备份无列举 API 等） |

### 环境提醒

`/home` 已用 **97%**（约 9.5G 可用），`/home/huang/cargo-target` 占 **150G**。期间 `libduckdb-sys` 曾因满盘在 `ar` 阶段失败，subagent 删了重复的构建缓存目录（约 10GB，未动仓库文件）后恢复。**这一晚没有做过 `cargo clean` 或任何清理之外的删除。**

## 验收方式（最省事的顺序）

```bash
cd /home/huang/Personal/Dev/Code/octopus
cargo test --workspace          # 应全绿
pnpm -C frontend build          # 应全绿（含 vue-tsc）
git log --oneline -10           # 看这晚产生了哪些提交（若已提交）
```

另外：**运行中的后端需要重启一次**才会带上本晚的引擎改动（含迁移）。

## 阶段状态

| 阶段 | 内容 | 状态 | 证据 |
|---|---|---|---|
| 差距分析 | 逐票对照设计书与代码 → `docs/gap-analysis.md` | ✅ 完成（423 行） | 见下 §差距分析 |
| M1 | fastembed 本地 EmbeddingBackend（512d） | ✅ 完成并复验 | 见下 §M1 |
| B | 存档级模型（新建时选共用/分角色） | ⏳ 待 M1 | 规格 `docs/save-role-models.md` |
| M2 | DuckDB 向量库 + SQLite FTS5 + WAL + 重建 | ✅ 完成并复验 | 见下 §M2 |
| M3a | 记忆管线：中文 FTS 修复 + 写作侧索引 + 检索注入 | ✅ 完成并复验 | 见下 §M3a |
| M3b | 记忆管线：微摘要 + 场景压缩 | ✅ 完成并复验 | 见下 §M3b |
| #14 | 存档升级链路 + 新原点（缺口 #1） | ✅ 完成并复验 | 见下 §#14 |
| #06 | RNG 持久化 + 快照 | ✅ 完成并复验 | 见下 §#06 |
| #04a | 协议补全（判定器/intent_id/interact+query_world/actor 约束） | ✅ 完成并复验 | 见下 §#04 |
| #04b | 回合内 3 轮 tool-call 续跑 | ✅ 完成并复验 | 见下 §#04b |
| #04c | 角色 AI 按角色并行 + scope 过滤 | ✅ 完成并复验 | 见下 §#04c |
| 嵌入配置 | 修 config + 两道保险（回落本地 / 补回供应商） | ✅ 完成并复验 | 见下 §嵌入 |
| M4 | 409 round_in_progress | ✅ 完成并复验 | 见下 §M4 |
| 其余 | 差距分析暴露的其他缺口 | ⏳ 待分析 | — |

## 嵌入配置的完整修复（含两道保险）

**触发**：真机日志 `Embedding 后端构建失败（embedding role 指向的模型「deepseek-flash」…不是 embedding 模型），已回退 StubEmbedding`。

**根因有两层**：
1. **用户配置错了**：你的 `config.json` 里 `providers` 只有 `deepseek`（**没有 fastembed**），`roles.embedding` 指向 `deepseek/deepseek-flash`。已修：补上 `fastembed`（kind `local-embedding`，模型 `bge-small-zh-v1.5`/`bge-m3`）、把 embedding 角色指到 `fastembed/bge-small-zh-v1.5`；权限仍 0600、DeepSeek 密钥原样保留。
2. **产品缺陷（已修）**：配置一旦存在就整份覆盖默认值 → 老配置里缺了后来新增的本地 embedding 供应商，**设置里根本选不到它**，用户只能选错。

**两道保险**：
| 保险 | 位置 | 行为 |
|---|---|---|
| 读配置时补回内置供应商 | `crates/octopus-api/src/config.rs` `with_missing_builtin_providers` | `get_config` 返回给前端的副本里补上缺失的 `local-embedding` 供应商；**不偷偷改写磁盘文件** |
| 配错时回落本地而非 Stub | `crates/octopus-api/src/ai.rs` `build_embedding_backend` | 配置的后端不可用时，先尝试**本地 fastembed**（有语义），只有本地也起不来才退 Stub（并继续大声 WARN） |
| 后端标识 | `ports.rs` `EmbeddingBackend::backend_name` | Stub 与 bge-small 都是 512 维，光看维度分不出退到了哪个；加标识便于日志排障与测试断言 |

**验证（我自己跑的）**：`cargo test --workspace` → api **30**（+2 新测试）/ engine 181 / types 92 / ai / index 3 全绿；`pnpm -C frontend build` 通过。新测试：`misconfigured_embedding_role_falls_back_to_local_fastembed`、`legacy_config_gains_missing_local_embedding_provider`。

**你要做的**：重启后端（配置在启动时读取）；首次嵌入会下载约 100MB 权重到 fastembed 缓存。

## #04b 回合内 3 轮 tool-call 续跑

**做了什么**：主线 AI 阶段新增 `MAX_STORY_AI_ROUNDS = 3` 的续轮循环——本轮 intent 结算后，若产生了**新引擎信息**（`query_world` 答案 / `check` 结果 / `interact` 结果），把它们装进新的 `TurnContext.turn_feedback` 再调一次模型，直到 `finish_turn`、无新信息或到 3 轮上限。`finish_turn` 仍在结算前拦截（不算意图、不校验、不进日志、不发事件）。首轮 `turn_feedback` 为空 → 单轮路径的提示词**逐字不变**；回喂只装模型自己查的结果，不塞世界全量。`replay()`/`replay_from()` 未动（权威仍全走命令日志）。`STORY_PREAMBLE` 加了第 10 条说明这个循环。

**验证（我自己跑的）**：`cargo test --workspace` → engine **177** / api 28 / types 92 / ai 18（1 ignored）/ index 3 / migration 1 全绿；`pnpm -C frontend build` 通过。5 个新测试：查询→续轮两轮效果都落地、一直查询恰好停在 3 轮、`finish_turn` 不发事件、立即 finish 保持单次调用、多轮日志重放逐位一致。

**明确未做**：角色 AI 并行与 per-AI scope 过滤（见 #04c）、`opponent_id/target_value`、Lua `QueryWorld` 回灌、`interact` 挂载技能；`turn_feedback` 未额外按 token 预算截断（查询答案本身已按关键词收窄）。

## #04c 角色 AI 按角色并行 + scope 过滤

**做了什么**：`present_npcs()` 取出在场 NPC（按 template id 升序）；**多于 1 个**时给每个 NPC 构造裁剪后的上下文并 `join_all` **并行**调用（新增 `AiProvider::character_intents_scoped`，默认实现退回旧入口，既有 provider 零改动），结算严格按 actor id 升序 → **事件顺序与完成顺序无关**；缺省归属改为本次调用负责的那个 NPC（旧实现一律归首个 NPC）。**单 NPC 走原路径**，提示词与事件顺序逐字不变，并用**金样本**（`crates/octopus-ai/tests/fixtures/single_npc_character_prompt.txt`，改动前快照）`assert_eq!` 锁死。

**scope 过滤（#16 认知边界）**：scoped 提示词只给本人的 persona、不给本任务/其它场景，`【你的角色】` 段声明只扮演本人；查询侧新增 `handle_intent_scoped`——属性/资源/物品只看自己，世界标记/任务/遭遇/地点不给，关系只给触及自己的边；新增 `query_character`（查他人直接驳回 `target_invalid`）与 `query_relationships`。**「主线 AI 获胜」**编码为「主线阶段整体先结算 + 同一 `intent_id` 主线先登记、角色侧被去重」，并有测试同时断言顺序与去重。

**验证（我自己跑的）**：`cargo test --workspace` → engine **181** / ai 20（1 ignored）/ api 28 / types 92 / index 3 全绿；`pnpm -C frontend build` 通过。

**明确未做**：`ProtocolPanel.vue` 白名单 UI 未更新；两个新查询意图**故意没写进**单 NPC 的 preamble（会破坏逐字兼容），只有多 NPC 的 scoped 提示词宣传它们；`present_npcs` 只覆盖 NPC（非受控可玩角色仍不单派调用）；自身属性/资源通过收窄后的 `query_character` 获取而非注入提示词。

## #01 声明区校验 + 命名统一

**做了什么**：新增三条 **Error**（仅当对应声明区**非空**时才校验 —— 空表示作者未启用该契约，旧故事书行为不变）：`dangling_event_ref`（`skills[].effect.triggers[].event` 不在 `events`）、`dangling_relationship_type_ref`（`relationships[].type` 与 `relationship_ge` 条件的 `type` 不在 `relationship_types`）、`dangling_target_type_ref`（`skills[].target` 不在 `target_types`）；既有 `undeclared_flag`（Warning）的 code/severity **未改**。补上 `objects` 的引用校验（`location_id`/`skills`/`condition`）。

**命名统一（向后兼容）**：关系边端点规范为 `from`/`to`（旧 `from_id`/`to_id` 走 `relationship_endpoint` 读取兼容，conditions/lua_host/validate 统一走它，前端载入时归一化后写规范名）；场景事件规范为 `scene`（旧 `scene_change` 通过 `normalize_event_name` 兼容匹配）。`world.check`、`kind: pc|npc` 核对后本已规范，未动。

**验证（我自己跑的）**：`cargo test --workspace` → engine **172** / api 28 / types 92 / ai 18（1 ignored）/ index 3 / migration 1 全绿；`pnpm -C frontend build` 通过。8 个新测试覆盖四类悬空引用、规范名与旧名都能通过校验、纯旧名故事书仍干净、声明区为空时不误报、objects 引用、scene 别名。

**明确未做**：声明区条目缺 key 的校验（避免新增非「悬空」失败）、`set_flag` 对 flags 的引用校验（沿用既有 Warning）、`objects.actions/condition` 的运行期消费（属 #04）。

## #04/#12 协议与规则（收窄版）

**做了什么**：
1. **`check` 走故事书判定器**：`session.rs::run_check` 复用 `command::run_check`——声明的骰式、`attribute_modifier`/中心偏移、`degree_thresholds`、`mode gte|lte|opposed`（含 Lua 判定器）。`world.check` 缺失/非法时**逐字保留旧行为**（1d20、无修正、默认分档、target 12）；移除了硬编码的 `mine_foreshadow` 演示 flag；`CheckResult` 载荷 schema 与旧日志重放兼容。
2. **`intent_id` 幂等去重**：`IntentEnvelope.intent_id` → `Option<String>`，新增 `parse_intent_envelopes`，`AiOutput.intents` 改为携带包络；`run_round` 用回合级 `HashSet` 跨主线/角色两阶段去重。缺省/空 id → 与今天完全一致（不去重）。
3. **`interact` + 真 `query_world`**：新增 `Intent::Interact { object_id, action }`（目录/前言/kind）；对 `storybook.objects` 校验，未知物件/动作 → `target_invalid`，故事书无 objects → 优雅降级并说明。`query_world` 不再 no-op：从权威 `WorldState` 汇总（场景/在场/属性资源/标记/任务/遭遇/地点/关系/物件+背包），落 `Resolution` 并进 `canon_lines` 供后续回合读到（切片内等价的回喂通路；隐藏任务不返回、只报在场角色）。
4. **actor/scope 约束**：显式 `actor_id` 指向不存在/不在场 → `actor_not_found`（不再静默改判）；角色 AI 阶段显式指向受控 PC → `actor_not_controlled`。未提供 actor_id 时保持既有文本推断 + 阶段回落；**我修的「PC 名不从句中推断」保持有效**（回归测试仍绿）。

**验证（我自己跑的）**：`cargo test --workspace` → engine **164** / api **28** / types **92** / ai 18（1 ignored）/ index 3 / migration 1 全绿；`pnpm -C frontend build` 通过。6 个新测试：声明式 check、缺省 1d20、lte 模式、intent_id 去重、query_world、interact 降级与 actor 越权各情形。

**明确不在本切片**：3 轮 continue 循环、角色 AI 按角色并行、按 AI 的查询 scope 过滤（`query_character`/`query_relationships`）、`check` 的 `opponent_id`/`target_value`、Lua `QueryWorld` 请求通路（仍是 no-op）、`interact` 未执行物件挂载的 skills/conditions。

## #06 确定性缺口（RNG 持久化 + 全量快照）

**它先核验了差距分析，并纠正了一处说法**：差距分析说「session 从未把 rng 消耗写进日志」——**部分有误**（技能管线一直有写 `rng_consume` 事件，既有测试也一直过）。但**真缺陷成立**：`replay` 从不恢复 RNG 位置，且 `check` 意图（硬编码 1d20）、状态 tick、触发效果、伤害骰这些路径**根本没记录**消耗。修复前复现：不重启下一掷 = 11，重启重建后 = 12（发散）。

**改法**：`DeterministicRng` 暴露 `position()/restore(n)`（splitmix64 状态是「种子+次数」的纯函数）；`session.rs` 用统一的 emit 漏斗按水位线把所有新消耗写成 `rng_consume` 事件（所有掷骰路径一视同仁），`apply_event` 累加 `WorldState.rng_position`，`replay/replay_from` 末尾据此复位 RNG，`flush_events`/`apply_rewind` 也保持一致；删掉技能管线里那条重复事件。旧日志（无该事件）`rng_position` 缺省 0，行为同今天。

**快照（派生启动缓存）**：migration `000017` 建 `snapshots` 表（含 `format_version`/`storybook_revision`/`state_json`），保留最新 **5** 份（同事务裁剪）；`session_for` 读最新快照，经「格式版本 + 内嵌故事书版次 + JSON 可解析 + seq 不超日志」四重校验后只重放 `seq >` 快照部分（全部命令仍进 event_log 供 history），任一不满足回退全量重放；写入时机 = 手动存档 + 每 10 回合。**#14 的 Origin 检查点未动**——那是权威的，快照只是派生缓存。

**验证（我自己跑的）**：`cargo test --workspace` → engine **156** / api **28** / types **92** / ai 18（1 ignored）/ index 3 / migration 1 全绿；`pnpm -C frontend build` 通过。新增测试：重启后骰值与世界状态一致、快照+后续重放 == 全量重放（状态/seq/RNG 三者）、保留 5 份、缺/过期/坏 JSON 回退。

**明确未做**：**Player 实体**（#06⑥）；干净退出/场景切换触发快照（无 quit 端点）；没有端到端 HTTP 测试证明「快照路径确实被采用」（只有引擎级等价性 + 门禁单测 + 既有重启 e2e）。

## #14 存档升级链路 + 新原点（差距 #1）

**做了什么**：`needs_upgrade` 改为**读取时计算**（`embedded_revision < 故事书当前已发布 revision`；沙箱存档不提示）；新增 `POST /api/saves/{id}/upgrade/dry-run`（`UpgradeReport{from,to,groups:{changes,gone_characters}}`，纯函数 `upgrade::compute_upgrade_report`）与 `POST /api/saves/{id}/upgrade`（逐项裁决 `freeze`/`departure`）。执行顺序：**先自动备份**（导出 `.octopus.zip` 到 `backups/`，失败则中止且不写任何东西）→ 换冻结故事书、bump revision → 应用裁决（departure = 权威 StateUpdate + System 提示；freeze 定义进 `legacy` 区）→ 维护历史 → **先写一条全量状态 Origin 检查点事件**再写裁决事件，保证升级后可从日志重放而不是重跑 diff。校验严格（缺/未知/重复裁决 → 400），重复执行为事务内幂等。「新原点」端点（原 501）实现：写 Origin 检查点 → 旧行进库内 `archived_commands` → 维护历史 → 丢缓存会话，单事务。

**验证（我自己跑的）**：`cargo test --workspace` → engine **152** / api **27** / types **92** / ai 18（1 ignored）/ index 3 / migration 全绿；`pnpm -C frontend build` 通过。新增测试含 API 端到端两段式升级（dry-run 列出消失角色 → 缺裁决 400 且存档不变 → 执行 bump/legacy/备份落地/维护/日志事件/离场投影 → 幂等重执行）与「新原点 + 重启后重放一致」。

**明确未做（据实记录）**：① 日志/快照的 `format_version` 字段；② golden fixtures 目录（测试有，无 fixtures 树）；③ **无 snapshot 表**——升级/新原点用「日志内全量检查点事件」代替（v1 无快照；重放正确且原子，但检查点载荷大）；④ 冻结角色的**规则**未与新故事书合并（数据/实例保留，运行时查不到被删的技能/物品）；⑤ 受控 PC 若被裁决离场，`controlled` 不重指；⑥ dry-run 只 diff 顶层实体数组 + 定义变更，不深 diff skeleton/narrative/attribute_dimensions/lore；⑦ 备份只在磁盘、无列举/下载 API；⑧ 备份格式是既有 `.octopus.zip` 而非 `.sqlite`（属 #27）。

**环境提醒**：执行期间 `/home` 曾 100%，`libduckdb-sys` 构建脚本在 `ar` 阶段失败；subagent 删了 5 个重复的 `libduckdb-sys-*` **构建缓存目录**（约 10GB，未动仓库文件）后恢复。当前 `/home` 96%（约 14G 可用），`cargo-target` 占 150G。

## M3b 记忆管线收口（微摘要 + 场景压缩）

**1. 回合微摘要（不额外调用）**：新增 `Intent::Summary { text }`（octopus-types，ts-rs 绑定已重生）；`protocol.rs` 把 `summary` 加进 `KNOWN_INTENTS`/`INTENT_CATALOG`/`intent_kind`，并在 `STORY_PREAMBLE` 里要求主线 AI **每回合输出一条摘要**。`handle_intent(Summary)` 只写派生表、**不产生事件、不改状态**。

**2. 场景压缩**：`AiProvider` 新增**带默认实现**的 `summarize()`（`Ok(None)`，所以既有实现/测试 mock 不受影响）；`RigProvider` 用 **pair 角色**实现（普通 completion + `SUMMARY_PREAMBLE`，配置缺 pair 时回落 story）。`AdvanceScene` 成功时把该场景窗口内的回合摘要压缩；返回 None 或报错 → **确定性拼接兜底**，任何失败都只 warn、绝不让回合失败。`replay()` 从最后一条 `advance_scene` 结果重建压缩窗口；摘要不进命令日志，所以重放天然忽略它们。

**3. 存储/检索**：migration `000015` 建 `round_summaries(save_id, round, text)` 与 `scene_summaries(...)`；派生索引的 seq 放在**负 i64 空间**，与命令日志 seq 不冲突；摘要行与 `events_fts` 同事务写入（同一 CJK tokenizer，kind = `summary`/`scene_summary`）；`load_narrative_events`（重建）与 `load_narrative_events_by_seqs`（检索回填）都纳入摘要，因此 **`HybridMemoryRetriever` 一行都不用改**就能检索到摘要。

**验证（我自己跑的）**：`cargo test --workspace` → engine **145** / api **25** / ai 18（1 ignored）/ index 3 / types 82 / migration 1 全绿；`pnpm -C frontend build` 通过。

**顺带修的既有隐患**：`round_narrative_events_are_indexed_best_effort` 在 `RoundEnd` 前就数数（既有竞态），加了 `wait_round_end` 助手并用于两个测试，重复跑稳定。

## M3a 记忆检索（写作侧索引 + 混合检索 + 中文修复）

**1. 中文 FTS 修复**：新增 `crates/octopus-engine/src/text_index.rs` —— `index_text()` 把 CJK 逐字空格分开（`月光下的古堡` → `月 光 下 的 古 堡`）、拉丁/数字整词保留；`match_expression()` 把连续 CJK 合成 FTS5 **短语**（`"古 堡"`）。写入与查询共用同一 tokenizer，tokenizer 仍是 unicode61（预分词后即逐字成词，无需改迁移）。测试覆盖：「古堡」命中「月光下的古堡」、拉丁词、`save_id` 隔离、短语相邻性（「光古」不命中）。

**2. 写作侧索引**：`PersistingSink` 在权威落库成功后、广播前，只对叙事事件 `try_send` 进**有界队列（256）+ 每会话单 worker**（满则 warn 丢弃）→ embed → `VectorIndex::upsert`。既避免「每事件 spawn 一个任务」的无界增长，也不阻塞落库/广播；失败只 warn（索引可重建）。

**3. 检索注入**：`MemoryHit` + `MemoryRetriever` 端口、`TurnContext.memories`；`HybridMemoryRetriever` = 向量 top-K ∪ FTS top-K，按 seq 去重（向量优先），K=5，原文从权威 commands 按 seq 回填；降级链 向量→FTS→空，**绝不向上报错**。`Session::set_memory_retriever(...)`（`Session::new` 签名不变，避免大量测试改动），`AppState::session_for` 统一注入。提示词新增 `turn_prompt_story()`：**【相关往事】只给主线 AI**，角色 AI 仍用不含记忆的版本（「所见即所知」）。

**验证（我自己跑的）**：`cargo test --workspace` → engine **139** / api **24** / ai **18**（1 ignored）/ index 3 / types 82 / migration 1 全绿；`pnpm -C frontend build` 通过。

**顺带修的既有问题**：`test_session_survives_restart_via_command_log` 的轮询条件原先只等任意 `RoundEnd`，元指令回合会提前 break（既有 flaky）；改为等 `RoundEnd(p) if p.round >= 2`。

**已知限制**：整段连续中文查询会合成一个长短语，FTS 召回偏窄——定位为短关键词兜底，语义召回交给向量；`events_fts` 现在存分词文本，M2 时期写入的未分词旧行不命中新查询（派生表，可 `rebuild_memory_index` 重建）。

## M2 DuckDB 向量库 + FTS5 + WAL

**做了什么**：`VectorIndex` 端口（`upsert`/`search`/`dimension`/`rebuild_from`）落在 engine；新 crate **`crates/octopus-index`** 用 `duckdb` bundled 实现 `DuckDbVectorIndex`，自有派生文件 `octopus-vectors.duckdb`（`event_vectors(save_id, seq, round, kind, text, embedding FLOAT[N])` + `meta.dim`；维度不符自动 DROP 重建；`array_cosine_similarity` 暴力 top-K，不上 HNSW）。权威库加 FTS5 `events_fts`（migration `000014`），**与 commands 同事务写入**；`PRAGMA journal_mode=WAL` 已开；`memory.rs` 提供 `rebuild_memory_index`（读命令日志重建）。`AppState` 第 5 个参数 `vector_index_path`（best-effort 打开，失败只 WARN 并禁用），bin 默认放 `octopus.db` 同级、`OCTOPUS_VECTORS` 可覆盖，`.gitignore` 忽略该派生库。

**验证（我自己跑的）**：`cargo test --workspace` → engine **130** / **octopus-index 3** / api 23 / ai 17（1 ignored）/ types 82 全绿；`pnpm -C frontend build` 通过。`duckdb bundled` 有编译证据（`Compiling libduckdb-sys` → `duckdb`）。

**暴露的问题（已排进 M3 修）**：**FTS5 的 `unicode61` 不能按字切中文**——实测「月光下的古堡」搜「古堡」不命中（连续中文被当成整词）。M2 按其规格保留 unicode61 并在注释写明；M3 会改成「CJK 逐字预分词 + 短语查询」或换 trigram，并补中文子串命中的测试。

**其它决定**：`rebuild_from` 收的是文本行、没有向量，所以 `.embed()` 的调用点在 `DuckDbVectorIndex` 内部（它持有注入的 EmbeddingBackend），engine 只负责读日志；DuckDB 加了 `PRIMARY KEY(save_id,seq)` + `INSERT OR REPLACE`；duckdb-rs 不能绑定数组，向量以内联有限浮点字面量写入。

## B 存档级模型（新建时选共用 / 分角色）

**做了什么**：新增 `saves.roles_json`（migration `m20260914_000013`，并把旧的 `model_*` 回填成 `{mode:"shared", story, character}`）。引擎 `TurnContext.model` → `RoleModels { story, character }`，`rig_provider` 主线/角色各取各的（含采样与思考强度）。`SaveRoles`/`RoleModel`/`SaveRoleMode` 进 octopus-types（ts-rs 导出）。API：`CreateSaveRequest.roles`、`SaveSettings.roles`，`PUT /settings` 会 `set_models`。前端：新建游戏弹窗（`NewGameDialog.vue`）+ 新组件 `RoleModelPicker.vue`（复用含「关闭」档的目录助手），游玩页 `InputBar` 共用时一个选择器、分角色时主线/角色各一个。读取顺序 = `roles_json` → 旧 `model_*`（套到两个角色）→ 全局默认。

**验证（我自己跑的）**：`cargo test --workspace` → migration 1 / ai **17** / api **22** / engine **128** / types **82** 全绿；`pnpm -C frontend build` 通过。

**决定/注意**：新建弹窗默认「分角色」（与今天全局拆分一致）；**换模型会清空思考强度**（避免旧 effort 落到新模型上）；`roles.pair` 保持全局；存档包导出/导入仍不带模型设置（原来也不带 `model_*`），导入后回落全局默认。

## M4 409 round_in_progress

**问题（差距分析高收益单点）**：`submit_round` 把 busy 判定放在 `tokio::spawn` 内，接口总是 202，前端拿不到 409——与设计决策第 6 条冲突。

**改法**：`Session::is_idle()`（无进行中回合且无待确认动作，`can_rewind` 改为引用它）+ `Session::has_seen_request(rid)`；`submit_round` **在 spawn 之前**判 `is_idle`，非 idle 且不是同一 `request_id` 的幂等重复 → `EngineError::RoundInProgress`（经 `error.rs` 既有映射变 409 `round_in_progress`）。前端 `play.ts` **本来就有** `round_in_progress` 的 toast 分支，现在终于能走到。

**验证（我自己跑的）**：新增 API 测试 `concurrent_round_submission_returns_409`（用阻塞 AI 把「回合进行中」稳定摆出来：第一发 202 → 第二发 **409 + code=round_in_progress**）；`cargo test --workspace` → api **23**（+1）/ engine 128 / ai 17 / types 82 全绿；`pnpm -C frontend build` 通过。

## M1 本地嵌入（fastembed）

**做了什么**：`crates/octopus-ai/src/embedding.rs` 新增 `FastEmbedBackend`——懒加载且只初始化一次（`tokio::sync::OnceCell<Arc<TextEmbedding>>`），加载与推理都在 `spawn_blocking` 里跑，`OCTOPUS_EMBEDDING_CACHE` 可覆盖模型缓存目录，`dimension()` 返回真实维度。`crates/octopus-api/src/ai.rs` 按 `kind == "local-embedding"`（或 id `fastembed`）走本地后端，不再要求 `base_url`；远程 rig `/embeddings` 路径与 `OCTOPUS_EMBEDDING=stub` 都保留。默认配置的 `fastembed` 预设 kind 改为 `local-embedding`。回退 Stub 时现在是**大声 WARN**（说明 Stub 是伪向量、未来检索结果无意义），不硬失败。

**验证（我自己跑的）**：`cargo test --workspace` → octopus-ai **16**（+4，1 个 ignored）/ api 21 / engine 126 / types 79 全绿；`pnpm -C frontend build` 通过；`cargo build -p octopus-bin` 通过（日志确认 `Compiling fastembed v4.9.1`、`ort v2.0.0-rc.9`）。

**偏差（重要）**：`bge-m3` 用不了。`fastembed::EmbeddingModel::BGEM3` 只在 fastembed ≥5.x 存在，而 Cargo.lock 与 feature 无关 → rig 0.42 的可选 `rig-fastembed` 总会被解析并锁定 fastembed 4.x（`ort =2.0.0-rc.9`）；fastembed 5.x 要 `ort` rc.10+ 且都是精确锁定，Cargo 无法同时满足。现用 4.9.1，`bge-m3` 会返回**明确错误**并提示改用 `bge-small-zh-v1.5`，且有回归测试 `rejects_bge_m3_until_fastembed_is_upgraded` 在将来可用时失败提醒。真要启用需升级 rig / 改用 rig-core / 打补丁排除 `rig-fastembed`。

**注意**：该 subagent 跑过一次 `cargo fmt -p octopus-ai -p octopus-api`，可能顺带格式化了这两个 crate 里既有的未提交代码（纯格式，无逻辑改动）。

## 差距分析结论（`docs/gap-analysis.md`，锚点 git 4b5b3c4）

**计数**：✅ 完整 7 票（#02/#07/#09/#11/#17/#22/#25）；🟡 部分 18 票；❌ 缺失 2 票（#14 存档版本迁移、#19 演出模板自定义）。

**Top 缺口（报告原文口径）**：
1. **#14 升级整体缺失**：后端无 `/upgrade/dry-run`、`/upgrade`；前端 `upgradeDryRun/upgradeExecute` 是 mock-only；服务端 `needs_upgrade` 永不计算（写死 false）；「新原点」端点恒 501；无 `format_version`、无 golden fixtures。
2. **记忆/检索链路**：#05 摘要 + top5 注入 + 角色隔离全缺；#15 有 fastembed 能力但没人调用；#27 无 DuckDB、无 FTS5、无 WAL。
3. **#19 + #08 演出模板**：只有单一聊天流，无三模板/切换/token 换肤。
4. **#04 协议补全**：`interact` 无意图、`query_world` 是 no-op、check 硬编码 1d20 不用 `world.check`、无 `opponent_id/target_value`、无 intent_id 去重、无 3 轮 continue、角色 AI 不按角色并行、scope/actor 不强制。
5. **#06 确定性缺口**：RNG 消耗记录了但从不落日志/回放（重启后骰序可能重演）；无全量快照；无 Player 实体。

**高收益单点**：#24 的 `409 round_in_progress` **永远到不了客户端**（busy 判在 `tokio::spawn` 内，总是 202）。

## 今晚执行优先级（我按「用户点名 → 缺口大小/依赖 → 可验证性」排）

| 顺序 | 项 | 规模 | 说明 |
|---|---|---|---|
| 1 | **B 存档级模型** | 中 | 用户点名；进行中 |
| 2 | **M4 #24 409** | 小 | 用户点名 + 报告高收益单点 |
| 3 | **#06 RNG 持久化 + 全量快照** | 中 | 设计核心承诺（严格事件溯源/确定性重放）现在不成立 |
| 4 | **M2 #27 DuckDB + FTS5 + WAL** | 大 | 用户点名；记忆管线的前置 |
| 5 | **M3 #05 记忆管线** | 大 | 用户点名 |
| 6 | **#14 升级流程 + 新原点 + fixtures** | 大 | 缺口 #1；已有前端 UI 但后端不存在 |
| 7 | **#04 协议补全** | 大 | 缺口 #4 |
| 8 | 其余（#12/#13/#26/#18/#08/#20 等） | 中 | 时间允许则继续 |

> 说明：18 张「部分」不可能一晚全清。**已完成的每项都会在下面留证据；没做的会如实列出**，不会含糊。

## 设计书位置备忘

- 设计地图与决策：`docs/blueprint/map.md`、`docs/blueprint/decision-log-design-pass.md`
- 分票：`docs/blueprint/issues/01–27`；调研：`docs/blueprint/research/*`
- 本项目内的实现规格：`docs/narrative-contract.md`、`docs/protocol-interface.md`、`docs/p3-think-and-import.md`、`docs/save-role-models.md`、`docs/memory-pipeline.md`

## 未完成 / 需要你拍板

_（收尾时填写）_
