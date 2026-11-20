//! Control by MCP: an agent drives the editor.
//!
//! A Model Context Protocol server (the official Rust SDK, `rmcp`) runs on
//! its own thread with its own tokio runtime, on `127.0.0.1` only, and only
//! when switched on (`--mcp`, or the checkbox in the panel). It does not
//! touch the document: each tool call becomes a [`Request`] sent to the UI
//! thread, which runs it between frames exactly as if it had been typed in
//! the console -- same interpreter, same undo history, shown in the log --
//! and answers on a oneshot channel.
//!
//! Requests from a web page are refused: `rmcp` accepts only loopback
//! `Host` headers (against DNS rebinding), and Origin validation is on with
//! an empty allowlist, so any request a browser marks with an `Origin` is
//! rejected. Writing over an existing file needs the user's yes in the
//! window (see `App::mcp_poll`).
//!
//! `rmcp` speaks every protocol version from 2024-11-05 to the stateless
//! 2026-07-28 one (no session, the version on each request). The tools carry
//! annotations (which only read, which may destroy) so a client can approve
//! the harmless ones by itself; `describe_scene` answers with structured
//! content and its schema; a render reports its samples as progress, and a
//! client that cancels a call drops it from the editor too.

use eframe::egui;
use rmcp::{
    ErrorData, Peer, RoleServer, ServerHandler,
    handler::server::{common::schema_for_output, router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, ContentBlock, Implementation, ProgressNotificationParam, ProgressToken, RequestMetaObject,
        ServerCapabilities, ServerConfig,
    },
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::{Deserialize, Serialize};
use std::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The port the server listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 47800;

/// How long a tool call waits for the editor. Long enough for the user to
/// answer a confirmation, or for a render of many samples.
const REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// What an agent asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// A console line.
    Command(String),
    /// Several console lines as one undo step, stopping at the first error.
    Batch(Vec<String>),
    Describe,
    /// Every palette colour, with its material when not plain diffuse.
    Palette,
    /// The current image: the editor view, or the path tracer once it has
    /// `samples`; scaled to fit `max_size` pixels.
    Capture { render: bool, samples: u32, max_size: u32 },
}

pub enum Reply {
    Text(String),
    Error(String),
    Png(Vec<u8>),
    Scene(Box<SceneDescription>),
}

pub struct Request {
    pub op: Op,
    /// Closed when the client cancelled the call or gave up waiting: the
    /// editor then drops what it was keeping for it.
    pub reply: tokio::sync::oneshot::Sender<Reply>,
    /// How far a long request has got (a render's samples so far), for the
    /// client's progress notifications.
    pub progress: tokio::sync::watch::Sender<u32>,
}

/// The open document, as `describe_scene` returns it.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SceneDescription {
    /// The document's name.
    pub file: String,
    /// Where it was loaded from or last saved; null for a new model.
    pub path: Option<String>,
    pub unsaved_changes: bool,
    pub models: Vec<ModelInfo>,
    /// Models placed in the scene: one model can be placed many times.
    pub instances: Vec<InstanceInfo>,
    /// Index into `instances`.
    pub selected_instance: usize,
    /// Index into `models` of the model edits go to.
    pub selected_model: Option<usize>,
    pub layers: Vec<LayerInfo>,
    pub colours_in_use: Vec<ColourUse>,
    /// The palette index painting uses.
    pub brush_colour: u8,
    pub animation_frame: u32,
    /// Whether the editor shows the path tracer.
    pub rendering: bool,
    pub undo_steps: usize,
    /// How to read the coordinates.
    pub axes: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ModelInfo {
    pub index: usize,
    /// Cells along x, y, z.
    pub size: [u32; 3],
    /// Filled cells.
    pub voxels: u64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct InstanceInfo {
    pub index: usize,
    /// Index into `models`.
    pub model: usize,
    /// World position of the model's centre, in voxels.
    pub position: [i32; 3],
    pub layer: i32,
    pub hidden: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct LayerInfo {
    pub id: i32,
    pub name: Option<String>,
    pub hidden: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ColourUse {
    /// Palette index 1-255.
    pub index: u8,
    /// `#rrggbb`.
    pub hex: String,
    /// Voxels of this colour across all models.
    pub voxels: u64,
}

/// The running server. Dropping it stops it.
pub struct Server {
    pub port: u16,
    pub requests: mpsc::Receiver<Request>,
    cancel: CancellationToken,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Server {
    /// Binds `127.0.0.1:port` and serves `/mcp` on a new thread. `ctx`
    /// wakes the UI when a request arrives: egui only redraws on input.
    pub fn start(port: u16, ctx: egui::Context) -> std::io::Result<Self> {
        // Bound here, so a port in use is reported to the caller.
        let listener = std::net::TcpListener::bind(("127.0.0.1", port))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let (sender, requests) = mpsc::channel();
        let cancel = CancellationToken::new();

        let stop = cancel.clone();
        std::thread::Builder::new().name("mcp".to_owned()).spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[voxferro] mcp: no runtime: {e}");
                    return;
                }
            };
            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::from_std(listener) {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("[voxferro] mcp: {e}");
                        return;
                    }
                };
                let config = StreamableHttpServerConfig::default()
                    .enforce_origin_validation()
                    .with_cancellation_token(stop.clone());
                let service: StreamableHttpService<Editor, LocalSessionManager> = StreamableHttpService::new(
                    move || Ok(Editor::new(sender.clone(), ctx.clone())),
                    Default::default(),
                    config,
                );
                let router = axum::Router::new().nest_service("/mcp", service);
                let _ = axum::serve(listener, router)
                    .with_graceful_shutdown(async move { stop.cancelled_owned().await })
                    .await;
            });
        })?;
        Ok(Self { port, requests, cancel })
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }
}

