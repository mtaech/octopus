# AI Provider 配置与凭据管理

Type: grilling
Status: resolved
Assignee: agent
Blocked by: 10, 20

## Question

25 张票只定了 Provider 抽象（#10 选 rig `CompletionModel`、#20 定 `AiProvider` 端口），**用户侧**如何配置 LLM、密钥存哪、模型怎么分工、失败怎么办，完全没有设计。需要拍板：

① 配置内容与形态：provider / base_url / model / 生成参数（温度、max tokens）在哪配、什么格式；是否提供设置 UI。
② 密钥存储：keyring vs 配置文件 vs 环境变量；多 provider 多 key 的组织。
③ 模型分工：主线 AI、角色 AI、Embedding 是否可分别选模型/端点。
④ 失败与降级：超时、重试、限流、结构化输出/工具调用解析失败、无 key/未配置时的行为。
⑤ 成本护栏：每回合 / 每存档的 token 与调用次数上限是否可配、超限如何提示。

## Answer

**决议（设计复审 2026-09-09 拍板）：**

### ① 配置形态

- 单一配置文件：app 数据目录 `config.toml`（人类可读、可手改），**权限 0600**。
- 结构：`[providers.<id>]`（kind/base_url/api_key/model/params）+ `[roles]`（story / character / embedding 各指向一个 provider+model）。
- 设置 UI：v1 提供基础设置面板（provider 选择、key 输入、模型选择、连通性测试）；写回配置文件，不引入服务端密钥管理。

### ② 密钥存储

- **本地配置文件（0600）**，不落 keyring（评审确认）。日志与错误信息中密钥一律脱敏。
- 支持环境变量覆盖（开发友好），优先级：环境变量 > 配置文件。

### ③ 模型分工

- **主线 AI / 角色 AI / Embedding 三类各自可配**（评审确认）。
- 默认：主线用强模型、角色用便宜模型、embedding 固定本地 `bge-small-zh-v1.5`（#15）；三者可指向同一 provider。
- 每类可覆盖生成参数（温度、max tokens、超时）。

### ④ 失败与降级

- 超时/限流/5xx：指数退避重试（有上限），仍失败则发 `system` 事件（error）并**中止本回合**，不写半截命令（#04 四阶段管线的原子性）。
- 结构化输出 / 工具调用解析失败：把原始输出与解析错误回填给模型重试一次（#04 单回合 3 轮上限内），仍失败则按纯叙事降级并提示。
- 未配置 / 无 key：进入只读引导态（游玩页提示「去设置 Provider」），不发起调用。

### ⑤ 成本护栏

- 每存档可设每回合 token 上限与单回合调用次数上限（默认取 #05 的预算）；超限发 `system` 事件（warn）并收束回合。
- 提供本存档累计 token 统计（只读展示，不入命令日志）。

## 落账

- 新票，无前置阻塞；解锁「Provider 设置 UI」随 #22 编辑器/设置页落地。
- 影响：`octopus-ai` 实现 `AiProvider` 端口的配置装配；#20 端口不变。
