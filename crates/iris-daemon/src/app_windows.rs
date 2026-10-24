//! Named-pipe IPC and the Win32 wallpaper tick.
//!
//! The current-thread runtime owns the pipe accepts, the replies, and the
//! 16 ms message pump. WASAPI runs on its own thread. Each slot drops its
//! wgpu surface before its HWND.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iris_backend_windows::{
    enumerate_monitors, pump_messages, DesktopWindow, DisplayMonitor, Loopback, PumpEvent,
};
use iris_core::ipc::IpcErrorBody;
use iris_core::{
    read_payload, write_bytes, FrameDecoder, IpcCall, IpcEndpoint, IpcRequest, IpcResponse,
    MonitorInfo, MonitorTarget, NamedPipeListener, RequestId, ResponseBody, RunState, ScaleMode,
    WallpaperConfig, WallpaperSource, WallpaperStatus, PROTOCOL_VERSION,
};
use iris_pkg::DecodedTexture;
use iris_render::{DrawParams, RenderContext, SceneDraw, WallpaperSurface};
use iris_scene::{AudioEngine, ScenePackage, SceneRuntime};
use tokio::sync::{mpsc, oneshot};

use crate::error::Error;
use crate::load::{load_source, load_wallpaper, prepare_scene, Loaded};

struct Slot {
    surface: WallpaperSurface,
    window: DesktopWindow,
    index: u32,
    device_name: String,
    primary: bool,
    config: WallpaperConfig,
    asset: Asset,
    fit: iris_render::Fit,
    state: RunState,
    message: Option<String>,
    user_paused: bool,
    auto_paused: bool,
    elapsed: f32,
    last_present: Option<Instant>,
}

struct SceneAsset {
    package: Arc<ScenePackage>,
    runtime: SceneRuntime,
}

enum Asset {
    Still(Arc<DecodedTexture>),
    Scene(SceneAsset),
}

struct App {
    slots: HashMap<String, Slot>,
    gpu: Option<RenderContext>,
    monitors: Vec<DisplayMonitor>,
    broadcast: Option<WallpaperConfig>,
    running: bool,
    pointer: [f32; 2],
    telemetry: iris_telemetry::Hub,
    pause_on_fullscreen: bool,
    pause_on_game: bool,
    webs: HashMap<String, crate::loader::WebSlot>,
    web_broadcast: Option<WallpaperConfig>,
}

struct Pending {
    payload: Vec<u8>,
    reply: oneshot::Sender<(Vec<u8>, oneshot::Sender<()>)>,
}

pub(crate) fn serve(config: iris_core::DaemonConfig) -> Result<(), Error> {
    let name = match config.endpoint {
        IpcEndpoint::NamedPipe { name } => name,
        IpcEndpoint::Unix { path } => {
            return Err(Error::Unsupported(format!(
                "unix socket is not supported: {}",
                path.display()
            )));
        }
    };
    let pause_on_fullscreen = config.pause_on_fullscreen;
    let pause_on_game = config.pause_on_game;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|err| Error::Io(err.to_string()))?;
    runtime.block_on(serve_async(name, pause_on_fullscreen, pause_on_game))
}

