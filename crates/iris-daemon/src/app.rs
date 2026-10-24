//! IPC server, wallpaper slots, and the `wl_surface.frame` loop.
//!
//! The calloop thread owns every slot. A slot's renderer is the first field so
//! the GPU surface drops before the layer-shell role.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction};
use iris_backend_wayland::{BackendEvent, LayerGeometry, OutputSnapshot, Session, WaylandPump};
use iris_core::ipc::IpcErrorBody;
use iris_core::{
    FrameDecoder, IpcCall, IpcRequest, IpcResponse, MonitorInfo, MonitorTarget, RequestId,
    ResponseBody, RunState, ScaleMode, WallpaperConfig, WallpaperStatus, PROTOCOL_VERSION,
};
use iris_pkg::DecodedTexture;
use iris_render::{
    DrawParams, DrawSpan, Fit, RenderContext, SceneDraw, WallpaperSurface, YuvFrame, YuvMatrix,
    YuvPlane,
};
use iris_scene::{ScenePackage, SceneRuntime};
use iris_video::{DecodedVideoFrame, Matrix, Picture, PlaybackOptions, VideoPlayer};

use crate::error::Error;
use crate::load::{load_source, load_wallpaper, prepare_scene, Loaded};

const DRAIN_CAP: usize = 64;
const WRITE_RETRY: Duration = Duration::from_millis(5);

struct Slot {
    renderer: WallpaperSurface,
    layer_id: u64,
    index: u32,
    connector: String,
    primary: bool,
    config: WallpaperConfig,
    asset: Asset,
    clock: MediaClock,
    fit: Fit,
    state: RunState,
    message: Option<String>,
    user_paused: bool,
    auto_paused: bool,
    last_ms: Option<u32>,
    elapsed: f32,
    last_present: Option<Instant>,
}

enum Asset {
    Still(std::sync::Arc<DecodedTexture>),
    Video(VideoPlayer),
    Scene(SceneAsset),
}

struct SceneAsset {
    package: std::sync::Arc<ScenePackage>,
    runtime: SceneRuntime,
}

struct MediaClock {
    origin: Instant,
    paused_at: Option<Instant>,
    paused_for: Duration,
    rate: f32,
}

impl MediaClock {
    fn new(rate: f32) -> Self {
        Self {
            origin: Instant::now(),
            paused_at: None,
            paused_for: Duration::ZERO,
            rate,
        }
    }

    fn pause(&mut self) {
        if self.paused_at.is_none() {
            self.paused_at = Some(Instant::now());
        }
    }

    fn resume(&mut self) {
        if let Some(at) = self.paused_at.take() {
            self.paused_for += at.elapsed();
        }
    }

    fn seconds(&self) -> f64 {
        let end = self.paused_at.unwrap_or_else(Instant::now);
        let elapsed = end
            .saturating_duration_since(self.origin)
            .saturating_sub(self.paused_for);
        elapsed.as_secs_f64() * f64::from(self.rate)
    }
}

#[derive(Clone)]
struct SharedAsset {
    config: WallpaperConfig,
    still: Option<std::sync::Arc<DecodedTexture>>,
    scene: Option<std::sync::Arc<ScenePackage>>,
    fit: Fit,
}

struct ClientSlot {
    stream: UnixStream,
    decoder: FrameDecoder,
    outgoing: Vec<u8>,
}

struct App {
    slots: HashMap<u32, Slot>,
    gpu: Option<RenderContext>,
    broadcast: Option<SharedAsset>,
    session: Option<Rc<RefCell<Session>>>,
    wayland_error: Option<String>,
    clients: HashMap<u64, ClientSlot>,
    next_client: u64,
    write_armed: bool,
    running: bool,
    shutdown_after_flush: bool,
    pointer: [f32; 2],
    socket_path: Option<PathBuf>,
    handle: LoopHandle<'static, App>,
    signal: LoopSignal,
    telemetry: iris_telemetry::Hub,
    pause_on_fullscreen: bool,
    pause_on_game: bool,
    webs: HashMap<u32, crate::loader::WebSlot>,
    web_broadcast: Option<WallpaperConfig>,
    web_timer: bool,
}

struct WebMonitor {
    id: u32,
    index: u32,
    connector: String,
    primary: bool,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

enum Opened {
    First(RenderContext, WallpaperSurface),
    Surface(WallpaperSurface),
}

enum ReadStep {
    Bytes(usize),
    Blocked,
    Closed,
}

enum WriteStep {
    Done,
    Pending,
    Again,
    Drop,
}

enum FrameDecision {
    Skip,
    Failed,
    Draw {
        delta: f32,
        elapsed: f32,
        fit: Fit,
        image_width: u32,
        image_height: u32,
    },
    Scene {
        delta: f32,
        elapsed: f32,
        pointer: [f32; 2],
        viewport: [f32; 2],
        tick: iris_scene::Tick,
    },
}

/// Seconds since the previous presented frame, scaled by `rate`.
///
/// A gap above one second, or a missing previous stamp, uses one frame at 60 Hz.
pub(crate) fn frame_delta(prev: Option<u32>, now: u32, rate: f32) -> f32 {
    let seconds = match prev {
        Some(prev) => {
            let elapsed_ms = now.wrapping_sub(prev);
            if elapsed_ms > 1_000 {
                1.0 / 60.0
            } else {
                elapsed_ms as f32 / 1_000.0
            }
        }
        None => 1.0 / 60.0,
    };
    seconds * rate
}

fn skip_for_cap(last: Option<Instant>, cap: Option<u32>) -> bool {
    crate::policy::under_cap(last, cap)
}

pub(crate) fn serve(config: iris_core::DaemonConfig) -> Result<(), Error> {
    let pause_on_fullscreen = config.pause_on_fullscreen;
    let pause_on_game = config.pause_on_game;
    let endpoint = config.endpoint;
    let path = match endpoint {
        iris_core::IpcEndpoint::Unix { path } => path,
        iris_core::IpcEndpoint::NamedPipe { name } => {
            return Err(Error::Unsupported(format!(
                "named pipe is not supported: {name}"
            )));
        }
    };
    let listener = bind_listener(&path)?;
    let mut event_loop: EventLoop<'static, App> =
        EventLoop::try_new().map_err(|err| Error::Loop(err.to_string()))?;
    let handle = event_loop.handle();
    let signal = event_loop.get_signal();
    let mut app = App {
        slots: HashMap::new(),
        gpu: None,
        broadcast: None,
        session: None,
        wayland_error: None,
        clients: HashMap::new(),
        next_client: 1,
        write_armed: false,
        running: true,
        shutdown_after_flush: false,
        pointer: [0.0, 0.0],
        socket_path: None,
        handle,
        signal,
        telemetry: iris_telemetry::Hub::start(iris_telemetry::DEFAULT_INTERVAL),
        pause_on_fullscreen,
        pause_on_game,
        webs: HashMap::new(),
        web_broadcast: None,
        web_timer: false,
    };
    app.arm_policy_timer();
    let source = Generic::new(listener, Interest::READ, Mode::Level);
    if let Err(err) = app
        .handle
        .insert_source(source, |readiness, listener, app| {
            if readiness.error {
                tracing::error!("ipc listener failed");
                return Ok(PostAction::Remove);
            }
            app.accept_clients(&**listener);
            Ok(PostAction::Continue)
        })
    {
        let _ = std::fs::remove_file(&path);
        return Err(Error::Loop(err.error.to_string()));
    }
    app.socket_path = Some(path);
    let run_result = event_loop.run(None::<Duration>, &mut app, |app| {
        app.drain(DRAIN_CAP);
        if app.any_outgoing() && !app.write_armed {
            app.arm_write_timer();
        }
        if app.shutdown_after_flush && !app.any_outgoing() {
            app.signal.stop();
        }
    });
    let socket_path = app.socket_path.take();
    drop(app);
    drop(event_loop);
    if let Some(path) = socket_path {
        let _ = std::fs::remove_file(path);
    }
    run_result.map_err(|err| Error::Loop(err.to_string()))
}

fn bind_listener(path: &std::path::Path) -> Result<UnixListener, Error> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| Error::Io(err.to_string()))?;
        }
    }
    let listener = match UnixListener::bind(path) {
        Ok(listener) => listener,
        Err(err)
            if err.kind() == ErrorKind::AddrInUse || err.kind() == ErrorKind::AlreadyExists =>
        {
            std::fs::remove_file(path).map_err(|err| Error::Io(err.to_string()))?;
            UnixListener::bind(path).map_err(|err| Error::Io(err.to_string()))?
        }
        Err(err) => return Err(Error::Io(err.to_string())),
    };
    if let Err(err) = listener.set_nonblocking(true) {
        drop(listener);
        let _ = std::fs::remove_file(path);
        return Err(Error::Io(err.to_string()));
    }
    Ok(listener)
}

