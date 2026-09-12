//! Octopus 本地启动器：一个管理后端（axum）与前端（Vite）服务的原生桌面窗口。

mod services;

use std::process::Command;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::*;

use services::{
    open_url, package_manager, pkill, repo_root, spawn_logged, terminate, Phase, Service,
};

/// 本界面用到的主题颜色。渲染前一次性取出，避免在构建元素时反复借用上下文。
#[derive(Clone, Copy)]
struct Palette {
    background: Hsla,
    foreground: Hsla,
    muted: Hsla,
    muted_foreground: Hsla,
    border: Hsla,
    group_box: Hsla,
    primary: Hsla,
    primary_foreground: Hsla,
    success: Hsla,
    warning: Hsla,
    danger: Hsla,
}

struct Launcher {
    backend: Service,
    frontend: Service,
    repo: std::path::PathBuf,
    backend_port: u16,
    frontend_port: u16,
    message: String,
}

impl Launcher {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut me = Self {
            backend: Service::new("后端服务", 8787),
            frontend: Service::new("前端服务", 5173),
            repo: repo_root(),
            backend_port: 8787,
            frontend_port: 5173,
            message: String::new(),
        };
        me.refresh(cx);
        me.start_polling(cx);
        me
    }

    /// 每 1.5 秒刷新一次服务状态。
    fn start_polling(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        refresh_service(&mut self.backend);
        refresh_service(&mut self.frontend);
        cx.notify();
    }

    fn logs(&self) -> Vec<String> {
        let mut logs = self.backend.logs.tail(200);
        logs.extend(self.frontend.logs.tail(200));
        logs
    }

    fn start_backend(&mut self, cx: &mut Context<Self>) {
        if self.backend.child.is_some() || self.backend.port_open() {
            self.backend.logs.push("[launcher] 后端已在运行，忽略启动".into());
            cx.notify();
            return;
        }
        let mut cmd = Command::new("cargo");
        cmd.arg("run")
            .arg("-p")
            .arg("octopus-bin")
            .current_dir(&self.repo)
            .env("OCTOPUS_ADDR", format!("127.0.0.1:{}", self.backend_port))
            .env("OCTOPUS_DB", self.repo.join("octopus.db"))
            .env(
                "RUST_LOG",
                std::env::var("RUST_LOG").unwrap_or_else(|_| "info,sqlx=warn".to_string()),
            );
        self.backend
            .logs
            .push("[launcher] 启动后端：cargo run -p octopus-bin".into());
        match spawn_logged(cmd, self.backend.logs.clone(), "[后端]") {
            Ok(child) => {
                self.backend.child = Some(child);
                self.backend.phase = Phase::Starting;
                self.backend.note.clear();
            }
            Err(err) => {
                self.backend.phase = Phase::Stopped;
                self.backend.note = format!("启动失败：{err}");
            }
        }
        cx.notify();
    }

    fn start_frontend(&mut self, cx: &mut Context<Self>) {
        if self.frontend.child.is_some() || self.frontend.port_open() {
            self.frontend.logs.push("[launcher] 前端已在运行，忽略启动".into());
            cx.notify();
            return;
        }
        let Some(pm) = package_manager() else {
            self.frontend.note = "未找到 pnpm / npm / bun / yarn".into();
            cx.notify();
            return;
        };
        let mut cmd = Command::new(pm);
        cmd.arg("run").arg("dev").current_dir(self.repo.join("frontend"));
        self.frontend
            .logs
            .push(format!("[launcher] 启动前端：{pm} run dev"));
        match spawn_logged(cmd, self.frontend.logs.clone(), "[前端]") {
            Ok(child) => {
                self.frontend.child = Some(child);
                self.frontend.phase = Phase::Starting;
                self.frontend.note.clear();
            }
            Err(err) => {
                self.frontend.phase = Phase::Stopped;
                self.frontend.note = format!("启动失败：{err}");
            }
        }
        cx.notify();
    }

    fn stop_backend(&mut self, cx: &mut Context<Self>) {
        self.backend.logs.push("[launcher] 停止后端".into());
        if let Some(child) = self.backend.child.as_mut() {
            terminate(child);
        } else if self.backend.port_open() {
            pkill("octopus-bin");
        }
        self.backend.phase = Phase::Stopping;
        cx.notify();
    }

    fn stop_frontend(&mut self, cx: &mut Context<Self>) {
        self.frontend.logs.push("[launcher] 停止前端".into());
        if let Some(child) = self.frontend.child.as_mut() {
            terminate(child);
        } else if self.frontend.port_open() {
            let pattern = format!("{}/frontend", self.repo.display());
            pkill(&pattern);
        }
        self.frontend.phase = Phase::Stopping;
        cx.notify();
    }

    fn start_all(&mut self, cx: &mut Context<Self>) {
        self.start_backend(cx);
        self.start_frontend(cx);
    }

    fn stop_all(&mut self, cx: &mut Context<Self>) {
        self.stop_backend(cx);
        self.stop_frontend(cx);
    }

    fn open_web(&mut self, cx: &mut Context<Self>) {
        let url = format!("http://127.0.0.1:{}", self.frontend_port);
        open_url(&url);
        self.message = format!("已在浏览器打开 {url}");
        cx.notify();
    }

    fn clear_logs(&mut self, cx: &mut Context<Self>) {
        self.backend.logs.clear();
        self.frontend.logs.clear();
        cx.notify();
    }
}