async fn serve_async(
    name: String,
    pause_on_fullscreen: bool,
    pause_on_game: bool,
) -> Result<(), Error> {
    let engine = AudioEngine::shared();
    let _loopback = Loopback::start(move |samples, rate| engine.push_loopback(samples, rate));
    let message_window = match DesktopWindow::message_window() {
        Ok(window) => Some(window),
        Err(err) => {
            tracing::warn!("message window: {err}");
            None
        }
    };
    let monitors = match enumerate_monitors() {
        Ok(monitors) => monitors,
        Err(err) => {
            tracing::warn!("monitor enumeration: {err}");
            Vec::new()
        }
    };
    let mut listener = NamedPipeListener::bind(&name)?;
    let (inbound_tx, mut inbound_rx) = mpsc::channel::<Pending>(32);
    let mut clients = Vec::new();
    let mut app = App {
        slots: HashMap::new(),
        gpu: None,
        monitors,
        broadcast: None,
        running: true,
        pointer: [0.0, 0.0],
        telemetry: iris_telemetry::Hub::start(iris_telemetry::DEFAULT_INTERVAL),
        pause_on_fullscreen,
        pause_on_game,
        webs: HashMap::new(),
        web_broadcast: None,
    };
    tracing::info!("listening on {name}");

    loop {
        tokio::select! {
            biased;
            accepted = listener.accept() => {
                match accepted {
                    Ok(stream) => {
                        let tx = inbound_tx.clone();
                        clients.push(tokio::spawn(async move {
                            client_task(stream, tx).await;
                        }));
                    }
                    Err(err) => tracing::warn!("named pipe accept: {err}"),
                }
            }
            pending = inbound_rx.recv() => {
                let Some(pending) = pending else {
                    break;
                };
                let frame = app.handle_payload(&pending.payload);
                let shutdown = !app.running;
                let (done_tx, done_rx) = oneshot::channel();
                if pending.reply.send((frame, done_tx)).is_err() {
                    if shutdown {
                        break;
                    }
                    continue;
                }
                if shutdown {
                    let _ = done_rx.await;
                    break;
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(16)) => {
                app.tick();
            }
        }
    }

    drop(app);
    drop(message_window);
    for client in clients {
        client.abort();
    }
    Ok(())
}

async fn client_task(
    mut stream: tokio::net::windows::named_pipe::NamedPipeServer,
    inbound: mpsc::Sender<Pending>,
) {
    let mut decoder = FrameDecoder::new();
    loop {
        let payload = match read_payload(&mut stream, &mut decoder).await {
            Ok(payload) => payload,
            Err(_) => break,
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        if inbound
            .send(Pending {
                payload,
                reply: reply_tx,
            })
            .await
            .is_err()
        {
            break;
        }
        let Ok((frame, done_tx)) = reply_rx.await else {
            break;
        };
        if !frame.is_empty() {
            let _ = write_bytes(&mut stream, &frame).await;
        }
        let _ = done_tx.send(());
    }
}

impl App {
    fn tick(&mut self) {
        if pump_messages() == PumpEvent::DisplayChange {
            self.hotplug();
        }
        self.apply_context();
        let pointer = self.pointer;
        let on_battery = self.telemetry.snapshot().on_battery;
        let telemetry = crate::policy::telemetry_frame(&self.telemetry);
        let names: Vec<String> = self.slots.keys().cloned().collect();
        for name in names {
            let Some(slot) = self.slots.get_mut(&name) else {
                continue;
            };
            present(slot, pointer, on_battery, telemetry);
        }
        self.pump_webs();
    }

    fn apply_context(&mut self) {
        let fullscreen = self.telemetry.is_fullscreen_app_active();
        let daemon_fullscreen = self.pause_on_fullscreen;
        let daemon_game = self.pause_on_game;
        for slot in self.slots.values_mut() {
            let want =
                crate::policy::want_pause(daemon_fullscreen, daemon_game, &slot.config, fullscreen);
            let playing = slot.state == RunState::Playing;
            match crate::policy::context_step(slot.user_paused, slot.auto_paused, playing, want) {
                crate::policy::ContextStep::Hold => {}
                crate::policy::ContextStep::AutoPause => {
                    slot.state = RunState::Paused;
                    slot.auto_paused = true;
                }
                crate::policy::ContextStep::AutoResume => {
                    slot.auto_paused = false;
                    if slot.state == RunState::Paused {
                        slot.state = RunState::Playing;
                        slot.last_present = None;
                    }
                }
            }
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

    fn hotplug(&mut self) {
        let monitors = match enumerate_monitors() {
            Ok(monitors) => monitors,
            Err(err) => {
                tracing::warn!("hotplug enumeration: {err}");
                return;
            }
        };
        let live: Vec<String> = monitors
            .iter()
            .map(|monitor| monitor.device_name.clone())
            .collect();
        let stale: Vec<String> = self
            .slots
            .keys()
            .filter(|name| !live.iter().any(|live_name| live_name == *name))
            .cloned()
            .collect();
        for name in stale {
            self.slots.remove(&name);
        }
        let stale_web: Vec<String> = self
            .webs
            .keys()
            .filter(|name| !live.iter().any(|live_name| live_name == *name))
            .cloned()
            .collect();
        for name in stale_web {
            self.webs.remove(&name);
        }
        for monitor in &monitors {
            if let Some(slot) = self.webs.get_mut(&monitor.device_name) {
                slot.index = monitor.index;
                slot.primary = monitor.primary;
                slot.instance.place(iris_web::Bounds {
                    x: monitor.x,
                    y: monitor.y,
                    width: monitor.width_px,
                    height: monitor.height_px,
                });
            }
        }
        for monitor in &monitors {
            if let Some(slot) = self.slots.get_mut(&monitor.device_name) {
                slot.index = monitor.index;
                slot.primary = monitor.primary;
                if let Err(err) =
                    slot.window
                        .move_to(monitor.x, monitor.y, monitor.width_px, monitor.height_px)
                {
                    tracing::warn!("resize {}: {err}", monitor.device_name);
                }
                if let Err(err) = slot.surface.resize(monitor.width_px, monitor.height_px) {
                    slot.state = RunState::Failed;
                    slot.message = Some(err.to_string());
                }
            }
        }
        self.monitors = monitors;
        if let Some(config) = self.web_broadcast.clone() {
            self.hotplug_web(&config);
            return;
        }
        let Some(config) = self.broadcast.clone() else {
            return;
        };
        let fit = map_fit(config.scale);
        let asset = match load_asset(&config) {
            Ok(asset) => asset,
            Err(err) => {
                tracing::warn!("hotplug reload: {err}");
                return;
            }
        };
        let fresh: Vec<DisplayMonitor> = self
            .monitors
            .iter()
            .filter(|monitor| {
                !self.slots.contains_key(&monitor.device_name)
                    && !self.webs.contains_key(&monitor.device_name)
            })
            .cloned()
            .collect();
        for monitor in fresh {
            let one = clone_asset(&asset);
            if let Err(err) = self.create_slot(&monitor, &config, one, fit) {
                tracing::warn!("hotplug {}: {err}", monitor.device_name);
            }
        }
    }

    fn handle_payload(&mut self, payload: &[u8]) -> Vec<u8> {
        let request = match IpcRequest::from_payload(payload) {
            Ok(request) => request,
            Err(err) => return frame_core(RequestId::Null, &err),
        };
        let request_id = request.id.clone();
        let call = match request.decode_call() {
            Ok(call) => call,
            Err(err) => return frame_core(request_id, &err),
        };
        match self.dispatch(call) {
            Ok(body) => frame_ok(request_id, body),
            Err(err) => frame_err(request_id, err.to_ipc()),
        }
    }

    fn dispatch(&mut self, call: IpcCall) -> Result<ResponseBody, Error> {
        match call {
            IpcCall::Ping => Ok(ResponseBody::Pong {
                protocol: PROTOCOL_VERSION,
            }),
            IpcCall::Shutdown => {
                self.running = false;
                self.broadcast = None;
                self.web_broadcast = None;
                self.slots.clear();
                self.webs.clear();
                self.gpu = None;
                Ok(ResponseBody::Ack)
            }
            IpcCall::GetStatus => Ok(self.status()),
            IpcCall::ListMonitors => self.list_monitors(),
            IpcCall::ApplyWallpaper { config } => self.apply(config),
            IpcCall::ClearWallpaper { target } => self.clear(&target),
            IpcCall::Pause { target } => self.set_paused(target, true),
            IpcCall::Resume { target } => self.set_paused(target, false),
            IpcCall::SetPointer { x, y } => self.set_pointer(x, y),
            IpcCall::GetTelemetry => Ok(ResponseBody::Telemetry(crate::policy::report(
                &self.telemetry,
            ))),
            _ => Err(Error::Unsupported("unsupported ipc call".to_owned())),
        }
    }

    fn list_monitors(&mut self) -> Result<ResponseBody, Error> {
        self.monitors = enumerate_monitors()?;
        let monitors = self
            .monitors
            .iter()
            .map(|monitor| MonitorInfo {
                index: monitor.index,
                connector: monitor.device_name.clone(),
                name: monitor.device_name.clone(),
                width_px: monitor.width_px,
                height_px: monitor.height_px,
                scale: monitor.scale,
                primary: monitor.primary,
                refresh_millihz: monitor.refresh_millihz,
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
                            name: slot.device_name.clone(),
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
        let asset = load_asset(&config)?;
        if matches!(config.target, MonitorTarget::All) {
            self.web_broadcast = None;
            self.webs.clear();
            self.broadcast = Some(config.clone());
        }
        let targets = resolve(&self.monitors, &config.target)?;
        let mut failure = None;
        for monitor in targets {
            let one = clone_asset(&asset);
            let result = if self.slots.contains_key(&monitor.device_name) {
                self.replace_slot(&monitor, &config, one, fit)
            } else {
                self.create_slot(&monitor, &config, one, fit)
            };
            if let Err(err) = result {
                tracing::warn!("apply {}: {err}", monitor.device_name);
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

    fn apply_web(
        &mut self,
        config: WallpaperConfig,
        project: iris_web::WebProject,
    ) -> Result<ResponseBody, Error> {
        let monitors = self.web_monitors(&config.target)?;
        if matches!(config.target, MonitorTarget::All) {
            self.broadcast = None;
            self.web_broadcast = Some(config.clone());
            self.slots.clear();
            self.gpu = None;
            self.webs.clear();
        }
        let mut failure = None;
        for monitor in monitors {
            if let Err(err) = self.apply_web_one(&monitor, &config, &project) {
                tracing::warn!("web apply {}: {err}", monitor.device_name);
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
        monitor: &DisplayMonitor,
        config: &WallpaperConfig,
        project: &iris_web::WebProject,
    ) -> Result<(), Error> {
        self.slots.remove(&monitor.device_name);
        if self.slots.is_empty() {
            self.gpu = None;
        }
        self.webs.remove(&monitor.device_name);
        let instance = iris_web::WebWallpaperInstance::open(
            project.clone(),
            iris_web::Bounds {
                x: monitor.x,
                y: monitor.y,
                width: monitor.width_px.max(1),
                height: monitor.height_px.max(1),
            },
            true,
        );
        let launched = instance.launched();
        let message = instance.warning().map(str::to_owned);
        if let Some(message) = &message {
            tracing::warn!("web wallpaper on {}: {message}", monitor.device_name);
        }
        let state = if launched {
            RunState::Playing
        } else {
            RunState::Failed
        };
        self.webs.insert(
            monitor.device_name.clone(),
            crate::loader::WebSlot {
                index: monitor.index,
                connector: monitor.device_name.clone(),
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

    fn web_monitors(&self, target: &MonitorTarget) -> Result<Vec<DisplayMonitor>, Error> {
        if self.monitors.is_empty() {
            tracing::warn!("no monitors; opening the web wallpaper on a virtual display");
            return virtual_display(target);
        }
        resolve(&self.monitors, target)
    }

    fn hotplug_web(&mut self, config: &WallpaperConfig) {
        let project = match crate::loader::web_project(&config.source) {
            Ok(Some(project)) => project,
            Ok(None) => {
                tracing::warn!("hotplug web source is no longer a web wallpaper");
                return;
            }
            Err(err) => {
                tracing::warn!("hotplug web: {err}");
                return;
            }
        };
        let fresh: Vec<DisplayMonitor> = self
            .monitors
            .iter()
            .filter(|monitor| {
                !self.slots.contains_key(&monitor.device_name)
                    && !self.webs.contains_key(&monitor.device_name)
            })
            .cloned()
            .collect();
        for monitor in fresh {
            if let Err(err) = self.apply_web_one(&monitor, config, &project) {
                tracing::warn!("hotplug {}: {err}", monitor.device_name);
            }
        }
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

    fn create_slot(
        &mut self,
        monitor: &DisplayMonitor,
        config: &WallpaperConfig,
        asset: Asset,
        fit: iris_render::Fit,
    ) -> Result<(), Error> {
        self.webs.remove(&monitor.device_name);
        let window =
            DesktopWindow::open(monitor.x, monitor.y, monitor.width_px, monitor.height_px)?;
        if let Err(err) = window.attach_desktop() {
            tracing::warn!("workerw injection: {err}");
        }
        let native = window.handle();
        let width = monitor.width_px.max(1);
        let height = monitor.height_px.max(1);
        let opened = if let Some(gpu) = &self.gpu {
            match gpu.attach(native, width, height) {
                Ok(surface) => Ok((None, surface)),
                Err(err) => Err(err),
            }
        } else {
            match RenderContext::open(native, width, height) {
                Ok((context, surface)) => Ok((Some(context), surface)),
                Err(err) => Err(err),
            }
        };
        let (context, mut surface) = match opened {
            Ok(pair) => pair,
            Err(err) => {
                drop(window);
                return Err(err.into());
            }
        };
        if let Err(err) = upload_asset(&mut surface, &asset) {
            drop(surface);
            drop(context);
            drop(window);
            return Err(err);
        }
        let mut slot = Slot {
            surface,
            window,
            index: monitor.index,
            device_name: monitor.device_name.clone(),
            primary: monitor.primary,
            config: config.clone(),
            asset,
            fit,
            state: RunState::Playing,
            message: None,
            user_paused: false,
            auto_paused: false,
            elapsed: 0.0,
            last_present: None,
        };
        let on_battery = self.telemetry.snapshot().on_battery;
        let telemetry = crate::policy::telemetry_frame(&self.telemetry);
        present(&mut slot, self.pointer, on_battery, telemetry);
        if let Some(context) = context {
            self.gpu = Some(context);
        }
        self.slots.insert(monitor.device_name.clone(), slot);
        Ok(())
    }

    fn replace_slot(
        &mut self,
        monitor: &DisplayMonitor,
        config: &WallpaperConfig,
        asset: Asset,
        fit: iris_render::Fit,
    ) -> Result<(), Error> {
        let pointer = self.pointer;
        let on_battery = self.telemetry.snapshot().on_battery;
        let telemetry = crate::policy::telemetry_frame(&self.telemetry);
        let Some(slot) = self.slots.get_mut(&monitor.device_name) else {
            return Err(Error::InvalidTarget(format!(
                "output {} disappeared",
                monitor.device_name
            )));
        };
        slot.window
            .move_to(monitor.x, monitor.y, monitor.width_px, monitor.height_px)?;
        slot.surface.resize(monitor.width_px, monitor.height_px)?;
        upload_asset(&mut slot.surface, &asset)?;
        slot.config = config.clone();
        slot.asset = asset;
        slot.fit = fit;
        slot.state = RunState::Playing;
        slot.user_paused = false;
        slot.auto_paused = false;
        slot.message = None;
        slot.index = monitor.index;
        slot.primary = monitor.primary;
        slot.elapsed = 0.0;
        slot.last_present = None;
        present(slot, pointer, on_battery, telemetry);
        Ok(())
    }

    fn clear(&mut self, target: &MonitorTarget) -> Result<ResponseBody, Error> {
        if matches!(target, MonitorTarget::All) {
            self.broadcast = None;
            self.web_broadcast = None;
            self.webs.clear();
        }
        let gpu = match self.matching(target) {
            Ok(names) => names,
            Err(Error::InvalidTarget(_)) => Vec::new(),
            Err(err) => return Err(err),
        };
        let web = self.web_names(target)?;
        if gpu.is_empty() && web.is_empty() && !matches!(target, MonitorTarget::All) {
            return Err(Error::InvalidTarget(format!("no wallpaper on {target}")));
        }
        for name in gpu {
            self.slots.remove(&name);
        }
        for name in web {
            self.webs.remove(&name);
        }
        if self.slots.is_empty() {
            self.gpu = None;
        }
        Ok(ResponseBody::Ack)
    }

    fn set_paused(
        &mut self,
        target: Option<MonitorTarget>,
        paused: bool,
    ) -> Result<ResponseBody, Error> {
        let (gpu, web) = match target.as_ref() {
            None => (
                self.slots.keys().cloned().collect::<Vec<_>>(),
                self.webs.keys().cloned().collect::<Vec<_>>(),
            ),
            Some(target) => {
                let gpu = match self.matching(target) {
                    Ok(names) => names,
                    Err(Error::InvalidTarget(_)) => Vec::new(),
                    Err(err) => return Err(err),
                };
                let web = self.web_names(target)?;
                if gpu.is_empty() && web.is_empty() && !matches!(target, MonitorTarget::All) {
                    return Err(Error::InvalidTarget(format!("no wallpaper on {target}")));
                }
                (gpu, web)
            }
        };
        for name in gpu {
            if let Some(slot) = self.slots.get_mut(&name) {
                if paused {
                    slot.state = RunState::Paused;
                    slot.user_paused = true;
                    slot.auto_paused = false;
                } else if slot.state == RunState::Paused {
                    slot.user_paused = false;
                    slot.auto_paused = false;
                    slot.state = RunState::Playing;
                    slot.last_present = None;
                }
            }
        }
        for name in web {
            if let Some(slot) = self.webs.get_mut(&name) {
                if paused {
                    crate::loader::user_pause(slot);
                } else {
                    crate::loader::user_resume(slot);
                }
            }
        }
        Ok(ResponseBody::Ack)
    }

    fn web_names(&self, target: &MonitorTarget) -> Result<Vec<String>, Error> {
        let names: Vec<String> = match target {
            MonitorTarget::All => self.webs.keys().cloned().collect(),
            MonitorTarget::Primary => self
                .webs
                .values()
                .filter(|slot| slot.primary)
                .map(|slot| slot.connector.clone())
                .collect(),
            MonitorTarget::Index { index } => self
                .webs
                .values()
                .filter(|slot| slot.index == *index)
                .map(|slot| slot.connector.clone())
                .collect(),
            MonitorTarget::Connector { name } => self
                .webs
                .values()
                .filter(|slot| slot.connector == *name)
                .map(|slot| slot.connector.clone())
                .collect(),
            _ => return Err(Error::Unsupported(format!("monitor target {target}"))),
        };
        Ok(names)
    }

    fn matching(&self, target: &MonitorTarget) -> Result<Vec<String>, Error> {
        let names: Vec<String> = match target {
            MonitorTarget::All => self.slots.keys().cloned().collect(),
            MonitorTarget::Primary => self
                .slots
                .values()
                .filter(|slot| slot.primary)
                .map(|slot| slot.device_name.clone())
                .collect(),
            MonitorTarget::Index { index } => self
                .slots
                .values()
                .filter(|slot| slot.index == *index)
                .map(|slot| slot.device_name.clone())
                .collect(),
            MonitorTarget::Connector { name } => self
                .slots
                .values()
                .filter(|slot| slot.device_name == *name)
                .map(|slot| slot.device_name.clone())
                .collect(),
            _ => return Err(Error::Unsupported(format!("monitor target {target}"))),
        };
        if names.is_empty() && !matches!(target, MonitorTarget::All) {
            return Err(Error::InvalidTarget(format!("no wallpaper on {target}")));
        }
        Ok(names)
    }
}

fn present(
    slot: &mut Slot,
    pointer: [f32; 2],
    on_battery: bool,
    telemetry: iris_render::TelemetryFrame,
) {
    if slot.state != RunState::Playing {
        return;
    }
    let cap = crate::policy::effective_fps(&slot.config, on_battery);
    if crate::policy::under_cap(slot.last_present, cap) {
        return;
    }
    let delta = match slot.last_present {
        Some(last) => {
            let seconds = last.elapsed().as_secs_f32();
            if seconds > 1.0 {
                1.0 / 60.0
            } else {
                seconds
            }
        }
        None => 1.0 / 60.0,
    } * slot.config.playback.rate;
    let elapsed = slot.elapsed;
    let fit = slot.fit;
    let (width, height) = slot.surface.size();
    let viewport = [width as f32, height as f32];
    let prepared = match &mut slot.asset {
        Asset::Still(texture) => Prep::Still {
            image_width: texture.width,
            image_height: texture.height,
        },
        Asset::Scene(scene) => Prep::Scene(scene.runtime.tick(delta, pointer, viewport, elapsed)),
    };
    let drawn = match prepared {
        Prep::Still {
            image_width,
            image_height,
        } => slot.surface.draw(DrawParams {
            time: elapsed + delta,
            delta_time: delta,
            fit,
            image_width,
            image_height,
        }),
        Prep::Scene(tick) => {
            let draw = SceneDraw {
                time: elapsed + delta,
                delta_time: delta,
                pointer,
                viewport,
                audio_spectrum_16: tick.audio16,
                audio_spectrum_64: tick.audio64,
                clear: tick.clear,
                instances: &tick.instances,
                spans: &tick.spans,
                telemetry,
            };
            slot.surface.draw_scene(&draw)
        }
    };
    match drawn {
        Ok(()) => {
            slot.last_present = Some(Instant::now());
            slot.elapsed = elapsed + delta;
            slot.state = RunState::Playing;
            slot.message = None;
        }
        Err(err) => {
            slot.state = RunState::Failed;
            slot.message = Some(err.to_string());
        }
    }
}

enum Prep {
    Still { image_width: u32, image_height: u32 },
    Scene(iris_scene::Tick),
}

fn load_asset(config: &WallpaperConfig) -> Result<Asset, Error> {
    if let WallpaperSource::Video { .. } = &config.source {
        return match load_wallpaper(&config.source) {
            Ok(Loaded::Still(texture)) => Err(Error::Unsupported(format!(
                "video decoded {}x{} without a windows player",
                texture.width, texture.height
            ))),
            Err(err) => Err(err),
        };
    }
    match prepare_scene(&config.source)? {
        Some(package) => {
            let package = Arc::new(package);
            Ok(Asset::Scene(SceneAsset {
                runtime: SceneRuntime::new((*package).clone()),
                package,
            }))
        }
        None => Ok(Asset::Still(Arc::new(load_source(&config.source)?))),
    }
}

fn clone_asset(asset: &Asset) -> Asset {
    match asset {
        Asset::Still(texture) => Asset::Still(Arc::clone(texture)),
        Asset::Scene(scene) => {
            let package = Arc::clone(&scene.package);
            Asset::Scene(SceneAsset {
                runtime: SceneRuntime::new((*package).clone()),
                package,
            })
        }
    }
}

fn upload_asset(surface: &mut WallpaperSurface, asset: &Asset) -> Result<(), Error> {
    match asset {
        Asset::Still(texture) => surface.upload(texture).map_err(Error::from),
        Asset::Scene(scene) => {
            let images = scene
                .package
                .images
                .iter()
                .map(|image| image.as_ref())
                .collect::<Vec<_>>();
            surface.upload_scene_images(&images).map_err(Error::from)
        }
    }
}

fn virtual_display(target: &MonitorTarget) -> Result<Vec<DisplayMonitor>, Error> {
    let monitor = DisplayMonitor {
        index: 0,
        device_name: "virtual-0".to_owned(),
        x: 0,
        y: 0,
        width_px: 1920,
        height_px: 1080,
        scale: 1.0,
        primary: true,
        refresh_millihz: Some(60_000),
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

fn resolve(
    monitors: &[DisplayMonitor],
    target: &MonitorTarget,
) -> Result<Vec<DisplayMonitor>, Error> {
    match target {
        MonitorTarget::All => {
            if monitors.is_empty() {
                Err(Error::InvalidTarget("no outputs".to_owned()))
            } else {
                Ok(monitors.to_vec())
            }
        }
        MonitorTarget::Primary => monitors
            .iter()
            .find(|monitor| monitor.primary)
            .cloned()
            .map(|monitor| vec![monitor])
            .ok_or_else(|| Error::InvalidTarget("no primary output".to_owned())),
        MonitorTarget::Index { index } => monitors
            .iter()
            .find(|monitor| monitor.index == *index)
            .cloned()
            .map(|monitor| vec![monitor])
            .ok_or_else(|| Error::InvalidTarget(format!("no output at index {index}"))),
        MonitorTarget::Connector { name } => monitors
            .iter()
            .find(|monitor| monitor.device_name == *name)
            .cloned()
            .map(|monitor| vec![monitor])
            .ok_or_else(|| Error::InvalidTarget(format!("unknown connector {name}"))),
        _ => Err(Error::Unsupported(format!("monitor target {target}"))),
    }
}

fn map_fit(mode: ScaleMode) -> iris_render::Fit {
    match mode {
        ScaleMode::Fill => iris_render::Fit::Fill,
        ScaleMode::Fit => iris_render::Fit::Fit,
        ScaleMode::Stretch => iris_render::Fit::Stretch,
        ScaleMode::Center => iris_render::Fit::Center,
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