fn busy() -> Error {
    Error::Wayland(iris_backend_wayland::Error::Dispatch(
        "wayland session is busy".to_owned(),
    ))
}

fn map_fit(mode: ScaleMode) -> Fit {
    match mode {
        ScaleMode::Fill => Fit::Fill,
        ScaleMode::Fit => Fit::Fit,
        ScaleMode::Stretch => Fit::Stretch,
        ScaleMode::Center => Fit::Center,
    }
}

fn materialize(
    config: &WallpaperConfig,
    still: Option<std::sync::Arc<DecodedTexture>>,
    scene: Option<std::sync::Arc<ScenePackage>>,
) -> Result<Asset, Error> {
    if let Some(package) = scene {
        return Ok(Asset::Scene(SceneAsset {
            runtime: SceneRuntime::new((*package).clone()),
            package,
        }));
    }
    if let Some(texture) = still {
        return Ok(Asset::Still(texture));
    }
    let options = PlaybackOptions {
        looping: config.playback.looping,
        muted: config.playback.muted,
        volume: config.volume,
        rate: config.playback.rate,
    };
    match load_wallpaper(&config.source, options)? {
        Loaded::Still(texture) => Ok(Asset::Still(std::sync::Arc::new(texture))),
        Loaded::Video(player) => Ok(Asset::Video(player)),
    }
}

fn upload_asset(renderer: &mut WallpaperSurface, asset: &Asset) -> Result<(), Error> {
    match asset {
        Asset::Still(texture) => renderer.upload(texture).map_err(Error::from),
        Asset::Video(player) => {
            let frame = player
                .warmup()
                .ok_or_else(|| Error::Load("video produced no frame".to_owned()))?;
            renderer.upload_video(&yuv_of(&frame)).map_err(Error::from)
        }
        Asset::Scene(scene) => {
            let images = scene
                .package
                .images
                .iter()
                .map(|image| image.as_ref())
                .collect::<Vec<_>>();
            renderer.upload_scene_images(&images).map_err(Error::from)
        }
    }
}

fn asset_size(asset: &Asset) -> (u32, u32) {
    match asset {
        Asset::Still(texture) => (texture.width, texture.height),
        Asset::Video(player) => {
            let info = player.info();
            (info.width, info.height)
        }
        Asset::Scene(_) => (1, 1),
    }
}

fn yuv_of(frame: &DecodedVideoFrame) -> YuvFrame<'_> {
    match &frame.picture {
        Picture::Yuv420 { y, u, v, matrix } => YuvFrame::Yuv420 {
            y: borrowed_plane(y),
            u: borrowed_plane(u),
            v: borrowed_plane(v),
            matrix: mapped_matrix(*matrix),
        },
        Picture::Nv12 { y, uv, matrix } => YuvFrame::Nv12 {
            y: borrowed_plane(y),
            uv: borrowed_plane(uv),
            matrix: mapped_matrix(*matrix),
        },
    }
}

fn borrowed_plane(plane: &iris_video::Plane) -> YuvPlane<'_> {
    YuvPlane {
        width: plane.width,
        height: plane.height,
        stride: plane.stride,
        data: &plane.bytes,
    }
}

fn mapped_matrix(matrix: Matrix) -> YuvMatrix {
    match matrix {
        Matrix::Bt601 => YuvMatrix::Bt601,
        Matrix::Bt709 => YuvMatrix::Bt709,
    }
}

impl App {
    fn session_rc(&self) -> Result<Rc<RefCell<Session>>, Error> {
        self.session.clone().ok_or_else(|| {
            Error::Wayland(iris_backend_wayland::Error::Connect(
                "wayland is not connected".to_owned(),
            ))
        })
    }

    fn ensure_wayland(&mut self) -> Result<(), Error> {
        if self.session.is_some() {
            return Ok(());
        }
        if let Some(message) = &self.wayland_error {
            return Err(Error::Wayland(iris_backend_wayland::Error::Connect(
                message.clone(),
            )));
        }
        let session = match Session::connect() {
            Ok(session) => session,
            Err(err) => {
                self.wayland_error = Some(err.to_string());
                return Err(Error::Wayland(err));
            }
        };
        let session = Rc::new(RefCell::new(session));
        let pump = match WaylandPump::new(Rc::clone(&session)) {
            Ok(pump) => pump,
            Err(err) => return Err(Error::Wayland(err)),
        };
        if let Err(err) = self.handle.insert_source(pump, |_event, _meta, app| {
            app.drain(DRAIN_CAP);
        }) {
            return Err(Error::Loop(err.error.to_string()));
        }
        self.session = Some(session);
        self.drain(DRAIN_CAP);
        Ok(())
    }

    fn drain(&mut self, cap: usize) {
        let Some(session) = self.session.clone() else {
            return;
        };
        for _ in 0..cap {
            let events = {
                let Ok(mut session) = session.try_borrow_mut() else {
                    tracing::debug!("wayland session is busy");
                    break;
                };
                session.take_events()
            };
            if events.is_empty() {
                break;
            }
            for event in events {
                self.on_backend(event);
            }
        }
    }

    fn accept_clients(&mut self, listener: &UnixListener) {
        loop {
            match listener.accept() {
                Ok((stream, _)) => self.take_client(stream),
                Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                Err(err) => {
                    tracing::debug!("accept: {err}");
                    break;
                }
            }
        }
    }

    fn take_client(&mut self, stream: UnixStream) {
        if let Err(err) = stream.set_nonblocking(true) {
            tracing::debug!("accepted socket: {err}");
            return;
        }
        let Ok(watched) = stream.try_clone() else {
            return;
        };
        let id = self.next_client;
        self.next_client = self.next_client.saturating_add(1);
        self.clients.insert(
            id,
            ClientSlot {
                stream,
                decoder: FrameDecoder::new(),
                outgoing: Vec::new(),
            },
        );
        let source = Generic::new(watched, Interest::READ, Mode::Level);
        if self
            .handle
            .insert_source(source, move |readiness, _meta, app| {
                if readiness.error {
                    app.drop_client(id);
                    return Ok(PostAction::Remove);
                }
                app.on_client(id)
            })
            .is_err()
        {
            self.drop_client(id);
        }
    }

    fn on_client(&mut self, id: u64) -> Result<PostAction, std::io::Error> {
        if !self.pump_reads(id) || !self.drain_payloads(id) {
            return Ok(PostAction::Remove);
        }
        Ok(PostAction::Continue)
    }

    fn pump_reads(&mut self, id: u64) -> bool {
        let mut buf = [0u8; 8192];
        loop {
            let step = {
                let Some(slot) = self.clients.get_mut(&id) else {
                    return false;
                };
                read_step(&mut slot.stream, &mut buf)
            };
            match step {
                ReadStep::Bytes(0) => continue,
                ReadStep::Bytes(n) => {
                    if let Some(slot) = self.clients.get_mut(&id) {
                        slot.decoder.push(&buf[..n]);
                    }
                }
                ReadStep::Blocked => return true,
                ReadStep::Closed => {
                    self.drop_client(id);
                    return false;
                }
            }
        }
    }