/// The MCP side: one per session, all sending into the same channel.
#[derive(Clone)]
struct Editor {
    sender: mpsc::Sender<Request>,
    ctx: egui::Context,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CommandArgs {
    /// One console line, e.g. `box 0 0 0 7 7 7 12` or `camera iso`. `help`
    /// lists the commands.
    command: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct BatchArgs {
    /// Console lines, run in order as one undo step. Running stops at the
    /// first line that fails; the result says which. Lines that write files
    /// are not accepted here: use save_file.
    commands: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct VoxelArgs {
    /// Voxels as [x, y, z, colour] in the selected model's own cells,
    /// z up; colour is a palette index 1-255, or 0 to erase.
    voxels: Vec<[i32; 4]>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct PathArgs {
    /// A path on this computer.
    path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SaveArgs {
    /// Where to save; omitted, the file's own path. Writing over an
    /// existing file waits for the user to allow it in the editor.
    path: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CaptureArgs {
    /// Path traced render instead of the editor view.
    #[serde(default)]
    render: bool,
    /// Samples to accumulate for a render (default 64).
    samples: Option<u32>,
    /// Largest side of the returned image in pixels (default 1024).
    max_size: Option<u32>,
}

#[tool_router]
impl Editor {
    fn new(sender: mpsc::Sender<Request>, ctx: egui::Context) -> Self {
        Self { sender, ctx, tool_router: Self::tool_router() }
    }

    /// Sends `op` to the editor and waits for its answer, the client's
    /// cancellation or the timeout. With `progress`, every change the editor
    /// reports goes to the client as a progress notification out of `total`.
    async fn ask(&self, op: Op, ct: CancellationToken, progress: Option<(Peer<RoleServer>, ProgressToken, u32)>) -> CallToolResult {
        let (reply, mut answer) = tokio::sync::oneshot::channel();
        let (progress_sender, mut done) = tokio::sync::watch::channel(0u32);
        if self.sender.send(Request { op, reply, progress: progress_sender }).is_err() {
            return error("the editor has closed");
        }
        self.ctx.request_repaint();
        let deadline = tokio::time::sleep(REPLY_TIMEOUT);
        tokio::pin!(deadline);
        let mut watching = progress.is_some();
        // Dropping `answer` on the way out (cancelled, timed out) closes the
        // editor's sender, which is how it learns to stop waiting too.
        loop {
            tokio::select! {
                result = &mut answer => return match result {
                    Ok(Reply::Text(t)) => CallToolResult::success(vec![ContentBlock::text(t)]),
                    Ok(Reply::Error(e)) => error(e),
                    Ok(Reply::Png(png)) => CallToolResult::success(vec![ContentBlock::image(base64(&png), "image/png")]),
                    Ok(Reply::Scene(scene)) => match serde_json::to_value(scene) {
                        Ok(value) => CallToolResult::structured(value),
                        Err(e) => error(e.to_string()),
                    },
                    Err(_) => error("the editor dropped the request"),
                },
                _ = ct.cancelled() => {
                    // Wake the editor so it sees the request is gone.
                    drop(answer);
                    self.ctx.request_repaint();
                    return error("cancelled");
                }
                _ = &mut deadline => {
                    drop(answer);
                    self.ctx.request_repaint();
                    return error("the editor did not answer in time");
                }
                changed = done.changed(), if watching => {
                    let Ok(()) = changed else {
                        watching = false;
                        continue;
                    };
                    let so_far = *done.borrow_and_update();
                    if let Some((peer, token, total)) = &progress {
                        let note = ProgressNotificationParam::new(token.clone(), so_far as f64)
                            .with_total(*total as f64)
                            .with_message(format!("{so_far} of {total} samples"));
                        let _ = peer.notify_progress(note).await;
                    }
                }
            }
        }
    }

    #[tool(
        title = "Run a console command",
        description = "Run one line of the editor's console: the same commands its buttons use. \
            Edits go into the undo history. Send `help` for the list. Returns the console's output.",
        annotations(destructive_hint = true, open_world_hint = false)
    )]
    async fn run_command(&self, Parameters(a): Parameters<CommandArgs>, ct: CancellationToken) -> Result<CallToolResult, ErrorData> {
        Ok(self.ask(Op::Command(a.command), ct, None).await)
    }

    #[tool(
        title = "Run console commands",
        description = "Run several console lines in order as one undo step: much faster than \
            one run_command per line. Stops at the first line that fails. Returns each line's output.",
        annotations(destructive_hint = true, open_world_hint = false)
    )]
    async fn run_commands(&self, Parameters(a): Parameters<BatchArgs>, ct: CancellationToken) -> Result<CallToolResult, ErrorData> {
        if a.commands.is_empty() {
            return Ok(error("no commands given"));
        }
        Ok(self.ask(Op::Batch(a.commands), ct, None).await)
    }

    #[tool(
        title = "List the palette",
        description = "List the palette: each index 1-255 with its colour as hex, and its \
            material when it is not plain diffuse. Use it to pick colours before building.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_palette(&self, ct: CancellationToken) -> Result<CallToolResult, ErrorData> {
        Ok(self.ask(Op::Palette, ct, None).await)
    }

    #[tool(
        title = "Describe the scene",
        description = "Describe the open document: file, models with their sizes and voxel \
            counts, placed instances, layers, the colours in use, the selected model, the \
            animation frame and whether there are unsaved changes.",
        output_schema = schema_for_output::<SceneDescription>(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn describe_scene(&self, ct: CancellationToken) -> Result<CallToolResult, ErrorData> {
        Ok(self.ask(Op::Describe, ct, None).await)
    }

    #[tool(
        title = "Set voxels",
        description = "Set voxels of the selected model, as one undo step. Coordinates are \
            the model's own cells, z up; colour 0 erases.",
        annotations(destructive_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn set_voxels(&self, Parameters(a): Parameters<VoxelArgs>, ct: CancellationToken) -> Result<CallToolResult, ErrorData> {
        if a.voxels.is_empty() {
            return Ok(error("no voxels given"));
        }
        let mut line = String::from("voxels");
        for v in &a.voxels {
            line.push_str(&format!(" {} {} {} {}", v[0], v[1], v[2], v[3]));
        }
        Ok(self.ask(Op::Command(line), ct, None).await)
    }

    #[tool(
        title = "Open a file",
        description = "Open a .vox file. With unsaved changes the editor asks the user first.",
        annotations(destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn open_file(&self, Parameters(a): Parameters<PathArgs>, ct: CancellationToken) -> Result<CallToolResult, ErrorData> {
        Ok(self.ask(Op::Command(format!("open {}", a.path)), ct, None).await)
    }

    #[tool(
        title = "Save the document",
        description = "Save the document as .vox. Writing over an existing file (including the \
            file's own path) waits for the user to allow it in the editor.",
        annotations(destructive_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn save_file(&self, Parameters(a): Parameters<SaveArgs>, ct: CancellationToken) -> Result<CallToolResult, ErrorData> {
        let line = match a.path {
            Some(p) => format!("save {p}"),
            None => "save".to_owned(),
        };
        Ok(self.ask(Op::Command(line), ct, None).await)
    }

    #[tool(
        title = "Capture the view",
        description = "Capture what the editor shows as a PNG: the editor view, or with \
            render=true the path traced image after the given number of samples. A render \
            reports its samples as progress when the client asks for it.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn capture_view(
        &self,
        Parameters(a): Parameters<CaptureArgs>,
        ct: CancellationToken,
        meta: RequestMetaObject,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let samples = a.samples.unwrap_or(64).clamp(1, 4096);
        let op = Op::Capture { render: a.render, samples, max_size: a.max_size.unwrap_or(1024).clamp(64, 4096) };
        let progress = match meta.get_progress_token() {
            Some(token) if a.render => Some((peer, token, samples)),
            _ => None,
        };
        Ok(self.ask(op, ct, progress).await)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Editor {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("voxferro", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Voxferro, a voxel editor compatible with the .vox format. \
                 Start with describe_scene and get_palette; send `help` to run_command for the \
                 command list. Build with run_commands (many lines, one call, one undo step) or \
                 set_voxels; look at the result with capture_view. Everything can be undone \
                 with the `undo` command.",
            )
    }
}

fn error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

/// Standard base64 with padding, for image content.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Where a console line would write a file, for the commands that do.
/// `current` is the open file's own path, which a bare `save` writes.
pub fn write_target(line: &str, current: Option<&str>) -> Option<Result<std::path::PathBuf, String>> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let path = |p: &str| std::path::PathBuf::from(p);
    match words.as_slice() {
        ["save"] => Some(current.map(path).ok_or_else(|| "this model has never been saved: give a path".to_owned())),
        ["save", p, ..] => Some(Ok(path(p))),
        ["export"] => Some(Err("export needs a path".to_owned())),
        ["export", "materials" | "render" | "cameras", p, ..] => Some(Ok(path(p))),
        ["export", f, p, ..] if crate::exchange::FORMATS.iter().any(|(n, ..)| n == f) => Some(Ok(if *f == "obj" { path(p).with_extension("obj") } else { path(p) })),
        ["export", f] if crate::exchange::FORMATS.iter().any(|(n, ..)| n == f) => Some(Err(format!("export {f} needs a path"))),
        ["export", p, ..] if p.to_ascii_lowercase().ends_with(".ply") || p.ends_with(".xraw") || p.ends_with(".qb") => Some(Ok(path(p))),
        ["export", p, ..] => Some(Ok(path(p).with_extension("obj"))),
        ["anim", "export", dir, ..] => Some(Ok(path(dir))),
        ["render", "save", p, ..] => Some(Ok(path(p))),
        ["render", "image", _, _, _, p, ..] => Some(Ok(path(p))),
        ["palette", "save", p, ..] => Some(Ok(path(p))),
        ["snap", "window", p, ..] => Some(Ok(path(p))),
        ["snap", p, ..] if *p != "window" => Some(Ok(path(p))),
        ["project", "rename", name @ ..] => Some(match current {
            Some(c) => crate::project::renamed(std::path::Path::new(c), &name.join(" ")),
            None => Err("this model has never been saved".to_owned()),
        }),
        ["project", "dup"] => Some(current.map(|c| crate::project::next_free(std::path::Path::new(c))).ok_or_else(|| "this model has never been saved".to_owned())),
        ["project", "delete"] => Some(current.map(path).ok_or_else(|| "this model has no file".to_owned())),
        _ => None,
    }
}

/// Lines that would open a native file panel when run without a path:
/// the panel would block the editor waiting for someone at the screen.
pub fn needs_path(line: &str) -> bool {
    match line.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["open"] | ["import"] | ["export"] | ["snap"] | ["snap", "window"] => true,
        ["export", f] => crate::exchange::FORMATS.iter().any(|(n, ..)| n == f),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn writing_commands_are_found() {
        assert_eq!(write_target("save", Some("/a.vox")), Some(Ok("/a.vox".into())));
        assert!(matches!(write_target("save", None), Some(Err(_))));
        assert_eq!(write_target("save b.vox", None), Some(Ok("b.vox".into())));
        assert_eq!(write_target("export m", None), Some(Ok("m.obj".into())));
        assert_eq!(write_target("export qb m.qb", None), Some(Ok("m.qb".into())));
        assert_eq!(write_target("export m.ply", None), Some(Ok("m.ply".into())));
        assert!(matches!(write_target("export slab", None), Some(Err(_))));
        assert_eq!(write_target("anim export out 8", None), Some(Ok("out".into())));
        assert_eq!(write_target("render save r.png", None), Some(Ok("r.png".into())));
        assert_eq!(write_target("palette save p.png", None), Some(Ok("p.png".into())));
        assert_eq!(write_target("snap s.png", None), Some(Ok("s.png".into())));
        assert_eq!(write_target("project rename fort", Some("/a/castle.vox")), Some(Ok("/a/fort.vox".into())));
        assert!(matches!(write_target("project dup", None), Some(Err(_))));
        assert_eq!(write_target("box 0 0 0 1 1 1 2", None), None);
        assert!(needs_path("open"));
        assert!(!needs_path("open a.vox"));
        assert!(needs_path("import") && needs_path("export ply") && !needs_path("export ply a.ply"));
    }
}
