//! 进程与服务管理：启动、停止、状态探测。
//!
//! 这里不依赖 GPUI，便于单独推理与测试。

use std::io::{BufRead, BufReader, Read};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

/// 服务所处的阶段。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Stopped,
    Starting,
    Running,
    Stopping,
    Crashed,
}

/// 有上限的日志环形缓冲，供 UI 显示子进程输出。
#[derive(Clone, Default)]
pub struct LogBuffer(Arc<Mutex<Vec<String>>>);

impl LogBuffer {
    pub fn push(&self, line: String) {
        let mut v = self.0.lock().unwrap();
        v.push(line);
        let len = v.len();
        if len > 1000 {
            v.drain(0..len - 1000);
        }
    }

    pub fn tail(&self, n: usize) -> Vec<String> {
        let v = self.0.lock().unwrap();
        let start = v.len().saturating_sub(n);
        v[start..].to_vec()
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

/// 一个被管理的服务。
pub struct Service {
    pub name: &'static str,
    pub port: u16,
    pub phase: Phase,
    pub child: Option<Child>,
    pub logs: LogBuffer,
    pub note: String,
}

impl Service {
    pub fn new(name: &'static str, port: u16) -> Self {
        Self {
            name,
            port,
            phase: Phase::Stopped,
            child: None,
            logs: LogBuffer::default(),
            note: String::new(),
        }
    }

    pub fn port_open(&self) -> bool {
        is_port_open(self.port)
    }
}

/// 本机端口是否已被监听。
pub fn is_port_open(port: u16) -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

/// 通过编译期路径反推仓库根目录（本 crate 位于 <root>/crates/octopus-launcher）。
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 选择可用的前端包管理器，优先 pnpm。
pub fn package_manager() -> Option<&'static str> {
    for candidate in ["pnpm", "npm", "bun", "yarn"] {
        let ok = Command::new(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Some(candidate);
        }
    }
    None
}

/// 启动子进程并把 stdout/stderr 行读进日志缓冲；子进程独立成进程组，便于整组终止。
pub fn spawn_logged(mut cmd: Command, buf: LogBuffer, tag: &'static str) -> std::io::Result<Child> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    if let Some(out) = child.stdout.take() {
        pipe_reader(out, buf.clone(), tag);
    }
    if let Some(err) = child.stderr.take() {
        pipe_reader(err, buf, tag);
    }
    Ok(child)
}

fn pipe_reader<R: Read + Send + 'static>(reader: R, buf: LogBuffer, tag: &'static str) {
    std::thread::spawn(move || {
        let reader = BufReader::new(reader);
        for line in reader.lines().map_while(Result::ok) {
            buf.push(format!("{tag} {line}"));
        }
    });
}

/// 终止子进程所在的整个进程组（SIGTERM）。
pub fn terminate(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        let pid = child.id() as i32;
        // 负数 pid 表示进程组；spawn 时用了 process_group(0)，故 pgid == pid。
        libc::kill(-pid, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

/// 兜底清理：终止由外部启动、我们拿不到句柄的进程。
pub fn pkill(pattern: &str) {
    let _ = Command::new("pkill")
        .arg("-f")
        .arg(pattern)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// 用系统默认浏览器打开 URL。
pub fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(target_os = "windows")]
    let opener = "explorer";
    #[cfg(all(unix, not(target_os = "macos")))]
    let opener = "xdg-open";

    let _ = Command::new(opener)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