    fn drain_payloads(&mut self, id: u64) -> bool {
        loop {
            let next = {
                let Some(slot) = self.clients.get_mut(&id) else {
                    return false;
                };
                slot.decoder.next_payload()
            };
            match next {
                Ok(None) => return self.clients.contains_key(&id),
                Ok(Some(payload)) => {
                    self.handle_payload(id, &payload);
                    if !self.clients.contains_key(&id) {
                        return false;
                    }
                }
                Err(_) => {
                    self.drop_client(id);
                    return false;
                }
            }
        }
    }

    fn handle_payload(&mut self, id: u64, payload: &[u8]) {
        let request = match IpcRequest::from_payload(payload) {
            Ok(request) => request,
            Err(err) => {
                let frame = frame_core(RequestId::Null, &err);
                self.reply(id, &frame, false);
                return;
            }
        };
        let request_id = request.id.clone();
        let call = match request.decode_call() {
            Ok(call) => call,
            Err(err) => {
                let frame = frame_core(request_id, &err);
                self.reply(id, &frame, false);
                return;
            }
        };
        let stop = matches!(call, IpcCall::Shutdown);
        let frame = match self.dispatch(call) {
            Ok(body) => frame_ok(request_id, body),
            Err(err) => frame_err(request_id, err.to_ipc()),
        };
        self.reply(id, &frame, stop);
        self.drain(DRAIN_CAP);
    }

    fn dispatch(&mut self, call: IpcCall) -> Result<ResponseBody, Error> {
        match call {
            IpcCall::Ping => Ok(ResponseBody::Pong {
                protocol: PROTOCOL_VERSION,
            }),
            IpcCall::Shutdown => {
                self.running = false;
                self.shutdown_cleanup();
                Ok(ResponseBody::Ack)
            }
            IpcCall::GetStatus => Ok(self.status()),
            IpcCall::ListMonitors => self.list_monitors(),
            IpcCall::ApplyWallpaper { config } => self.apply(config),
            IpcCall::ClearWallpaper { target } => self.clear(target),
            IpcCall::Pause { target } => self.pause(target),
            IpcCall::Resume { target } => self.resume(target),
            IpcCall::SetPointer { x, y } => self.set_pointer(x, y),
            IpcCall::GetTelemetry => Ok(ResponseBody::Telemetry(crate::policy::report(
                &self.telemetry,
            ))),
            _ => Err(Error::Unsupported("unsupported ipc call".to_owned())),
        }
    }

    fn list_monitors(&mut self) -> Result<ResponseBody, Error> {
        self.ensure_wayland()?;
        self.drain(DRAIN_CAP);
        let session = self.session_rc()?;
        let session = session.try_borrow().map_err(|_| busy())?;
        let monitors = session
            .snapshots()
            .into_iter()
            .map(|snapshot| MonitorInfo {
                index: snapshot.index,
                connector: snapshot.connector,
                name: snapshot.name,
                width_px: snapshot.width_px,
                height_px: snapshot.height_px,
                scale: snapshot.scale,
                primary: snapshot.primary,
                refresh_millihz: snapshot.refresh_millihz,
            })
            .collect();
        Ok(ResponseBody::Monitors { monitors })
    }

    fn status(&self) -> ResponseBody {
        let mut rows: Vec<(u32, WallpaperStatus)> = self
            .slots
            .values()
            .map(|slot| {
                (
                    slot.index,
                    WallpaperStatus {
                        id: slot.config.id.clone(),
                        target: MonitorTarget::Connector {
                            name: slot.connector.clone(),
                        },
                        state: slot.state,
                        message: slot.message.clone(),
                    },
                )
            })
            .collect();
        for slot in self.webs.values() {
            rows.push((
                slot.index,
                WallpaperStatus {
                    id: slot.config.id.clone(),
                    target: MonitorTarget::Connector {
                        name: slot.connector.clone(),
                    },
                    state: slot.state,
                    message: slot.message.clone(),
                },
            ));
        }
        rows.sort_by_key(|(index, _)| *index);
        ResponseBody::Status {
            running: self.running,
            wallpapers: rows.into_iter().map(|(_, row)| row).collect(),
        }
    }

    fn apply(&mut self, config: WallpaperConfig) -> Result<ResponseBody, Error> {
        if let Some(project) = crate::loader::web_project(&config.source)? {
            return self.apply_web(config, project);
        }
        let fit = map_fit(config.scale);
        let (still, scene) = match &config.source {
            iris_core::WallpaperSource::Video { .. } => (None, None),
            other => match prepare_scene(other)? {
                Some(package) => (None, Some(std::sync::Arc::new(package))),
                None => (Some(std::sync::Arc::new(load_source(other)?)), None),
            },
        };
        self.ensure_wayland()?;
        self.drain(DRAIN_CAP);
        let snapshots = {
            let session = self.session_rc()?;
            let session = session.try_borrow().map_err(|_| busy())?;
            session.snapshots()
        };
        let targets = resolve_apply(&snapshots, &config.target)?;
        if matches!(config.target, MonitorTarget::All) {
            self.web_broadcast = None;
            self.webs.clear();
            self.broadcast = Some(SharedAsset {
                config: config.clone(),
                still: still.clone(),
                scene: scene.clone(),
                fit,
            });
        }
        let mut failure = None;
        for snapshot in targets {
            let asset = materialize(&config, still.clone(), scene.clone())?;
            if let Err(err) = self.apply_one(&snapshot, &config, asset, fit) {
                tracing::warn!("apply {}: {err}", snapshot.connector);
                if failure.is_none() {
                    failure = Some(err);
                }
            }
        }
        self.drain(DRAIN_CAP);
        match failure {
            Some(err) => Err(err),
            None => Ok(ResponseBody::Ack),
        }
    }

    fn apply_web(
        &mut self,
        config: WallpaperConfig,
        project: iris_web::WebProject,
    ) -> Result<ResponseBody, Error> {
        let monitors = self.web_monitors(&config.target)?;
        if matches!(config.target, MonitorTarget::All) {
            self.broadcast = None;
            self.web_broadcast = Some(config.clone());
            self.drop_slots(self.slots.keys().copied().collect());
            self.webs.clear();
        }
        self.ensure_web_timer();
        let mut failure = None;
        for monitor in monitors {
            if let Err(err) = self.apply_web_one(&monitor, &config, &project) {
                tracing::warn!("web apply {}: {err}", monitor.connector);
                if failure.is_none() {
                    failure = Some(err);
                }
            }
        }
        match failure {
            Some(err) => Err(err),
            None => Ok(ResponseBody::Ack),
        }
    }

    fn apply_web_one(
        &mut self,
        monitor: &WebMonitor,
        config: &WallpaperConfig,
        project: &iris_web::WebProject,
    ) -> Result<(), Error> {
        self.drop_slots(vec![monitor.id]);
        self.webs.remove(&monitor.id);
        let instance = iris_web::WebWallpaperInstance::open(
            project.clone(),
            iris_web::Bounds {
                x: monitor.x,
                y: monitor.y,
                width: monitor.width,
                height: monitor.height,
            },
            true,
        );
        let launched = instance.launched();
        let message = instance.warning().map(str::to_owned);
        if let Some(message) = &message {
            tracing::warn!("web wallpaper on {}: {message}", monitor.connector);
        }
        let state = if launched {
            RunState::Playing
        } else {
            RunState::Failed
        };
        self.webs.insert(
            monitor.id,
            crate::loader::WebSlot {
                index: monitor.index,
                connector: monitor.connector.clone(),
                primary: monitor.primary,
                config: config.clone(),
                state,
                message: message.clone(),
                user_paused: false,
                auto_paused: false,
                instance,
            },
        );
        if launched {
            Ok(())
        } else {
            Err(Error::Load(
                message.unwrap_or_else(|| "webview failed to start".to_owned()),
            ))
        }
    }