/// 根据子进程是否存活、端口是否监听，推导服务阶段。
fn refresh_service(svc: &mut Service) {
    if let Some(child) = svc.child.as_mut() {
        match child.try_wait() {
            Ok(Some(status)) => {
                svc.child = None;
                svc.phase = if status.success() {
                    Phase::Stopped
                } else {
                    Phase::Crashed
                };
                svc.note = format!("进程已退出（{status}）");
            }
            _ => {
                if svc.phase != Phase::Stopping {
                    svc.phase = if svc.port_open() {
                        Phase::Running
                    } else {
                        Phase::Starting
                    };
                }
            }
        }
    } else if svc.port_open() {
        if svc.phase != Phase::Stopping && svc.phase != Phase::Running {
            svc.note = "由外部进程提供".into();
            svc.phase = Phase::Running;
        }
    } else {
        svc.phase = Phase::Stopped;
    }
}

/// 状态语义色与文案。颜色只表达状态含义，不作装饰。
fn status_style(phase: Phase, p: Palette) -> (Hsla, &'static str) {
    match phase {
        Phase::Running => (p.success, "运行中"),
        Phase::Starting => (p.warning, "启动中"),
        Phase::Stopping => (p.warning, "停止中"),
        Phase::Crashed => (p.danger, "已退出"),
        Phase::Stopped => (p.muted_foreground, "已停止"),
    }
}

/// 小号状态胶囊：圆点加文字，低透明度底色。
fn status_pill(color: Hsla, label: &str) -> impl IntoElement {
    h_flex()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded_full()
        .bg(color.opacity(0.14))
        .child(div().size_1().rounded_full().bg(color))
        .child(div().text_xs().text_color(color).child(label.to_string()))
}

/// 一个服务行：图标、名称与状态、详情、行尾的唯一主动作。
fn service_row(
    icon: IconName,
    name: &str,
    method: &str,
    phase: Phase,
    port: u16,
    note: &str,
    p: Palette,
    action: Button,
    with_top_border: bool,
) -> impl IntoElement {
    let (color, label) = status_style(phase, p);
    let detail = if note.is_empty() {
        format!("{method} · 127.0.0.1:{port}")
    } else {
        format!("{method} · 127.0.0.1:{port} · {note}")
    };

    let mut row = h_flex().w_full().items_center().gap_3().px_4().py_3();
    if with_top_border {
        row = row.border_t_1().border_color(p.border);
    }

    row.child(
        h_flex()
            .flex_shrink_0()
            .size_8()
            .justify_center()
            .items_center()
            .rounded_md()
            .bg(p.muted)
            .child(Icon::new(icon).text_color(p.muted_foreground)),
    )
    .child(
        v_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_sm().child(name.to_string()))
                    .child(status_pill(color, label)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .truncate()
                    .child(detail),
            ),
    )
    .child(div().flex_shrink_0().child(action))
}

fn log_panel(logs: Vec<String>, p: Palette, mono_font: SharedString) -> impl IntoElement {
    let panel = v_flex()
        .id("launcher-logs")
        .w_full()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .border_1()
        .border_color(p.border)
        .rounded_md()
        .bg(p.group_box)
        .font_family(mono_font)
        .p_3()
        .gap_1();

    if logs.is_empty() {
        panel.items_center().justify_center().child(
            div()
                .text_xs()
                .text_color(p.muted_foreground)
                .child("暂无日志，启动服务后会显示输出"),
        )
    } else {
        panel.children(
            logs.into_iter()
                .map(|line| div().text_xs().text_color(p.muted_foreground).child(line)),
        )
    }
}

