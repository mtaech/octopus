# AI→引擎动作协议

Type: grilling
Status: resolved
Blocked by: 01, 02, 03

## Question

定义主线 AI 与角色 AI 调用引擎的结构化动作协议（function calling）。需要拍板：
① 意图的 schema：动作类型（说 / 移动 / 使用技能 / 使用物品 / 掷骰判定 / 查询世界状态……）及各自参数。
② 引擎的「校验→结算→回写」管线：非法操作拒绝、数值结算、状态回写、结果回填给 AI。
③ Lua 脚本在结算管线中的挂载点。
④ 主线 AI 与角色 AI 谁有权产生哪类意图、冲突如何裁决。
⑤ 单回合内多次工具调用的循环边界与令牌预算。
## Answer

### (1) Intent type taxonomy (five categories)

| Category | Intents | Engine behavior |
|----------|---------|-----------------|
| Narrative | speak / narrate / emote | Logged to command log, no resolution |
| World | move / use_skill / use_item / interact | Validate -> resolve -> commit |
| Mechanical | check | Engine rolls dice + applies modifiers + returns success level |
| Query | query_world / query_character / query_relationships | Read-only, no command produced |
| Meta | switch_character / save / intervene | Player-only, AI cannot initiate |

### (2) Intent schema: discriminator union, one function per type

Common envelope: { intent_id: UUID, type: string, actor_id: string, params: {...} }

Each type is a separate function calling tool. MVP params below, additionalProperties: true for gradual extension.

| type | MVP params |
|------|------------|
| speak | content, tone? |
| narrate | content, style? |
| emote | emotion, gesture? |
| move | destination_id |
| use_skill | skill_id, target_id? |
| use_item | item_id, target_id? |
| interact | object_id, action |
| check | attribute, difficulty? |
| query_* | query, filters? |
| finish_turn | (no params) |

### (3) Validate -> Resolve -> Commit -> Feedback pipeline

Intent -> [1.Validate] -> [2.Resolve] -> [3.Commit] -> [4.Feedback]

1. Validate: Six rejection codes (actor_not_found / actor_not_controlled / target_invalid / insufficient_resource / cooldown_active / rule_violation). Rejected intents do NOT enter command log.
2. Resolve: pre-roll Lua -> roll dice -> post-roll Lua -> pre-resolve Lua -> core effect -> post-resolve Lua
3. Commit: Atomic state write + append command log + record RNG consumption. Events broadcast after this phase, triggering Lua event handlers.
4. Feedback: Return ResolutionResult (status / narrative / outcome / state_changes / triggered_events)

### (4) Lua mount points (7)

| # | Timing | Location |
|---|--------|----------|
| 2 | Check modifier, pre-roll | Resolve phase, before dice |
| 5 | Check modifier, post-roll | Resolve phase, after dice |
| 3 | Pre-resolve hook | Resolve phase, before core effect |
| 4 | Post-resolve hook | Resolve phase, after core effect |
| 6 | Event trigger | After Commit phase, engine broadcasts event |
| 7 | Condition eval | End of turn, beat/goal evaluation |

No Lua in Validate phase.

### (5) Story AI / Character AI authority boundary

| Intent class | Story AI | Character AI | Player |
|-------------|:--:|:--:|:--:|
| Narrative | Yes (narration) | Yes (own character) | Yes (own character) |
| World | Yes (any NPC) | Yes (self only) | Yes (controlled char) |
| Mechanical | Yes (any character) | Yes (self only) | Yes (controlled char) |
| Query | Yes (full scope) | Yes (scope-limited) | Yes (full scope) |
| Meta | No | No | Yes (exclusive) |

Conflict resolution: Story AI wins. Character AI has exclusive authority only over its own character (unless player-controlled).

### (6) Character AI query scope limits

| Query | Story AI scope | Character AI scope |
|-------|---------------|-------------------|
| query_world | All locations/scenes/characters | Current scene + line of sight |
| query_character | Any character instance | Self + current target |
| query_relationships | Any entity pair | Self-related edges only |

Engine enforces filtering based on actor_id.

### (7) Multi-turn tool call loop boundary within a round

Story AI produces intents -> Character AIs produce intents (parallel) -> Engine resolves sequentially -> event chain drains naturally.
AI may declare continue for supplementary actions (max 3 rounds, configurable).
AI declares finish_turn or hard cap reached -> round ends -> streaming output.
Round 2+ only produces necessary actions revealed by round 1 results.

### (8) LLM call count and token budget

| Call | Count | Window |
|------|-------|--------|
| Story AI intents | 1 + max 2 continue | 8K input |
| Character AI intents (parallel) | 1 per active char + max 1 continue | 4K input |
| Single tool call result | - | <= 2K output |
| Total round output | - | <= 4K tokens |

Character AI calls run in parallel. Story AI runs before Character AI.

### (9) Idempotent deduplication

Engine maintains per-round intent_id set. Duplicate id returns cached ResolutionResult. Set cleared at end of round.