    fn web_monitors(&mut self, target: &MonitorTarget) -> Result<Vec<WebMonitor>, Error> {
        match self.ensure_wayland() {
            Ok(()) => {
                self.drain(DRAIN_CAP);
                let snapshots = {
                    let session = self.session_rc()?;
                    let session = session.try_borrow().map_err(|_| busy())?;
                    session.snapshots()
                };
                if snapshots.is_empty() {
                    tracing::warn!(
                        "no wayland outputs; opening the web wallpaper on a virtual display"
                    );
                    return virtual_web_monitor(target);
                }
                let resolved = resolve_apply(&snapshots, target)?;
                let mut cursor = 0i32;
                let mut monitors = Vec::with_capacity(resolved.len());
                for snapshot in resolved {
                    let width = snapshot.width_px.max(1);
                    monitors.push(WebMonitor {
                        id: snapshot.id,
                        index: snapshot.index,
                        connector: snapshot.connector,
                        primary: snapshot.primary,
                        x: cursor,
                        y: 0,
                        width,
                        height: snapshot.height_px.max(1),
                    });
                    cursor = cursor.saturating_add(width as i32);
                }
                Ok(monitors)
            }
            Err(err) => {
                tracing::warn!("wayland unavailable for web wallpaper: {err}");
                virtual_web_monitor(target)
            }
        }
    }

    fn ensure_web_timer(&mut self) {
        if self.web_timer {
            return;
        }
        let timer = Timer::from_duration(Duration::from_millis(16));
        match self.handle.insert_source(timer, |_instant, _meta, app| {
            app.pump_webs();
            TimeoutAction::ToDuration(Duration::from_millis(16))
        }) {
            Ok(_) => self.web_timer = true,
            Err(err) => tracing::error!("arm web timer: {}", err.error),
        }
    }

    /// GTK and webview script injection. This timer does not replace `wl_surface.frame`.
    fn pump_webs(&mut self) {
        iris_web::pump();
        let playing = self
            .webs
            .values()
            .any(|slot| slot.state == RunState::Playing);
        if !playing {
            return;
        }
        let audio = iris_scene::AudioEngine::shared().snapshot();
        let telemetry = self.telemetry.snapshot();
        for slot in self.webs.values_mut() {
            crate::loader::drive(slot, &audio, &telemetry);
        }
    }

    fn apply_one(
        &mut self,
        snapshot: &OutputSnapshot,
        config: &WallpaperConfig,
        asset: Asset,
        fit: Fit,
    ) -> Result<(), Error> {
        if self.slots.contains_key(&snapshot.id) {
            self.replace_slot(snapshot, config, asset, fit)
        } else {
            self.create_slot(snapshot, config, asset, fit)
        }
    }

    fn replace_slot(
        &mut self,
        snapshot: &OutputSnapshot,
        config: &WallpaperConfig,
        asset: Asset,
        fit: Fit,
    ) -> Result<(), Error> {
        self.webs.remove(&snapshot.id);
        let layer_id = self
            .slots
            .get(&snapshot.id)
            .map(|slot| slot.layer_id)
            .ok_or_else(|| Error::Wayland(iris_backend_wayland::Error::UnknownLayer(0)))?;
        let (width, height, pending) = {
            let session = self.session_rc()?;
            let session = session.try_borrow().map_err(|_| busy())?;
            let geometry = session.geometry(layer_id)?;
            let pending = session.frame_pending(layer_id)?;
            (
                geometry.physical_w.max(1),
                geometry.physical_h.max(1),
                pending,
            )
        };
        {
            let Some(slot) = self.slots.get_mut(&snapshot.id) else {
                return Err(Error::InvalidTarget(format!(
                    "output {} disappeared",
                    snapshot.id
                )));
            };
            slot.renderer.resize(width, height)?;
            if let Err(err) = upload_asset(&mut slot.renderer, &asset) {
                slot.state = RunState::Failed;
                slot.message = Some(err.to_string());
                return Err(err);
            }
            slot.config = config.clone();
            slot.asset = asset;
            slot.clock = MediaClock::new(config.playback.rate);
            slot.fit = fit;
            slot.state = RunState::Playing;
            slot.message = None;
            slot.index = snapshot.index;
            slot.connector = snapshot.connector.clone();
            slot.primary = snapshot.primary;
            slot.last_ms = None;
            slot.elapsed = 0.0;
            slot.last_present = None;
        }
        if !pending {
            self.queue_and_commit(layer_id);
        }
        Ok(())
    }

    fn create_slot(
        &mut self,
        snapshot: &OutputSnapshot,
        config: &WallpaperConfig,
        mut asset: Asset,
        fit: Fit,
    ) -> Result<(), Error> {
        self.webs.remove(&snapshot.id);
        let layer_id = {
            let session = self.session_rc()?;
            let mut session = session.try_borrow_mut().map_err(|_| busy())?;
            session.create_layer(snapshot.id)?
        };
        let (width, height, window) = {
            let session = self.session_rc()?;
            let session = session.try_borrow().map_err(|_| busy())?;
            let geometry = session.geometry(layer_id)?;
            let window = session.window(layer_id)?;
            (
                geometry.physical_w.max(1),
                geometry.physical_h.max(1),
                window,
            )
        };
        let opened = if let Some(gpu) = &self.gpu {
            match gpu.attach(window, width, height) {
                Ok(surface) => Opened::Surface(surface),
                Err(err) => {
                    self.destroy_layer(layer_id);
                    return Err(err.into());
                }
            }
        } else {
            match RenderContext::open(window, width, height) {
                Ok((context, surface)) => Opened::First(context, surface),
                Err(err) => {
                    self.destroy_layer(layer_id);
                    return Err(err.into());
                }
            }
        };
        let (context, mut surface) = match opened {
            Opened::First(context, surface) => (Some(context), surface),
            Opened::Surface(surface) => (None, surface),
        };
        if let Err(err) = upload_asset(&mut surface, &asset) {
            drop(surface);
            drop(context);
            self.destroy_layer(layer_id);
            return Err(err);
        }
        {
            let session = self.session_rc()?;
            let mut session = match session.try_borrow_mut() {
                Ok(session) => session,
                Err(_) => {
                    drop(surface);
                    drop(context);
                    self.destroy_layer(layer_id);
                    return Err(busy());
                }
            };
            if let Err(err) = session.queue_frame(layer_id) {
                drop(session);
                drop(surface);
                drop(context);
                self.destroy_layer(layer_id);
                return Err(err.into());
            }
        }
        let (image_width, image_height) = asset_size(&asset);
        let delta = frame_delta(None, 0, config.playback.rate);
        let pointer = self.pointer;
        let initial = if let Asset::Scene(scene) = &mut asset {
            let (width, height) = surface.size();
            let viewport = [width as f32, height as f32];
            let tick = scene.runtime.tick(delta, pointer, viewport, 0.0);
            let spans = tick
                .spans
                .iter()
                .map(|span| DrawSpan {
                    blend: span.blend,
                    texture: span.texture,
                    start: span.start,
                    count: span.count,
                })
                .collect::<Vec<_>>();
            surface.draw_scene(&SceneDraw {
                time: 0.0,
                delta_time: delta,
                pointer,
                viewport,
                audio_spectrum_16: tick.audio16,
                audio_spectrum_64: tick.audio64,
                clear: tick.clear,
                instances: &tick.instances,
                spans: &spans,
                telemetry: crate::policy::telemetry_frame(&self.telemetry),
            })
        } else {
            surface.draw(DrawParams {
                time: 0.0,
                delta_time: delta,
                fit,
                image_width,
                image_height,
            })
        };
        if let Err(err) = initial {
            drop(surface);
            drop(context);
            self.commit_layer(layer_id);
            self.destroy_layer(layer_id);
            return Err(err.into());
        }
        if let Some(context) = context {
            self.gpu = Some(context);
        }
        self.slots.insert(
            snapshot.id,
            Slot {
                renderer: surface,
                layer_id,
                index: snapshot.index,
                connector: snapshot.connector.clone(),
                primary: snapshot.primary,
                config: config.clone(),
                asset,
                clock: MediaClock::new(config.playback.rate),
                fit,
                state: RunState::Playing,
                message: None,
                user_paused: false,
                auto_paused: false,
                last_ms: None,
                elapsed: 0.0,
                last_present: Some(Instant::now()),
            },
        );
        Ok(())
    }