impl Render for Launcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette {
            background: cx.theme().background,
            foreground: cx.theme().foreground,
            muted: cx.theme().muted,
            muted_foreground: cx.theme().muted_foreground,
            border: cx.theme().border,
            group_box: cx.theme().group_box,
            primary: cx.theme().primary,
            primary_foreground: cx.theme().primary_foreground,
            success: cx.theme().success,
            warning: cx.theme().warning,
            danger: cx.theme().danger,
        };

        let backend_action = match self.backend.phase {
            Phase::Stopped | Phase::Crashed => Button::new("start-backend")
                .icon(IconName::Play)
                .label("启动")
                .on_click(cx.listener(|this, _, _, cx| this.start_backend(cx))),
            Phase::Starting => Button::new("backend-starting").loading(true).label("启动中"),
            Phase::Stopping => Button::new("backend-stopping").loading(true).label("停止中"),
            Phase::Running => Button::new("stop-backend")
                .outline()
                .icon(IconName::Pause)
                .label("停止")
                .on_click(cx.listener(|this, _, _, cx| this.stop_backend(cx))),
        };

        let frontend_action = match self.frontend.phase {
            Phase::Stopped | Phase::Crashed => Button::new("start-frontend")
                .icon(IconName::Play)
                .label("启动")
                .on_click(cx.listener(|this, _, _, cx| this.start_frontend(cx))),
            Phase::Starting => Button::new("frontend-starting").loading(true).label("启动中"),
            Phase::Stopping => Button::new("frontend-stopping").loading(true).label("停止中"),
            Phase::Running => Button::new("stop-frontend")
                .outline()
                .icon(IconName::Pause)
                .label("停止")
                .on_click(cx.listener(|this, _, _, cx| this.stop_frontend(cx))),
        };

        let start_all = Button::new("start-all")
            .primary()
            .icon(IconName::Play)
            .label("全部启动")
            .on_click(cx.listener(|this, _, _, cx| this.start_all(cx)));
        let stop_all = Button::new("stop-all")
            .outline()
            .icon(IconName::Pause)
            .label("全部停止")
            .on_click(cx.listener(|this, _, _, cx| this.stop_all(cx)));
        let open_web = Button::new("open-web")
            .ghost()
            .icon(IconName::ExternalLink)
            .label("打开网页")
            .on_click(cx.listener(|this, _, _, cx| this.open_web(cx)));
        let clear_logs = Button::new("clear-logs")
            .ghost()
            .small()
            .label("清空")
            .on_click(cx.listener(|this, _, _, cx| this.clear_logs(cx)));

        let phases = [self.backend.phase, self.frontend.phase];
        let up = phases
            .iter()
            .filter(|phase| matches!(phase, Phase::Running | Phase::Starting))
            .count();
        let (overall_color, overall_label) = if up == phases.len() {
            (p.success, format!("{up} / {} 运行中", phases.len()))
        } else if up == 0 {
            (p.muted_foreground, format!("0 / {} 运行中", phases.len()))
        } else {
            (p.warning, format!("{up} / {} 运行中", phases.len()))
        };

        let logs = self.logs();
        let footer_note = if self.message.is_empty() {
            self.repo.display().to_string()
        } else {
            self.message.clone()
        };

        v_flex()
            .size_full()
            .bg(p.background)
            .text_color(p.foreground)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(p.border)
                    .child(
                        div()
                            .flex_shrink_0()
                            .size_8()
                            .rounded_md()
                            .bg(p.primary)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::LayoutDashboard).text_color(p.primary_foreground),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_base().child("Octopus 启动器"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .child("本地服务控制台"),
                            ),
                    )
                    .child(div().flex_1())
                    .child(status_pill(overall_color, &overall_label)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .px_4()
                    .py_4()
                    .gap_4()
                    .child(
                        v_flex()
                            .w_full()
                            .border_1()
                            .border_color(p.border)
                            .rounded_md()
                            .overflow_hidden()
                            .child(service_row(
                                IconName::SquareTerminal,
                                self.backend.name,
                                "axum · cargo run -p octopus-bin",
                                self.backend.phase,
                                self.backend.port,
                                &self.backend.note.clone(),
                                p,
                                backend_action,
                                false,
                            ))
                            .child(service_row(
                                IconName::Globe,
                                self.frontend.name,
                                "Vite · pnpm run dev",
                                self.frontend.phase,
                                self.frontend.port,
                                &self.frontend.note.clone(),
                                p,
                                frontend_action,
                                true,
                            )),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .gap_2()
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(p.muted_foreground)
                                            .child("运行日志"),
                                    )
                                    .child(clear_logs),
                            )
                            .child(log_panel(logs, p, cx.theme().mono_font_family.clone())),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(p.border)
                    .child(start_all)
                    .child(stop_all)
                    .child(open_web)
                    .child(div().flex_1())
                    .child(
                        div()
                            .min_w_0()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .truncate()
                            .child(footer_note),
                    ),
            )
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);

            cx.spawn(async move |cx| {
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(560.), px(640.)),
                    })),
                    window_min_size: Some(size(px(460.), px(520.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Octopus 启动器".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                };

                cx.open_window(options, |window, cx| {
                    let view = cx.new(|cx| Launcher::new(cx));
                    cx.new(|cx| Root::new(view, window, cx))
                })
                .expect("Failed to open window");
            })
            .detach();
        });
}
