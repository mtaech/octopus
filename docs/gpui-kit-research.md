# GPUI Kit 调研报告

> 调研目的：评估用 GPUI Kit 为 Octopus 写一个「本地管理面板」的可行性、上手方式与架构建议。
>
> 调研方式：只读一手资料（官方文档站、仓库源码、crates.io / docs.rs 元数据、官方 release）。每条关键结论都附来源链接。
>
> 调研日期：2026-09-12

## 一、结论速览

- **GPUI Kit 是 Longbridge 开源的 Rust 桌面应用框架**，把 Zed 的 GPUI 渲染层、一套无样式的行为基座（gpui-base）、一套 shadcn 风格的成品组件库（gpui-component）打包成一个 crate。它已经用于商业产品 Longbridge Pro。来源：[README](https://github.com/longbridge/gpui-kit)、[官网](https://gpui-kit.com)。
- **可以满足本地管理面板的需求，而且相当对口**：官方就有侧边栏（Sidebar）、可虚拟滚动的数据表格（Data Table）、表单（Form）、设置页（Settings）、命令面板（Command）、Dock 可拖拽多面板、图表（Chart/Plot）、Markdown/HTML 渲染、代码编辑器。来源：[组件目录](https://gpui-kit.com/llms.txt)、[Data Table](https://gpui-kit.com/component/data-table.md)、[Dock](https://gpui-kit.com/component/dock.md)、[Sidebar](https://gpui-kit.com/component/sidebar.md)。
- **当前版本 0.6.1（2026-09-09 发布），Apache-2.0**，依赖已发布的 `gpui-pre` 0.3.1 快照（不需要 git 依赖），edition 2024，要求 Rust 1.90+。本机是 rustc/cargo 1.97.1，满足要求。来源：[crates.io gpui-kit](https://crates.io/crates/gpui-kit)、[仓库 Cargo.toml](https://raw.githubusercontent.com/longbridge/gpui-kit/main/Cargo.toml)、[安装文档](https://gpui-kit.com/docs/installation)。
- **门槛在图形栈而非 Rust 版本**：Linux 需要 Vulkan、Wayland/X11、xkbcommon、fontconfig、webkit2gtk-4.1 等。本机已具备绝大部分（见第四节实测表）。
- **主要风险是它还很新**：`gpui-kit` 这个 umbrella crate 是 2026-09 才发布的新东西（crates.io 上只有 3 个版本），处在 0.x 阶段，API 可能变。底层的 `gpui-component` 则成熟得多（2025-02 首发、26 个版本、累计 11.5 万次下载）。建议锁死具体版本号。来源：[crates.io gpui-kit](https://crates.io/crates/gpui-kit)、[crates.io gpui-component](https://crates.io/crates/gpui-component)。
- **推荐落点**：在现有 Cargo workspace 里新增一个成员 `crates/octopus-panel`，作为独立的桌面二进制；数据通过现有 axum HTTP API（默认 `127.0.0.1:8787`）读取，复用 `octopus-types` 的类型，不重复实现引擎逻辑。

## 二、GPUI Kit 是什么

### 2.1 定位

一句话：**用 Rust 写跨平台桌面应用的完整框架**，而不只是一个控件库。官方把它描述为「production-ready 的 UI 系统 + 应用级的数据、布局、编辑能力，构建在可复用的行为/状态/基础设施之上，并对 JavaScript 扩展开放」。来源：[README](https://github.com/longbridge/gpui-kit)。

### 2.2 三层架构

README 给出的分层：

```text
gpui-kit             The one crate applications depend on
├── gpui-base        Unstyled behavior, state, and infrastructure
└── gpui-component   GPUI Component: the complete styled UI system
```

三种用法对应三种诉求（来源：[README](https://github.com/longbridge/gpui-kit)、[ARCHITECTURE.md](https://raw.githubusercontent.com/longbridge/gpui-kit/main/docs/ARCHITECTURE.md)）：

| 层 | 内容 | 适用场景 |
| --- | --- | --- |
| `gpui-component` | 完整、有样式、可主题化的 60+ 组件，提供生产可用的默认值 | 大多数应用（包括本项目） |
| `gpui-base` | 无样式的行为、状态与基础设施（键盘导航、虚拟化、焦点陷阱、弹层定位……） | 想自建设计系统、只复用难点行为 |
| `gpui-shell` | Rust host 内嵌的 JavaScript 运行时，按能力逐项授权 | 发布后仍要被脚本扩展的产品 |

架构原则（官方原话）：**行为归基础层，表现归应用层**。依赖方向永远向下，`gpui-base` 不得反向依赖 component 的主题或资源。

顶层 crate 的结构（[crates/kit/Cargo.toml](https://raw.githubusercontent.com/longbridge/gpui-kit/main/crates/kit/Cargo.toml)）：

- 默认 features：`default = ["component", "assets"]`，即默认带上有样式的组件库和默认图标集（Lucide）。
- 只想要无样式基座时可 `default-features = false, features = ["component"]`（注意：`component` 本身会初始化 base）。
- 还提供 `tree-sitter`、`decimal`、`inspector`、`profiler`、`test-support` 等可选 features。

### 2.3 与 GPUI、gpui-component 的关系

- **GPUI** 是 Zed 编辑器的 UI 框架（Apache-2.0）。GPUI Kit 不是 fork，而是「钉住一个匹配的 GPUI 版本并 re-export」。仓库里把 `gpui` 映射到已发布的 `gpui-pre` 0.3.1 快照 crate，所以应用侧不出现 git 依赖。来源：[仓库根 Cargo.toml](https://raw.githubusercontent.com/longbridge/gpui-kit/main/Cargo.toml)。
- **`gpui-component` 是这套生态的旧主线**，2025-02-06 首发于 crates.io；仓库随后演进为 `gpui-kit` umbrella（2026-09-03 首发），`gpui-component` 降为其中的「有样式组件层」。迁移到今天，`gpui-kit` 一个依赖就能拉到 GPUI + base + component + assets。来源：[crates.io gpui-kit](https://crates.io/crates/gpui-kit)、[crates.io gpui-component](https://crates.io/crates/gpui-component)、[README 的 Usage 段](https://github.com/longbridge/gpui-kit)。
- 确切的改名时间点官方没有单独公告，标记为 **UNVERIFIED**；但从仓库成员、crate 发布时间和文档口径可以确认「GPUI Kit = 现在的总入口，gpui-component = 其中一层」。

### 2.4 生产验证

README 明确写着它从第一天就用于构建 [Longbridge Pro](https://longbridge.com/desktop)，并在公开发售的商业桌面应用里持续打磨。Dock 布局文档也强调它是 Longbridge 生产环境的布局基础，不是孤立的 UI demo。来源：[README](https://github.com/longbridge/gpui-kit)、[Dock 文档](https://gpui-kit.com/component/dock.md)。

## 三、版本、许可与维护状态

| 项目 | 值 | 来源 |
| --- | --- | --- |
| 最新稳定版 | **0.6.1**（2026-09-09 发布） | [GitHub Release v0.6.1](https://github.com/longbridge/gpui-kit/releases/tag/v0.6.1)、[crates.io](https://crates.io/crates/gpui-kit) |
| 许可 | **Apache-2.0** | [README](https://github.com/longbridge/gpui-kit) |
| 底层 GPUI | `gpui-pre` 0.3.1（Zed GPUI 快照，Apache-2.0，notices 保留） | [仓库 Cargo.toml](https://raw.githubusercontent.com/longbridge/gpui-kit/main/Cargo.toml) |
| Rust edition | 2024 | [仓库 Cargo.toml](https://raw.githubusercontent.com/longbridge/gpui-kit/main/Cargo.toml) |
| MSRV | **Rust 1.90+** | [安装文档](https://gpui-kit.com/docs/installation) |
| 平台 | macOS 15+、Windows 10+、Linux（提供 bootstrap 脚本）；另有实验性 iOS/移动端与 WASM | [安装文档](https://gpui-kit.com/docs/installation)、[Comparison](https://gpui-kit.com/docs/comparison.md) |
| gpui-kit 下载量 | 7,264（新 crate，基本是 90 天内） | [crates.io](https://crates.io/crates/gpui-kit) |
| gpui-component 下载量 | 115,485 累计（26 个版本） | [crates.io](https://crates.io/crates/gpui-component) |
| 维护方 | Longbridge（主要作者 huacnlee） | [Release 作者](https://github.com/longbridge/gpui-kit/releases/tag/v0.6.1) |

发布节奏很快：v0.6.1 的 release note 包含多光标编辑、括号自动配对、智能缩进、headless UI 测试等（[Release v0.6.1](https://github.com/longbridge/gpui-kit/releases/tag/v0.6.1)）。这既是活跃的信号，也是 0.x 阶段 API 会变的信号。

与同类框架的定位对比（官方手工维护）：GPUI Kit 在「现代外观、CJK 支持、大表虚拟行列、图表、Dock 布局、Markdown 渲染、树形语法高亮、自定义主题」这些点上明显强于 Iced/egui；最小二进制约 12 MB。来源：[Comparison](https://gpui-kit.com/docs/comparison.md)。

## 四、快速开始

### 4.1 最小依赖

```toml
[dependencies]
gpui-kit = "0.6"
```

`gpui-kit` 会带入 GPUI 与 `gpui-base`；默认再启用 `gpui-component` 与默认图标集。来源：[README](https://github.com/longbridge/gpui-kit)、[Getting Started](https://gpui-kit.com/docs/getting-started.md)。

### 4.2 最小可运行程序

```rust
use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::*;

pub struct HelloWorld;

impl Render for HelloWorld {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_2()
            .size_full()
            .items_center()
            .justify_center()
            .child("Hello, World!")
            .child(
                Button::new("ok")
                    .primary()
                    .label("Let's Go!")
                    .on_click(|_, _, _| println!("Clicked!")),
            )
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            // 必须在使用任何组件前调用，负责主题与全局初始化
            gpui_kit::init(cx);

            cx.spawn(async move |cx| {
                cx.open_window(WindowOptions::default(), |window, cx| {
                    let view = cx.new(|_| HelloWorld);
                    // 窗口第一层必须是 Root，它管理 overlay / notification / 焦点
                    cx.new(|cx| Root::new(view, window, cx))
                })
                .expect("Failed to open window");
            })
            .detach();
        });
}
```

两个硬性约束（官方强调）：`gpui_kit::init(cx)` 必须是 `app.run` 闭包里的第一行；每个窗口第一层必须包一个 `Root`。来源：[Getting Started](https://gpui-kit.com/docs/getting-started.md)、[Coding Guides](https://gpui-kit.com/docs/coding-guides.md)。

### 4.3 加速 debug 构建

GPUI 和文本栈在 debug 下不优化会很慢。官方建议只对你依赖的重型 crate 开 `opt-level = 3`，自己的代码保持快速可调试的 debug 构建（profile 必须写在应用的根 `Cargo.toml`）：

```toml
[profile.dev.package]
gpui-pre          = { opt-level = 3 }
gpui-component    = { opt-level = 3 }
gpui-kit          = { opt-level = 3 }
gpui-kit-assets   = { opt-level = 3 }
gpui-pre-macros   = { opt-level = 3 }
gpui-pre-platform = { opt-level = 3 }
rustybuzz         = { opt-level = 3 }
taffy             = { opt-level = 3 }
ttf-parser        = { opt-level = 3 }
```

来源：[安装文档](https://gpui-kit.com/docs/installation)。

### 4.4 Linux 系统依赖与本机实测

官方 Ubuntu 24.04 安装脚本（[script/install-linux.sh](https://raw.githubusercontent.com/longbridge/gpui-kit/main/script/install-linux.sh)）需要：

```bash
gcc g++ clang libfontconfig-dev libwayland-dev \
libwebkit2gtk-4.1-dev libxkbcommon-x11-dev libx11-xcb-dev \
libssl-dev libzstd-dev vulkan-validationlayers libvulkan1
```

本机（Wayland 会话、`XDG_SESSION_TYPE=wayland`）实测：

| 依赖 | 本机状态 |
| --- | --- |
| rustc / cargo | 1.97.1 ✅（要求 1.90+） |
| Wayland client | 1.26.0 ✅ |
| xkbcommon / X11 / fontconfig / freetype | 均存在 ✅ |
| OpenSSL | 3.6.4 ✅ |
| webkit2gtk-4.1 / javascriptcoregtk-4.1 | 2.52.6 ✅ |
| Vulkan loader 与 ICD | /usr/share/vulkan/icd.d/ 有 radeon/nvidia/intel 等 ICD ✅ |
| `pkg-config vulkan` | MISSING（只是缺 .pc，loader/ICD 实际存在，通常不影响运行） |
| `pkg-config libzstd` | MISSING，建议补装 `libzstd-dev` 以免链接失败 |
| clang | 16 ✅ |
| 硬件 | 16 核 / 30 GB 内存，可接受 |

结论：本机基本可以直接构建，先补 `libzstd-dev`（以及保险起见 `vulkan-validationlayers`）即可。

### 4.5 官方示例与 AI 技能

仓库自带可运行的验证入口（[README](https://github.com/longbridge/gpui-kit)）：

```bash
cargo run                     # story gallery：展示所有组件
cargo run -p example-dock     # Dock 布局系统
cargo run -p system_monitor   # 实时图表（CPU/内存）
cargo run -p example-editor   # 带 LSP 的代码编辑器
```

官方还为编码 agent 提供了技能包，可直接安装：`npx skills add longbridge/gpui-kit`（含 `gpui-kit` 与 `gpui-kit-design-guides` 两个技能）。来源：[README](https://github.com/longbridge/gpui-kit)。

另外，文档站提供了给 LLM 用的聚合文本：`https://gpui-kit.com/llms.txt`（目录）与 `https://gpui-kit.com/llms-full.txt`（全文），任何文档页加 `.md` 后缀都能拿到 Markdown。这对「让 AI 帮忙写面板」非常友好。来源：[llms.txt](https://gpui-kit.com/llms.txt)。

## 五、组件清单（面向管理面板）

文档站 TOC 里 `/component/` 下的组件条目约 **72 个**，`/base/` 下另有 **38 个无样式 primitive**。来源：[llms.txt](https://gpui-kit.com/llms.txt)。

与本项目最相关的：

| 需求 | 组件 | 说明 |
| --- | --- | --- |
| 左侧导航 | **Sidebar** | 可折叠、分组、嵌套菜单、header/footer，文档明确点名 admin dashboards |
| 数据列表 | **Data Table / Table** | 虚拟滚动、排序、筛选、列宽拖拽/固定列、行/列/单元格选择、右键菜单、无限加载 |
| 多面板工作区 | **Dock** | 可拖拽 tab、嵌套 split、左/右/底 edge dock，布局可序列化 |
| 编辑与提交 | **Form / Field** | 标签布局、多列、footer；值/校验/提交由应用持有 |
| 应用设置 | **Settings / SettingPage / SettingGroup / SettingItem** | 类 macOS/iOS 设置页，支持搜索过滤 |
| 快速跳转 | **Command** | ⌘K 命令面板，分组 + 键盘导航 |
| 弹层 | **Dialog / AlertDialog / Sheet / Popover / HoverCard** | 创建/编辑/确认 |
| 反馈 | **Notification / Message / Alert / Spinner / Skeleton / Progress** | 异步状态与提示 |
| 输入 | **Input / Textarea / NumberInput / OtpInput / Select / Combobox / DatePicker / Calendar / Switch / Checkbox / Radio / Slider / ColorPicker** | 表单控件齐全 |
| 监控 | **Chart / Plot** | system_monitor 示例即用图表 |
| 层级 | **Tree** | 自管展开/选择/键盘移动/虚拟化 |
| 大日志 | **VirtualList / Virtual Table** | 只渲染可见范围 |
| 文本 | **Editor / TextView** | Tree-sitter 高亮 + LSP；Markdown（含 HTML 混排）与基础 HTML 渲染 |
| 骨架 | **TitleBar / StatusBar / Resizable / Tabs / Tooltip / Pagination / Badge / Avatar / Icon / Theme** | 桌面窗口外壳与主题 |
| 聊天类 | **Bubble / MessageScroller / Attachment / Clipboard** | Octopus 有 AI 对话场景，可能用得上 |

来源：[组件目录](https://gpui-kit.com/llms.txt)、[Data Table](https://gpui-kit.com/component/data-table.md)、[Dock](https://gpui-kit.com/component/dock.md)、[Sidebar](https://gpui-kit.com/component/sidebar.md)、[Form](https://gpui-kit.com/component/form.md)、[Command](https://gpui-kit.com/component/command.md)、[Settings](https://gpui-kit.com/component/settings.md)。

### 5.1 Data Table 的核心用法

表格不是「传数据进去」，而是应用实现 `TableDelegate`，用 `TableState` 持有状态（[Data Table 文档](https://gpui-kit.com/component/data-table.md)）：

```rust
use gpui_kit::component::table::{
    DataTable, TableState, TableDelegate, Column, ColumnSort,
};

impl TableDelegate for MyTableDelegate {
    fn columns_count(&self, _: &App) -> usize { self.columns.len() }
    fn rows_count(&self, _: &App) -> usize { self.data.len() }
    fn column(&self, col_ix: usize, _: &App) -> Column { self.columns[col_ix].clone() }
    fn render_td(
        &mut self, row_ix: usize, col_ix: usize,
        _: &mut Window, _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement { /* 按列 key 渲染单元格 */ }
}

let state = cx.new(|cx| TableState::new(delegate, window, cx));
```

### 5.2 Sidebar 的核心用法

```rust
use gpui_kit::component::sidebar::{
    Sidebar, SidebarHeader, SidebarGroup, SidebarMenu, SidebarMenuItem,
};

Sidebar::new()
    .header(SidebarHeader::new().child("Octopus 管理面板"))
    .child(
        SidebarGroup::new("导航")
            .child(
                SidebarMenu::new()
                    .child(
                        SidebarMenuItem::new("故事书")
                            .icon(IconName::Book)
                            .on_click(|_, _, _| { /* ... */ }),
                    ),
            ),
    )
```

来源：[Sidebar 文档](https://gpui-kit.com/component/sidebar.md)。

## 六、给 Octopus 本地管理面板的架构建议

> 以下为我的建议，不是官方规定；官方硬性约束已在第 4、7 节标出。

### 6.1 项目定位与数据接入

面板要管理的东西，现有后端已经有对应接口（`crates/octopus-api/src` 与 `crates/octopus-bin/src/main.rs` 中的路由）：

- 故事书：`/api/storybooks/{id}/publish`、`/api/storybooks/{id}/sandbox`
- 存档：`/api/saves`、`/api/saves/{id}/...`（state / history / rounds / export / import / maintenance / rest / origin / save）
- 模型供应商：`/api/providers/probe`、`/api/providers/test`
- 配置与资产：`/api/config`、`/api/assets`、`/api/assets/{name}`
- 调试：`/api/lua/run`、`/api/validate`、`/api/health`

两种接入方式：

1. **HTTP 客户端（推荐）**：面板调用 `http://127.0.0.1:8787`，复用 `octopus-types` 的请求/响应类型。优点是面板与引擎进程解耦，引擎改内部不会连带编译面板，也天然支持「引擎在跑、面板连上去」的使用方式。异步请求用 GPUI 的 `cx.spawn` 加 reqwest。
2. **进程内直连数据库**：把 `octopus-engine` / `octopus-api` 作为库依赖，用 sea-orm 直接读 `octopus.db`。优点是离线也能用；缺点是绕过了引擎校验，容易造成两份写逻辑。除非要做「引擎未启动时的只读诊断」，否则不建议。

### 6.2 放置位置

现有 workspace（[根 Cargo.toml](Cargo.toml)）：

```text
[workspace]
resolver = "2"
members = ["crates/octopus-types", "crates/octopus-engine",
           "crates/octopus-ai", "crates/octopus-api",
           "crates/octopus-bin", "migration"]
edition = "2024"
```

建议新增成员 `crates/octopus-panel`，只依赖 `gpui-kit`、`octopus-types`、reqwest/tokio/serde 等。这样桌面面板和 `octopus-bin`（axum 服务）是两个可独立构建的二进制，互不影响。

注意：整个 workspace 共享 `Cargo.lock`。每个成员的依赖解析会合并，加入 gpui-kit 后首次 `cargo build` 会拉一大批依赖并编译 GPUI，耗时较长；建议按 4.3 配置 profile。

### 6.3 目录结构（按「能力」组织，而非全局 views/models）

官方 Coding Guides 明确要求：**大型应用按能力拆 crate，不要建全局 `views/`、`models/`、`modals/` 目录**。对一个内部面板，可以先单 crate 加按能力分 module，规模大了再拆：[Coding Guides](https://gpui-kit.com/docs/coding-guides.md)。

```text
crates/octopus-panel/
├── Cargo.toml
└── src/
    ├── main.rs              # bootstrap：init、window、Root
    ├── app.rs               # AppShell：Sidebar + Dock + 全局路由状态
    ├── api/                 # HTTP client，复用 octopus-types
    ├── features/
    │   ├── storybooks/      # 列表 / 详情 / 发布
    │   ├── saves/           # 存档列表 / 状态 / 历史 / 导出导入
    │   ├── providers/       # 供应商配置与连通性测试
    │   ├── config/          # 应用配置编辑
    │   ├── assets/          # 素材浏览
    │   └── logs/            # 日志/调试（VirtualList）
    └── shared/              # 复用的面板组件、主题扩展
```

### 6.4 关键实现要点

1. **窗口外壳**：`main.rs` 只做四件事——`gpui_kit::init(cx)`、建 `AppShell` entity、用 `Root::new` 包一层、开窗口。`Root` 每个窗口只有一个，不要每页一个。
2. **导航**：`Sidebar` 放左侧，点击项切换中间内容。若要「多标签工作区」，中间换成 `DockArea`（`DockSkin::dock_area(...)`），每个面板实现 `BasePanel`（身份与持久化）和 `Panel`（标题/工具栏）。布局可序列化，适合记住用户上次的面板排列。[Dock 文档](https://gpui-kit.com/component/dock.md)
3. **状态所有权**：领域状态放 feature 的 model/view；瞬时 UI 状态放渲染它的 view；可复用的行为状态交给组件的 State（如 `InputState`、`TableState`）。受控组件遵循「传入当前值 → 收到变更意图 → 改自己状态 → `cx.notify()`」。
4. **异步请求**：用 `cx.spawn(async move |cx| { ... })`；不要把 `&mut Window`/`&mut App`/`&mut Context` 跨 await 持有，保留 `Entity`/`WeakEntity`、`FocusHandle`、领域 ID 这类句柄。
5. **列表身份**：重复元素用领域派生的 `ElementId`，不要用下标（官方 Coding Guides 的硬性要求）。
6. **主题**：用 `cx.theme()` 的语义 token，不要写死颜色；间距用 `p_2()`/`gap_3()` 这类 rem-based helper，保证窗口缩放可用。[Design Guides](https://gpui-kit.com/docs/design-guides.md)
7. **大表**：Data Table 的 `render_td` 是热路径，只做取数和廉价渲染，避免每次重建整个 `Vec`。
8. **AI 辅助**：把 `https://gpui-kit.com/llms-full.txt` 作为上下文，或安装官方技能 `npx skills add longbridge/gpui-kit`。

### 6.5 MVP 建议顺序

1. `/api/health` 加 `/api/config`：先把「面板能连上引擎、能展示/改配置」跑通，验证 GPUI 骨架。
2. 故事书列表加存档列表：用 Data Table，验证大表性能。
3. 供应商配置加 `/api/providers/test`：表单与异步反馈（Notification/Spinner）。
4. 存档详情：state / history / rounds，用 `Tabs` 加 `Tree` 加 `TextView`。
5. 日志与调试：`VirtualList` 加 `/api/lua/run`。

## 七、官方设计/编码规范要点

官方把 Design Guides 与 Coding Guides 定位为**规范（normative）**，不是可选参考。两者都建议在动手前完整读一遍。摘取对本项目影响最大的：

**Design Guides（[原文](https://gpui-kit.com/docs/design-guides.md)）**

- 桌面优先于 Web 习惯：键盘可达、窗口边框/菜单、密集数据视图、可缩放区域、常驻导航。
- `Button` 用于所有应用内命令；`Link` 只用于外部 URL 和邮箱。
- 先 token 后数值：禁止在应用 UI 里写 raw hex/rgb。
- 状态必须可见：hover / focus / selected / disabled / loading / 校验 / 危险态各有明确且一致的呈现。
- 覆盖层：Escape 关闭最上层并把焦点还给触发者。
- 文案：说出对象和动词（Delete "Roadmap"? 加 Delete 按钮），不要「你确定吗？」加 OK。
- 稳定身份：重复元素用领域派生的 `ElementId`。

**Coding Guides（[原文](https://gpui-kit.com/docs/coding-guides.md)）**

- 按能力组织代码，feature crate 内聚 model/service/view/commands/dialog。
- `init` 一次、`Root` 每窗口一层。
- 区分 `Context<Self>` / `App` / `Window` 的职责；永远不要跨调用持有 `&mut` 引用。
- 值语义组件用 `RenderOnce`；跨帧行为用 `Entity<T>`；不要给每个视觉碎片都建 entity。
- `cx.notify()` 触发重绘；`cx.emit` 表达语义事件；`cx.subscribe`/`observe` 监听。

## 八、风险与限制

| 风险 | 说明 | 缓解 |
| --- | --- | --- |
| **0.x API 不稳定** | `gpui-kit` 只有 3 个版本，2026-09 才出；0.x 语义下可能 breaking | 锁 `=0.6.1`，升级时看 release note |
| **新 umbrella，生态资料少** | 网上文章、问答、第三方教程主要针对旧 `gpui-component` 或 GPUI 本身 | 以 `llms-full.txt`、源码和 `gpui-component` 文档为准 |
| **首次编译重** | 拉入 GPUI、文本栈、wgpu 等，编译时间长、磁盘占用大 | 配好 4.3 的 profile；CI 缓存 `target/` |
| **二进制体积** | 官方对比最小约 12 MB | 内部工具可接受 |
| **GPU/图形栈要求** | 依赖 Vulkan（含软件渲染 lvp 兜底）加 Wayland/X11 | 本机已满足；无 GPU 的服务器需用 lvp/软渲染，性能另说 |
| **Linux 缺 `libzstd-dev`** | 本机 `pkg-config libzstd` 缺失 | 构建前补装 |
| **与 Vue 前端的关系** | 这是原生桌面面板，不是替换现有 Vue 游玩/编辑器页 | 明确定位为「本地运维/调试面板」，避免重复实现业务 UI |
| **中文排版** | 官方对比明确 CJK Support: Yes，比 egui 好 | 无需担心中日韩字体 |

## 九、下一步建议

1. 先补系统依赖，然后跑通官方最小例子，确认图形栈没问题。
2. 跑 story gallery 和 `cargo run -p example-dock`，直观确认 Sidebar / Data Table / Dock / 图表的观感是否达到产品要求。
3. 确定数据接入方式（推荐 HTTP 加 `octopus-types`）。
4. 新建 `crates/octopus-panel`，按 6.5 的顺序做 MVP。
5. 把 `https://gpui-kit.com/llms.txt` 与两个官方技能接入日常开发流程。

## 十、资料来源

一手资料：

- 仓库与 README：[github.com/longbridge/gpui-kit](https://github.com/longbridge/gpui-kit)
- README 原文：[raw README.md](https://raw.githubusercontent.com/longbridge/gpui-kit/main/README.md)
- 架构文档：[docs/ARCHITECTURE.md](https://raw.githubusercontent.com/longbridge/gpui-kit/main/docs/ARCHITECTURE.md)
- 仓库 Cargo.toml：[raw Cargo.toml](https://raw.githubusercontent.com/longbridge/gpui-kit/main/Cargo.toml)
- umbrella crate 特性：[crates/kit/Cargo.toml](https://raw.githubusercontent.com/longbridge/gpui-kit/main/crates/kit/Cargo.toml)
- 官方文档站：[gpui-kit.com](https://gpui-kit.com)、[Getting Started](https://gpui-kit.com/docs/getting-started.md)、[Installation](https://gpui-kit.com/docs/installation)、[Design Guides](https://gpui-kit.com/docs/design-guides.md)、[Coding Guides](https://gpui-kit.com/docs/coding-guides.md)、[Comparison](https://gpui-kit.com/docs/comparison.md)
- 组件文档：[llms.txt 目录](https://gpui-kit.com/llms.txt)、[Data Table](https://gpui-kit.com/component/data-table.md)、[Dock](https://gpui-kit.com/component/dock.md)、[Sidebar](https://gpui-kit.com/component/sidebar.md)、[Form](https://gpui-kit.com/component/form.md)、[Command](https://gpui-kit.com/component/command.md)、[Settings](https://gpui-kit.com/component/settings.md)
- Shell 扩展：[shell/getting-started](https://gpui-kit.com/shell/getting-started.md)
- 可执行配方与验收：[examples/ai_recipes/README.md](https://raw.githubusercontent.com/longbridge/gpui-kit/main/examples/ai_recipes/README.md)
- Linux 依赖脚本：[script/install-linux.sh](https://raw.githubusercontent.com/longbridge/gpui-kit/main/script/install-linux.sh)
- 发布记录：[v0.6.1](https://github.com/longbridge/gpui-kit/releases/tag/v0.6.1)
- 包元数据：[crates.io gpui-kit](https://crates.io/crates/gpui-kit)、[crates.io gpui-component](https://crates.io/crates/gpui-component)、[docs.rs gpui-kit](https://docs.rs/gpui-kit/)

本仓库上下文：

- `AGENTS.md`、`CONTEXT.md`
- `Cargo.toml`（workspace 成员与依赖）
- `crates/octopus-api/src/`、`crates/octopus-bin/src/main.rs`（现有 HTTP 路由）
- `frontend/src/router.ts`（现有页面：list / editor / play）

> 标注为 UNVERIFIED 的点：`gpui-component` 到 `gpui-kit` 改名的确切时间与官方公告；`gpui-kit` 是否在稳定版 Rust 上完全无 nightly 依赖（官方文档写 1.90+ 即可，本机尚未实际编译验证）。