    fn clear(&mut self, target: MonitorTarget) -> Result<ResponseBody, Error> {
        if matches!(target, MonitorTarget::All) {
            self.broadcast = None;
            self.web_broadcast = None;
            self.drop_slots(self.slots.keys().copied().collect());
            self.webs.clear();
            return Ok(ResponseBody::Ack);
        }
        let (gpu, web) = self.control_sets(Some(target))?;
        self.drop_slots(gpu);
        for id in web {
            self.webs.remove(&id);
        }
        Ok(ResponseBody::Ack)
    }

    fn set_pointer(&mut self, x: f32, y: f32) -> Result<ResponseBody, Error> {
        self.pointer = [x, y];
        for slot in self.webs.values_mut() {
            if slot.state == RunState::Playing {
                slot.instance.push_pointer(x, y);
            }
        }
        Ok(ResponseBody::Ack)
    }

    fn pause(&mut self, target: Option<MonitorTarget>) -> Result<ResponseBody, Error> {
        let (gpu, web) = self.control_sets(target)?;
        for id in gpu {
            if let Some(slot) = self.slots.get_mut(&id) {
                slot.state = RunState::Paused;
                slot.user_paused = true;
                slot.auto_paused = false;
                slot.clock.pause();
                if let Asset::Video(player) = &slot.asset {
                    player.set_paused(true);
                }
            }
        }
        for id in web {
            if let Some(slot) = self.webs.get_mut(&id) {
                crate::loader::user_pause(slot);
            }
        }
        Ok(ResponseBody::Ack)
    }

    fn resume(&mut self, target: Option<MonitorTarget>) -> Result<ResponseBody, Error> {
        let (gpu, web) = self.control_sets(target)?;
        let mut layers = Vec::new();
        for id in gpu {
            if let Some(slot) = self.slots.get_mut(&id) {
                slot.state = RunState::Playing;
                slot.user_paused = false;
                slot.auto_paused = false;
                slot.message = None;
                slot.clock.resume();
                if let Asset::Video(player) = &slot.asset {
                    player.set_paused(false);
                }
                layers.push(slot.layer_id);
            }
        }
        for id in web {
            if let Some(slot) = self.webs.get_mut(&id) {
                crate::loader::user_resume(slot);
            }
        }
        for layer_id in layers {
            self.kick_if_idle(layer_id);
        }
        Ok(ResponseBody::Ack)
    }

    fn control_sets(&self, target: Option<MonitorTarget>) -> Result<(Vec<u32>, Vec<u32>), Error> {
        match target {
            None => Ok((
                self.slots.keys().copied().collect(),
                self.webs.keys().copied().collect(),
            )),
            Some(target) => {
                let gpu = match self.matching_slots(&target) {
                    Ok(ids) => ids,
                    Err(Error::InvalidTarget(_)) => Vec::new(),
                    Err(err) => return Err(err),
                };
                let web = self.web_ids(&target)?;
                if gpu.is_empty() && web.is_empty() && !matches!(target, MonitorTarget::All) {
                    return Err(Error::InvalidTarget(format!("no wallpaper on {target}")));
                }
                Ok((gpu, web))
            }
        }
    }

    fn web_ids(&self, target: &MonitorTarget) -> Result<Vec<u32>, Error> {
        let ids: Vec<u32> = match target {
            MonitorTarget::All => self.webs.keys().copied().collect(),
            MonitorTarget::Primary => self
                .webs
                .iter()
                .filter(|(_, slot)| slot.primary)
                .map(|(id, _)| *id)
                .collect(),
            MonitorTarget::Index { index } => self
                .webs
                .iter()
                .filter(|(_, slot)| slot.index == *index)
                .map(|(id, _)| *id)
                .collect(),
            MonitorTarget::Connector { name } => self
                .webs
                .iter()
                .filter(|(_, slot)| slot.connector == *name)
                .map(|(id, _)| *id)
                .collect(),
            _ => return Err(Error::Unsupported(format!("monitor target {target}"))),
        };
        Ok(ids)
    }

    fn matching_slots(&self, target: &MonitorTarget) -> Result<Vec<u32>, Error> {
        let ids: Vec<u32> = match target {
            MonitorTarget::All => self.slots.keys().copied().collect(),
            MonitorTarget::Primary => self
                .slots
                .iter()
                .filter(|(_, slot)| slot.primary)
                .map(|(id, _)| *id)
                .collect(),
            MonitorTarget::Index { index } => self
                .slots
                .iter()
                .filter(|(_, slot)| slot.index == *index)
                .map(|(id, _)| *id)
                .collect(),
            MonitorTarget::Connector { name } => self
                .slots
                .iter()
                .filter(|(_, slot)| slot.connector == *name)
                .map(|(id, _)| *id)
                .collect(),
            _ => {
                return Err(Error::Unsupported(format!("monitor target {target}")));
            }
        };
        if ids.is_empty() && !matches!(target, MonitorTarget::All) {
            return Err(Error::InvalidTarget(format!("no wallpaper on {target}")));
        }
        Ok(ids)
    }

    fn drop_slots(&mut self, ids: Vec<u32>) {
        let mut layers = Vec::new();
        for id in ids {
            if let Some(slot) = self.slots.remove(&id) {
                layers.push(slot.layer_id);
            }
        }
        for layer_id in layers {
            self.destroy_layer(layer_id);
        }
    }

    fn shutdown_cleanup(&mut self) {
        self.broadcast = None;
        self.web_broadcast = None;
        self.webs.clear();
        let slots = std::mem::take(&mut self.slots);
        drop(slots);
        self.gpu = None;
        if let Some(session) = self.session.clone() {
            if let Ok(mut session) = session.try_borrow_mut() {
                session.destroy_all();
            }
        }
    }

