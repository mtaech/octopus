# 页面迁移契约 · .oct-* → shadcn-vue + Tailwind（subagent 必读）

> 背景：视觉系统已切到 shadcn-vue v2（reka base）+ Tailwind v4 + 夜行手记主题（墨黑 #141210 底 + 暖白文字 + 琥珀 #e8a44c accent）。主题 CSS 变量已在 src/style.css 定义好（--background/--foreground/--primary/--card 等 shadcn 语义色），全局生效。
> 你的任务：把指定页面从旧的 .oct-* 自定义控件迁移到 shadcn-vue 组件 + Tailwind 语义类。**信息架构、数据流、组件逻辑一律不动**，只换呈现层。

## 运行验证
```bash
export PATH=$HOME/.local/share/mise/installs/node/24.19.0/bin:$HOME/.local/bin:$PATH
cd /home/huang/Personal/Dev/Code/octopus/frontend
npm run typecheck  # vue-tsc -b
npm run build
# 或 npx vite --port 5173 起 dev 看效果
```

## 已装 shadcn-vue 组件（src/components/ui/）
alert avatar badge button card dialog dropdown-menu empty input label radio-group scroll-area select separator sheet skeleton switch tabs textarea tooltip
import 路径：@/components/ui/button、@/components/ui/card、...（每个目录有 index.ts 导出组件）。

## 主题语义色（Tailwind 类）
- 底色：bg-background / bg-card / bg-popover / bg-secondary / bg-muted / bg-accent
- 文字：text-foreground / text-muted-foreground / text-primary / text-card-foreground
- 边框：border-border / border-input；强调 ring：ring-ring
- 主色（琥珀）：bg-primary text-primary-foreground；破坏：bg-destructive/10 text-destructive；成功/警告信息补充：text-success / text-warning / text-info
- 字体：font-sans（Geist 默认）、font-serif（Noto Serif SC，标题/故事感可用）

## 类映射（旧 → 新）
- oct-btn → <Button>；oct-btn--primary → variant="default"（或 primary 语感用 default）；oct-btn--ghost → variant="ghost"；oct-btn--danger → variant="destructive"；oct-btn--sm → size="sm"；oct-btn 无 primary → variant="outline" 或 secondary
- oct-input → <Input>；oct-textarea → <Textarea>；oct-select → <Select>（含 SelectTrigger/SelectContent/SelectItem，注意 SelectItem 需包 SelectGroup）
- oct-card → <Card>（CardHeader/CardTitle/CardContent/CardFooter 组合）
- oct-badge → <Badge>；--ok → variant 自定或 text-success 类；--warn → text-warning 类
- oct-muted → text-muted-foreground；oct-faint → text-muted-foreground/70 或 text-xs
- 自定义弹窗（新建游戏/向导等）→ <Dialog> 或 <Sheet>（右侧抽屉用 Sheet side="right"）
- 自定义选项卡 → <Tabs> + TabsList/TabsTrigger/TabsContent
- 角色头像 → <Avatar> + AvatarFallback
- 确认/单选 → <RadioGroup>；开关 → <Switch>；提示 → <Tooltip>/<Alert>；空态 → <Empty>；骨架 → <Skeleton>

## 硬性规则
1. 禁止再新增 .oct-* 类。改一个删一个旧的 class。
2. 颜色只用语义 Tailwind 类（bg-*/text-*/border-* 映射上面色板），**禁止**写死 hex 或旧 --tpl-* 变量（旧 tokens.css 里 .tpl-* 仅兼容遗留，别新增依赖）。
3. 图标用 @tabler/icons-vue（已装）：import { IconX, IconPlus } from '@tabler/icons-vue'，配 data-icon 或 size 由组件管。**禁止新 emoji 图标**；旧 emoji 尽量换成 tabler 图标。
4. 间距用 Tailwind（gap-2 等），不用 space-y-*。等宽尺寸用 size-*。
5. 圆角/阴影交给组件默认（夜行手记主题已统一），别手写 radius。
6. 布局仍可用 scoped style 但尽量内联 Tailwind；如需局部特殊样式写 scoped CSS 引用语义 token（var(--background) 等）可以，但优先 Tailwind。
7. 保持中文文案与术语（CONTEXT.md），别改业务逻辑/store/数据。
8. 完成后 typecheck + build 必须零错误。若报错来自他人目录（并行迁移），只修自己目录。
9. 迁移后每个页面试跑 dev 确认不崩（可在报告里说明）。

## 交付（报告给主 agent）
- 迁移文件清单；改了哪些 .oct-* → 组件；是否引入新组件（若有需要但未装，列出组件名由主 agent 统一 add）。
- typecheck/build 结果；有无视觉遗留问题。