    fn on_backend(&mut self, event: BackendEvent) {
        match event {
            BackendEvent::OutputRemoved { id } => {
                self.drop_slots(vec![id]);
                self.webs.remove(&id);
            }
            BackendEvent::OutputAdded(snapshot) => {
                if self.slots.contains_key(&snapshot.id) || self.webs.contains_key(&snapshot.id) {
                    return;
                }
                if let Some(config) = self.web_broadcast.clone() {
                    self.hotplug_web(&snapshot, &config);
                    return;
                }
                let Some(asset) = self.broadcast.clone() else {
                    return;
                };
                let material =
                    match materialize(&asset.config, asset.still.clone(), asset.scene.clone()) {
                        Ok(material) => material,
                        Err(err) => {
                            tracing::warn!("hotplug {}: {err}", snapshot.connector);
                            return;
                        }
                    };
                if let Err(err) = self.apply_one(&snapshot, &asset.config, material, asset.fit) {
                    tracing::warn!("hotplug {}: {err}", snapshot.connector);
                }
            }
            BackendEvent::OutputUpdated(snapshot) => {
                if let Some(slot) = self.webs.get_mut(&snapshot.id) {
                    slot.connector = snapshot.connector.clone();
                    slot.index = snapshot.index;
                    slot.primary = snapshot.primary;
                }
                if let Some(slot) = self.slots.get_mut(&snapshot.id) {
                    slot.connector = snapshot.connector.clone();
                    slot.index = snapshot.index;
                    slot.primary = snapshot.primary;
                }
                let layer_id = self.slots.get(&snapshot.id).map(|slot| slot.layer_id);
                if let Some(layer_id) = layer_id {
                    if let Some(geometry) = self.geometry(layer_id) {
                        self.on_geometry(geometry);
                    }
                }
            }
            BackendEvent::Configured(geometry) | BackendEvent::ScaleChanged(geometry) => {
                self.on_geometry(geometry);
            }
            BackendEvent::Frame {
                layer_id,
                output_id,
                time_ms,
            } => self.on_frame(layer_id, output_id, time_ms),
            BackendEvent::Closed { layer_id } => {
                let output_id = self
                    .slots
                    .iter()
                    .find(|(_, slot)| slot.layer_id == layer_id)
                    .map(|(id, _)| *id);
                if let Some(output_id) = output_id {
                    self.drop_slots(vec![output_id]);
                } else {
                    self.destroy_layer(layer_id);
                }
            }
        }
    }

    fn on_geometry(&mut self, geometry: LayerGeometry) {
        let playing = {
            let Some(slot) = self.slots.get_mut(&geometry.output_id) else {
                return;
            };
            if slot.layer_id != geometry.layer_id {
                return;
            }
            let width = geometry.physical_w.max(1);
            let height = geometry.physical_h.max(1);
            if let Err(err) = slot.renderer.resize(width, height) {
                slot.state = RunState::Failed;
                slot.message = Some(err.to_string());
                return;
            }
            slot.state == RunState::Playing
        };
        let pending = self.frame_is_pending(geometry.layer_id).unwrap_or(true);
        if !playing || !pending {
            self.prepare_and_commit(geometry.layer_id);
        }
    }

    fn on_frame(&mut self, layer_id: u64, output_id: u32, time_ms: u32) {
        self.apply_context();
        let Some(output_id) = self.output_for_layer(layer_id, output_id) else {
            return;
        };
        let pointer = self.pointer;
        let on_battery = self.telemetry.snapshot().on_battery;
        let telemetry = crate::policy::telemetry_frame(&self.telemetry);
        let decision = {
            let Some(slot) = self.slots.get_mut(&output_id) else {
                return;
            };
            if slot.state != RunState::Playing {
                return;
            }
            let cap = crate::policy::effective_fps(&slot.config, on_battery);
            if skip_for_cap(slot.last_present, cap) {
                if let Asset::Video(player) = &slot.asset {
                    player.note_media_time(slot.clock.seconds());
                }
                FrameDecision::Skip
            } else {
                let delta = frame_delta(slot.last_ms, time_ms, slot.config.playback.rate);
                let elapsed = slot.elapsed;
                let fit = slot.fit;
                if matches!(slot.asset, Asset::Scene(_)) {
                    let (width, height) = slot.renderer.size();
                    let viewport = [width as f32, height as f32];
                    if let Asset::Scene(scene) = &mut slot.asset {
                        let tick = scene.runtime.tick(delta, pointer, viewport, elapsed);
                        FrameDecision::Scene {
                            delta,
                            elapsed,
                            pointer,
                            viewport,
                            tick,
                        }
                    } else {
                        FrameDecision::Skip
                    }
                } else {
                    let video_frame = match &slot.asset {
                        Asset::Video(player) => {
                            let media = slot.clock.seconds();
                            player.note_media_time(media);
                            Some(player.take_due(media))
                        }
                        Asset::Still(_) | Asset::Scene(_) => None,
                    };
                    match video_frame {
                        Some(None) => FrameDecision::Skip,
                        Some(Some(frame)) => {
                            let image_width = frame.width;
                            let image_height = frame.height;
                            if let Err(err) = slot.renderer.upload_video(&yuv_of(&frame)) {
                                slot.state = RunState::Failed;
                                slot.message = Some(err.to_string());
                                if let Asset::Video(player) = &slot.asset {
                                    player.set_paused(true);
                                }
                                FrameDecision::Failed
                            } else {
                                FrameDecision::Draw {
                                    delta,
                                    elapsed,
                                    fit,
                                    image_width,
                                    image_height,
                                }
                            }
                        }
                        None => {
                            let (image_width, image_height) = match &slot.asset {
                                Asset::Still(texture) => (texture.width, texture.height),
                                Asset::Video(_) | Asset::Scene(_) => (1, 1),
                            };
                            FrameDecision::Draw {
                                delta,
                                elapsed,
                                fit,
                                image_width,
                                image_height,
                            }
                        }
                    }
                }
            }
        };
        match decision {
            FrameDecision::Skip | FrameDecision::Failed => self.queue_and_commit(layer_id),
            FrameDecision::Scene {
                delta,
                elapsed,
                pointer,
                viewport,
                tick,
            } => {
                self.prepare_and_queue(layer_id);
                let spans = tick
                    .spans
                    .iter()
                    .map(|span| DrawSpan {
                        blend: span.blend,
                        texture: span.texture,
                        start: span.start,
                        count: span.count,
                    })
                    .collect::<Vec<_>>();
                let draw = SceneDraw {
                    time: elapsed + delta,
                    delta_time: delta,
                    pointer,
                    viewport,
                    audio_spectrum_16: tick.audio16,
                    audio_spectrum_64: tick.audio64,
                    clear: tick.clear,
                    instances: &tick.instances,
                    spans: &spans,
                    telemetry,
                };
                let drawn = {
                    let Some(slot) = self.slots.get_mut(&output_id) else {
                        return;
                    };
                    slot.renderer.draw_scene(&draw)
                };
                match drawn {
                    Ok(()) => {
                        if let Some(slot) = self.slots.get_mut(&output_id) {
                            slot.last_present = Some(Instant::now());
                            slot.elapsed = elapsed + delta;
                            slot.last_ms = Some(time_ms);
                            slot.state = RunState::Playing;
                            slot.message = None;
                        }
                    }
                    Err(err) => {
                        if let Some(slot) = self.slots.get_mut(&output_id) {
                            slot.state = RunState::Failed;
                            slot.message = Some(err.to_string());
                        }
                        self.commit_layer(layer_id);
                    }
                }
            }
            FrameDecision::Draw {
                delta,
                elapsed,
                fit,
                image_width,
                image_height,
            } => {
                self.prepare_and_queue(layer_id);
                let drawn = {
                    let Some(slot) = self.slots.get_mut(&output_id) else {
                        return;
                    };
                    slot.renderer.draw(DrawParams {
                        time: elapsed + delta,
                        delta_time: delta,
                        fit,
                        image_width,
                        image_height,
                    })
                };
                match drawn {
                    Ok(()) => {
                        if let Some(slot) = self.slots.get_mut(&output_id) {
                            slot.last_present = Some(Instant::now());
                            slot.elapsed = elapsed + delta;
                            slot.last_ms = Some(time_ms);
                            slot.state = RunState::Playing;
                            slot.message = None;
                        }
                    }
                    Err(err) => {
                        if let Some(slot) = self.slots.get_mut(&output_id) {
                            slot.state = RunState::Failed;
                            slot.message = Some(err.to_string());
                        }
                        self.commit_layer(layer_id);
                    }
                }
            }
        }
    }

    fn output_for_layer(&self, layer_id: u64, output_id: u32) -> Option<u32> {
        if self
            .slots
            .get(&output_id)
            .is_some_and(|slot| slot.layer_id == layer_id)
        {
            return Some(output_id);
        }
        self.slots
            .iter()
            .find(|(_, slot)| slot.layer_id == layer_id)
            .map(|(id, _)| *id)
    }

    fn geometry(&self, layer_id: u64) -> Option<LayerGeometry> {
        let session = self.session.clone()?;
        let session = session.try_borrow().ok()?;
        session.geometry(layer_id).ok()
    }

    fn frame_is_pending(&self, layer_id: u64) -> Result<bool, Error> {
        let session = self.session_rc()?;
        let session = session.try_borrow().map_err(|_| busy())?;
        Ok(session.frame_pending(layer_id)?)
    }

    fn prepare_and_queue(&self, layer_id: u64) {
        let Ok(session) = self.session_rc() else {
            return;
        };
        let Ok(mut session) = session.try_borrow_mut() else {
            return;
        };
        let _ = session.prepare_viewport(layer_id);
        let _ = session.queue_frame(layer_id);
    }

    fn prepare_and_commit(&self, layer_id: u64) {
        let Ok(session) = self.session_rc() else {
            return;
        };
        let Ok(mut session) = session.try_borrow_mut() else {
            return;
        };
        let _ = session.prepare_viewport(layer_id);
        let _ = session.commit(layer_id);
    }

    fn queue_and_commit(&self, layer_id: u64) {
        let Ok(session) = self.session_rc() else {
            return;
        };
        let Ok(mut session) = session.try_borrow_mut() else {
            return;
        };
        if session.queue_frame(layer_id).is_ok() {
            let _ = session.commit(layer_id);
        }
    }

    fn commit_layer(&self, layer_id: u64) {
        let Ok(session) = self.session_rc() else {
            return;
        };
        let Ok(mut session) = session.try_borrow_mut() else {
            return;
        };
        let _ = session.commit(layer_id);
    }

    fn kick_if_idle(&self, layer_id: u64) {
        let Ok(session) = self.session_rc() else {
            return;
        };
        let Ok(mut session) = session.try_borrow_mut() else {
            return;
        };
        match session.frame_pending(layer_id) {
            Ok(false) => {
                if session.queue_frame(layer_id).is_ok() {
                    let _ = session.commit(layer_id);
                }
            }
            Ok(true) => {}
            Err(err) => tracing::debug!("resume frame: {err}"),
        }
    }

    fn destroy_layer(&self, layer_id: u64) {
        let Ok(session) = self.session_rc() else {
            return;
        };
        let Ok(mut session) = session.try_borrow_mut() else {
            tracing::debug!("wayland session is busy; layer {layer_id} stays until shutdown");
            return;
        };
        if let Err(err) = session.destroy_layer(layer_id) {
            tracing::debug!("destroy layer {layer_id}: {err}");
        }
    }

    fn enqueue(&mut self, id: u64, frame: &[u8]) {
        if frame.is_empty() {
            return;
        }
        if let Some(slot) = self.clients.get_mut(&id) {
            slot.outgoing.extend_from_slice(frame);
        }
    }

    fn reply(&mut self, id: u64, frame: &[u8], stop_after_write: bool) {
        self.enqueue(id, frame);
        self.flush_client(id);
        let pending = self.client_pending(id);
        if stop_after_write {
            if pending {
                self.shutdown_after_flush = true;
                self.arm_write_timer();
            } else {
                self.signal.stop();
            }
        } else if pending {
            self.arm_write_timer();
        }
    }

    fn flush_client(&mut self, id: u64) {
        loop {
            let step = {
                let Some(slot) = self.clients.get_mut(&id) else {
                    return;
                };
                write_step(&mut slot.stream, &mut slot.outgoing)
            };
            match step {
                WriteStep::Done | WriteStep::Pending => return,
                WriteStep::Again => continue,
                WriteStep::Drop => {
                    self.drop_client(id);
                    return;
                }
            }
        }
    }

    fn flush_all(&mut self) {
        let ids: Vec<u64> = self.clients.keys().copied().collect();
        for id in ids {
            self.flush_client(id);
        }
    }

    fn client_pending(&self, id: u64) -> bool {
        self.clients
            .get(&id)
            .is_some_and(|slot| !slot.outgoing.is_empty())
    }

    fn any_outgoing(&self) -> bool {
        self.clients.values().any(|slot| !slot.outgoing.is_empty())
    }

    fn apply_context(&mut self) {
        let fullscreen = self.telemetry.is_fullscreen_app_active();
        let daemon_fullscreen = self.pause_on_fullscreen;
        let daemon_game = self.pause_on_game;
        let mut resume = Vec::new();
        for slot in self.slots.values_mut() {
            let want =
                crate::policy::want_pause(daemon_fullscreen, daemon_game, &slot.config, fullscreen);
            let playing = slot.state == RunState::Playing;
            match crate::policy::context_step(slot.user_paused, slot.auto_paused, playing, want) {
                crate::policy::ContextStep::Hold => {}
                crate::policy::ContextStep::AutoPause => {
                    slot.state = RunState::Paused;
                    slot.auto_paused = true;
                    slot.clock.pause();
                    if let Asset::Video(player) = &slot.asset {
                        player.set_paused(true);
                    }
                }
                crate::policy::ContextStep::AutoResume => {
                    slot.auto_paused = false;
                    slot.state = RunState::Playing;
                    slot.clock.resume();
                    if let Asset::Video(player) = &slot.asset {
                        player.set_paused(false);
                    }
                    resume.push(slot.layer_id);
                }
            }
        }
        for layer_id in resume {
            self.kick_if_idle(layer_id);
        }
        self.sync_webs();
    }

    fn sync_webs(&mut self) {
        let fullscreen = self.telemetry.is_fullscreen_app_active();
        let daemon_fullscreen = self.pause_on_fullscreen;
        let daemon_game = self.pause_on_game;
        for slot in self.webs.values_mut() {
            crate::loader::apply_policy(slot, fullscreen, daemon_fullscreen, daemon_game);
        }
    }

    fn hotplug_web(&mut self, snapshot: &OutputSnapshot, config: &WallpaperConfig) {
        let project = match crate::loader::web_project(&config.source) {
            Ok(Some(project)) => project,
            Ok(None) => {
                tracing::warn!(
                    "hotplug {}: source is no longer a web wallpaper",
                    snapshot.connector
                );
                return;
            }
            Err(err) => {
                tracing::warn!("hotplug {}: {err}", snapshot.connector);
                return;
            }
        };
        let monitor = WebMonitor {
            id: snapshot.id,
            index: snapshot.index,
            connector: snapshot.connector.clone(),
            primary: snapshot.primary,
            x: 0,
            y: 0,
            width: snapshot.width_px.max(1),
            height: snapshot.height_px.max(1),
        };
        if let Err(err) = self.apply_web_one(&monitor, config, &project) {
            tracing::warn!("hotplug {}: {err}", snapshot.connector);
        }
    }

    fn arm_policy_timer(&mut self) {
        let timer = Timer::from_duration(Duration::from_millis(500));
        if let Err(err) = self.handle.insert_source(timer, |_instant, _meta, app| {
            app.apply_context();
            TimeoutAction::ToDuration(Duration::from_millis(500))
        }) {
            tracing::error!("arm context timer: {}", err.error);
        }
    }

    fn arm_write_timer(&mut self) {
        if self.write_armed {
            return;
        }
        let timer = Timer::from_duration(WRITE_RETRY);
        match self.handle.insert_source(timer, |_instant, _meta, app| {
            app.flush_all();
            if app.any_outgoing() {
                TimeoutAction::ToDuration(WRITE_RETRY)
            } else {
                app.write_armed = false;
                if app.shutdown_after_flush {
                    app.signal.stop();
                }
                TimeoutAction::Drop
            }
        }) {
            Ok(_) => self.write_armed = true,
            Err(err) => tracing::error!("arm write timer: {}", err.error),
        }
    }

    fn drop_client(&mut self, id: u64) {
        self.clients.remove(&id);
    }
}

fn read_step(stream: &mut UnixStream, buf: &mut [u8]) -> ReadStep {
    match stream.read(buf) {
        Ok(0) => ReadStep::Closed,
        Ok(n) => ReadStep::Bytes(n),
        Err(err) if err.kind() == ErrorKind::WouldBlock => ReadStep::Blocked,
        Err(err) if err.kind() == ErrorKind::Interrupted => ReadStep::Bytes(0),
        Err(_) => ReadStep::Closed,
    }
}

fn write_step(stream: &mut UnixStream, outgoing: &mut Vec<u8>) -> WriteStep {
    if outgoing.is_empty() {
        return WriteStep::Done;
    }
    match stream.write(outgoing) {
        Ok(0) => WriteStep::Drop,
        Ok(n) => {
            outgoing.drain(..n);
            if outgoing.is_empty() {
                WriteStep::Done
            } else {
                WriteStep::Again
            }
        }
        Err(err) if err.kind() == ErrorKind::WouldBlock => WriteStep::Pending,
        Err(err) if err.kind() == ErrorKind::Interrupted => WriteStep::Again,
        Err(_) => WriteStep::Drop,
    }
}

fn frame_ok(id: RequestId, body: ResponseBody) -> Vec<u8> {
    match IpcResponse::success(id, body).to_frame() {
        Ok(frame) => frame,
        Err(err) => {
            tracing::error!("encode response: {err}");
            Vec::new()
        }
    }
}

fn frame_err(id: RequestId, body: IpcErrorBody) -> Vec<u8> {
    match IpcResponse::failure(id, body).to_frame() {
        Ok(frame) => frame,
        Err(err) => {
            tracing::error!("encode error response: {err}");
            Vec::new()
        }
    }
}

fn frame_core(id: RequestId, err: &iris_core::Error) -> Vec<u8> {
    frame_err(id, IpcErrorBody::from_core(err))
}

fn virtual_web_monitor(target: &MonitorTarget) -> Result<Vec<WebMonitor>, Error> {
    let monitor = WebMonitor {
        id: 1,
        index: 0,
        connector: "virtual-0".to_owned(),
        primary: true,
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    match target {
        MonitorTarget::All | MonitorTarget::Primary => Ok(vec![monitor]),
        MonitorTarget::Index { index } if *index == 0 => Ok(vec![monitor]),
        MonitorTarget::Connector { name } if name == "virtual-0" => Ok(vec![monitor]),
        MonitorTarget::Index { index } => {
            Err(Error::InvalidTarget(format!("no output at index {index}")))
        }
        MonitorTarget::Connector { name } => {
            Err(Error::InvalidTarget(format!("unknown connector {name}")))
        }
        _ => Err(Error::Unsupported(format!("monitor target {target}"))),
    }
}

fn resolve_apply(
    snapshots: &[OutputSnapshot],
    target: &MonitorTarget,
) -> Result<Vec<OutputSnapshot>, Error> {
    match target {
        MonitorTarget::All => {
            if snapshots.is_empty() {
                Err(Error::InvalidTarget("no outputs".to_owned()))
            } else {
                Ok(snapshots.to_vec())
            }
        }
        MonitorTarget::Primary => snapshots
            .iter()
            .find(|snapshot| snapshot.primary)
            .cloned()
            .map(|snapshot| vec![snapshot])
            .ok_or_else(|| Error::InvalidTarget("no primary output".to_owned())),
        MonitorTarget::Index { index } => snapshots
            .iter()
            .find(|snapshot| snapshot.index == *index)
            .cloned()
            .map(|snapshot| vec![snapshot])
            .ok_or_else(|| Error::InvalidTarget(format!("no output at index {index}"))),
        MonitorTarget::Connector { name } => snapshots
            .iter()
            .find(|snapshot| snapshot.connector == *name)
            .cloned()
            .map(|snapshot| vec![snapshot])
            .ok_or_else(|| Error::InvalidTarget(format!("unknown connector {name}"))),
        _ => Err(Error::Unsupported(format!("monitor target {target}"))),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    use iris_core::ipc::Outcome;
    use iris_core::{
        DaemonConfig, FrameDecoder, IpcCall, IpcEndpoint, IpcRequest, IpcResponse, RequestId,
        ResponseBody, PROTOCOL_VERSION,
    };

    use super::frame_delta;
    use crate::run_with;

    #[test]
    fn frame_delta_uses_one_second_and_falls_back_after() {
        assert_eq!(frame_delta(Some(0), 1_000, 1.0), 1.0);
        assert_eq!(frame_delta(Some(0), 2_000, 1.0), 1.0 / 60.0);
        assert_eq!(frame_delta(Some(0), 1_000, 2.0), 2.0);
    }

    #[test]
    fn named_pipe_is_rejected_before_listen() {
        let name = r"\\.\pipe\iris-test-daemon";
        let err = run_with(DaemonConfig {
            endpoint: IpcEndpoint::NamedPipe {
                name: name.to_owned(),
            },
            ..DaemonConfig::default()
        })
        .expect_err("named pipe");
        let text = err.to_string();
        assert!(text.contains(name), "{text}");
    }

    #[test]
    fn ping_and_shutdown_roundtrip_on_a_temp_socket() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("iris-test-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("socket dir");
        let path = dir.join("iris.sock");
        let path_for_thread = path.clone();
        let handle = std::thread::spawn(move || {
            run_with(DaemonConfig {
                endpoint: IpcEndpoint::Unix {
                    path: path_for_thread,
                },
                ..DaemonConfig::default()
            })
        });
        let mut stream = connect_retry(&path, &handle);
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("write timeout");
        send_call(&mut stream, 1, &IpcCall::Ping);
        let ping = read_response(&mut stream);
        send_call(&mut stream, 2, &IpcCall::GetStatus);
        let status = read_response(&mut stream);
        send_call(&mut stream, 3, &IpcCall::Shutdown);
        let shutdown = read_response(&mut stream);
        let joined = handle.join().expect("daemon thread");
        assert!(matches!(
            ping.outcome().expect("ping outcome"),
            Outcome::Success(ResponseBody::Pong { protocol }) if *protocol == PROTOCOL_VERSION
        ));
        assert!(matches!(
            status.outcome().expect("status outcome"),
            Outcome::Success(ResponseBody::Status { running: true, wallpapers }) if wallpapers.is_empty()
        ));
        assert!(matches!(
            shutdown.outcome().expect("shutdown outcome"),
            Outcome::Success(ResponseBody::Ack)
        ));
        joined.expect("daemon result");
        assert!(!path.exists(), "socket file remains at {}", path.display());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn connect_retry(
        path: &std::path::Path,
        handle: &std::thread::JoinHandle<Result<(), crate::Error>>,
    ) -> UnixStream {
        let started = Instant::now();
        loop {
            if handle.is_finished() {
                panic!("daemon exited before the socket accepted a client");
            }
            match UnixStream::connect(path) {
                Ok(stream) => return stream,
                Err(err) if started.elapsed() < Duration::from_secs(2) => {
                    std::thread::sleep(Duration::from_millis(20));
                    let _ = err;
                }
                Err(err) => panic!("connect {}: {err}", path.display()),
            }
        }
    }

    fn send_call(stream: &mut UnixStream, id: i64, call: &IpcCall) {
        let request = IpcRequest::from_call(RequestId::Number(id), call).expect("request");
        let frame = request.to_frame().expect("frame");
        stream.write_all(&frame).expect("write");
    }

    fn read_response(stream: &mut UnixStream) -> IpcResponse {
        let mut decoder = FrameDecoder::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).expect("read");
            assert!(n > 0, "daemon closed the socket");
            decoder.push(&buf[..n]);
            if let Some(payload) = decoder.next_payload().expect("frame") {
                return IpcResponse::from_payload(&payload).expect("response");
            }
        }
    }
}
