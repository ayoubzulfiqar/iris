//! Decode thread, hardware device, ring buffer, and optional PCM playback.
//!
//! The thread owns every `AVFrame` and `AVPacket`. Planes are copied into
//! owned buffers before those objects are dropped, which unreferences them.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ffmpeg_next::codec::packet::Mut as _;
use ffmpeg_next::ffi;
use ffmpeg_next::software::scaling::flag::Flags as ScaleFlags;
use ffmpeg_next::util::color::Space;
use ffmpeg_next::util::format::sample::Type as SamplePacking;
use ffmpeg_next::util::format::{Pixel, Sample};
use ffmpeg_next::{self as ffmpeg, ChannelLayout, Rational};

use crate::error::Error;
use crate::frame::{DecodedVideoFrame, Matrix, Picture, Plane};

const RING: usize = 4;
const AHEAD: f64 = 0.5;
const OPEN_WAIT: Duration = Duration::from_secs(5);
const MAX_EDGE: u32 = 16_384;

/// How the decoder was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwBackend {
    /// VA-API device.
    Vaapi,
    /// CUDA / NVDEC device.
    Cuda,
    /// CPU decoder.
    Software,
}

/// Stream facts captured when the file opens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoInfo {
    /// Coded width.
    pub width: u32,
    /// Coded height.
    pub height: u32,
    /// Average frame rate, or 30 when the file does not say.
    pub frame_rate: f64,
    /// Duration in seconds, when the container reports one.
    pub duration: Option<f64>,
    /// Device that opened successfully.
    pub hardware: HwBackend,
}

/// Playback knobs copied from [`iris_core`]'s wallpaper config by the daemon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackOptions {
    /// Seek to the start after `AVERROR_EOF` instead of stopping.
    pub looping: bool,
    /// Skip audio decode and do not open a device.
    pub muted: bool,
    /// Linear gain in `0.0..=1.0`.
    pub volume: f32,
    /// Speed applied to audio consumption. Video time is scaled by the daemon clock.
    pub rate: f32,
}

impl Default for PlaybackOptions {
    fn default() -> Self {
        Self {
            looping: true,
            muted: false,
            volume: 1.0,
            rate: 1.0,
        }
    }
}

struct State {
    queue: VecDeque<DecodedVideoFrame>,
    paused: bool,
    stop: bool,
    media_time: f64,
    error: Option<String>,
    info: Option<VideoInfo>,
    pcm: VecDeque<f32>,
    volume: f32,
    rate: f32,
    channels: u16,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

struct Inner {
    shared: Arc<Shared>,
    audio: Mutex<Option<cpal::Stream>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// A running decode. Cloning shares the ring and the thread.
#[derive(Clone)]
pub struct VideoPlayer {
    inner: Arc<Inner>,
}

impl VideoPlayer {
    /// Open `path` and block until the first frame is buffered or the open fails.
    pub fn open(path: &Path, options: PlaybackOptions) -> Result<Self, Error> {
        let meta = fs::symlink_metadata(path).map_err(|err| Error::open(path, err.to_string()))?;
        if !meta.file_type().is_file() {
            return Err(Error::open(path, "not a regular file"));
        }
        ensure_ffmpeg().map_err(|message| Error::open(path, message))?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                paused: false,
                stop: false,
                media_time: 0.0,
                error: None,
                info: None,
                pcm: VecDeque::new(),
                volume: options.volume.clamp(0.0, 1.0),
                rate: if options.rate.is_finite() && options.rate > 0.0 {
                    options.rate
                } else {
                    1.0
                },
                channels: 2,
            }),
            wake: Condvar::new(),
        });
        let audio = match start_audio(&shared, options.muted) {
            Ok(stream) => stream,
            Err(err) => {
                tracing::warn!("audio output unavailable, video stays silent: {err}");
                None
            }
        };
        let thread_shared = Arc::clone(&shared);
        let thread_path = path.to_path_buf();
        let thread = std::thread::Builder::new()
            .name("iris-video".to_owned())
            .spawn(move || decode_main(thread_path, options, thread_shared))
            .map_err(|err| Error::open(path, err.to_string()))?;
        let player = Self {
            inner: Arc::new(Inner {
                shared,
                audio: Mutex::new(audio),
                thread: Mutex::new(Some(thread)),
            }),
        };
        player.wait_ready(path)?;
        Ok(player)
    }

    /// Dimensions, rate, and which device opened.
    pub fn info(&self) -> VideoInfo {
        let state = lock(&self.inner.shared);
        state.info.unwrap_or(VideoInfo {
            width: 1,
            height: 1,
            frame_rate: 30.0,
            duration: None,
            hardware: HwBackend::Software,
        })
    }

    /// Take the first buffered frame and treat it as already shown.
    pub fn warmup(&self) -> Option<DecodedVideoFrame> {
        let mut state = lock(&self.inner.shared);
        let frame = state.queue.pop_front()?;
        state.media_time = frame.pts;
        self.inner.shared.wake.notify_all();
        Some(frame)
    }

    /// Publish the presentation clock so the decoder does not run ahead of it.
    pub fn note_media_time(&self, media_time: f64) {
        let mut state = lock(&self.inner.shared);
        state.media_time = media_time;
        self.inner.shared.wake.notify_all();
    }

    /// Pop the newest frame whose PTS is due. Late frames in front of it are dropped.
    pub fn take_due(&self, media_time: f64) -> Option<DecodedVideoFrame> {
        let mut state = lock(&self.inner.shared);
        state.media_time = media_time;
        let slack = 0.5
            / state
                .info
                .map(|info| info.frame_rate)
                .unwrap_or(30.0)
                .max(1.0);
        while state.queue.len() >= 2
            && state
                .queue
                .get(1)
                .is_some_and(|frame| frame.pts <= media_time + slack)
        {
            state.queue.pop_front();
        }
        let due = state
            .queue
            .front()
            .is_some_and(|frame| frame.pts <= media_time + slack);
        if !due {
            self.inner.shared.wake.notify_all();
            return None;
        }
        let frame = state.queue.pop_front();
        self.inner.shared.wake.notify_all();
        frame
    }

    /// Halt or restart the decode thread and the audio device.
    pub fn set_paused(&self, paused: bool) {
        {
            let mut state = lock(&self.inner.shared);
            state.paused = paused;
            self.inner.shared.wake.notify_all();
        }
        let audio = match self.inner.audio.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(stream) = audio.as_ref() {
            if paused {
                if let Err(err) = stream.pause() {
                    tracing::warn!("audio pause: {err}");
                }
            } else if let Err(err) = stream.play() {
                tracing::warn!("audio resume: {err}");
            }
        }
    }

    /// Replace the linear gain used by the audio callback.
    pub fn set_volume(&self, volume: f32) {
        let mut state = lock(&self.inner.shared);
        state.volume = volume.clamp(0.0, 1.0);
    }

    fn wait_ready(&self, path: &Path) -> Result<(), Error> {
        let deadline = Instant::now() + OPEN_WAIT;
        let mut state = lock(&self.inner.shared);
        loop {
            if let Some(message) = state.error.clone() {
                return Err(Error::decode(path, message));
            }
            if !state.queue.is_empty() {
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(Error::decode(path, "timed out waiting for the first frame"));
            }
            let wait = self
                .inner
                .shared
                .wake
                .wait_timeout(state, deadline.saturating_duration_since(now));
            state = match wait {
                Ok((guard, _)) => guard,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        {
            let mut state = lock(&self.shared);
            state.stop = true;
            state.paused = false;
            self.shared.wake.notify_all();
        }
        if let Some(handle) = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = handle.join();
        }
    }
}

fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    match shared.state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn ensure_ffmpeg() -> Result<(), String> {
    use std::sync::OnceLock;
    static READY: OnceLock<Result<(), String>> = OnceLock::new();
    READY
        .get_or_init(|| {
            let result = ffmpeg::init().map_err(|err| err.to_string());
            if result.is_ok() {
                ffmpeg::util::log::set_level(ffmpeg::util::log::Level::Quiet);
            }
            result
        })
        .clone()
}

fn start_audio(shared: &Arc<Shared>, muted: bool) -> Result<Option<cpal::Stream>, String> {
    if muted {
        return Ok(None);
    }
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no audio output device".to_owned())?;
    let supported = device
        .default_output_config()
        .map_err(|err| err.to_string())?;
    if supported.sample_format() != cpal::SampleFormat::F32 {
        return Err(format!(
            "output format {:?} is not f32",
            supported.sample_format()
        ));
    }
    let channels = supported.channels();
    {
        let mut state = lock(shared);
        state.channels = channels;
    }
    let config: cpal::StreamConfig = supported.into();
    let callback_shared = Arc::clone(shared);
    let stream = device
        .build_output_stream(
            &config,
            move |output: &mut [f32], _| fill_audio(output, &callback_shared),
            |err| tracing::warn!("audio device: {err}"),
            None,
        )
        .map_err(|err| err.to_string())?;
    stream.play().map_err(|err| err.to_string())?;
    Ok(Some(stream))
}

fn fill_audio(output: &mut [f32], shared: &Shared) {
    let mut state = lock(shared);
    let channels = usize::from(state.channels.max(1));
    let volume = state.volume;
    let rate = state.rate;
    let mut skip = 0.0f32;
    for frame in output.chunks_mut(channels) {
        if state.pcm.len() < 2 {
            for sample in frame.iter_mut() {
                *sample = 0.0;
            }
            continue;
        }
        let left = state.pcm.pop_front().unwrap_or(0.0) * volume;
        let right = state.pcm.pop_front().unwrap_or(0.0) * volume;
        skip += (rate - 1.0).max(0.0);
        while skip >= 1.0 && state.pcm.len() >= 2 {
            state.pcm.pop_front();
            state.pcm.pop_front();
            skip -= 1.0;
        }
        if channels == 1 {
            frame[0] = (left + right) * 0.5;
        } else {
            frame[0] = left;
            if channels > 1 {
                frame[1] = right;
            }
            for sample in frame.iter_mut().skip(2) {
                *sample = 0.0;
            }
        }
    }
}

fn decode_main(path: PathBuf, options: PlaybackOptions, shared: Arc<Shared>) {
    if let Err(err) = decode_file(&path, options, &shared) {
        tracing::error!("video {}: {err}", path.display());
        let mut state = lock(&shared);
        if state.error.is_none() {
            state.error = Some(err.to_string());
        }
        shared.wake.notify_all();
    }
}

fn decode_file(path: &Path, options: PlaybackOptions, shared: &Shared) -> Result<(), Error> {
    let mut input =
        ffmpeg::format::input(path).map_err(|err| Error::open(path, err.to_string()))?;
    let video = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| Error::open(path, "no video stream"))?;
    let video_index = video.index();
    let time_base = video.time_base();
    let frame_rate = rate_of(video.avg_frame_rate());
    let duration = seconds_of(video.duration(), time_base);
    let video_parameters = video.parameters();
    let audio = if options.muted {
        None
    } else {
        input
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .map(|stream| (stream.index(), stream.parameters()))
    };
    drop(video);

    let mut opened = open_video_decoder(&video_parameters, path)?;
    let hardware = opened.hardware;
    let mut decoder = &mut opened.decoder;
    let audio_index = audio.as_ref().map(|(index, _)| *index);
    let mut audio_decoder = match audio {
        Some((_, parameters)) => match ffmpeg::codec::Context::from_parameters(parameters)
            .and_then(|context| context.decoder().audio())
        {
            Ok(decoder) => Some(decoder),
            Err(err) => {
                tracing::warn!("audio decoder unavailable: {err}");
                None
            }
        },
        None => None,
    };
    let mut scaler: Option<ffmpeg::software::scaling::context::Context> = None;
    let mut resampler: Option<ffmpeg::software::resampling::context::Context> = None;
    let info = VideoInfo {
        width: decoder.width().max(1),
        height: decoder.height().max(1),
        frame_rate,
        duration,
        hardware,
    };
    if info.width > MAX_EDGE || info.height > MAX_EDGE {
        return Err(Error::open(
            path,
            format!("{}x{} exceeds {MAX_EDGE}", info.width, info.height),
        ));
    }
    {
        let mut state = lock(shared);
        state.info = Some(info);
    }

    let mut offset = 0.0f64;
    let mut last_pts = 0.0f64;
    let mut failures = 0u32;
    loop {
        if !wait_for_room(shared) {
            break;
        }
        match read_packet(&mut input) {
            Ok(Some(packet)) => {
                failures = 0;
                let index = packet.stream();
                if index == video_index {
                    if let Err(err) = push_video(
                        &mut decoder,
                        &packet,
                        &mut scaler,
                        time_base,
                        offset,
                        info.height,
                        shared,
                        &mut last_pts,
                    ) {
                        tracing::warn!("video frame dropped: {err}");
                    }
                } else if audio_index == Some(index) {
                    if let Some(audio_decoder) = audio_decoder.as_mut() {
                        if let Err(err) = push_audio(audio_decoder, &packet, &mut resampler, shared)
                        {
                            tracing::warn!("audio frame dropped: {err}");
                        }
                    }
                }
            }
            Ok(None) => {
                let _ = drain_video(
                    &mut decoder,
                    &mut scaler,
                    time_base,
                    offset,
                    info.height,
                    shared,
                    &mut last_pts,
                    true,
                );
                if !options.looping {
                    break;
                }
                let span = duration.unwrap_or(last_pts).max(1.0 / frame_rate.max(1.0));
                offset += span;
                if let Err(err) = seek_start(&mut input) {
                    tracing::warn!("loop seek failed: {err}");
                    break;
                }
                decoder.flush();
                if let Some(audio_decoder) = audio_decoder.as_mut() {
                    audio_decoder.flush();
                }
            }
            Err(err) => {
                failures += 1;
                tracing::warn!("packet read failed: {err}");
                if failures > 32 {
                    return Err(Error::decode(path, "too many corrupt packets"));
                }
            }
        }
    }
    Ok(())
}

fn wait_for_room(shared: &Shared) -> bool {
    let mut state = lock(shared);
    loop {
        if state.stop {
            return false;
        }
        let ahead = state
            .queue
            .back()
            .is_some_and(|frame| frame.pts > state.media_time + AHEAD);
        if !state.paused && state.queue.len() < RING && !ahead {
            return true;
        }
        state = match shared.wake.wait(state) {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
    }
}

fn read_packet(
    input: &mut ffmpeg::format::context::Input,
) -> Result<Option<ffmpeg::Packet>, ffmpeg::Error> {
    let mut packet = ffmpeg::Packet::empty();
    // SAFETY: `packet` is an owned empty AVPacket. `input` is an open
    // AVFormatContext. av_read_frame either fills the packet or returns an
    // error and leaves it empty. The packet is unreferenced when dropped.
    let ret = unsafe { ffi::av_read_frame(input.as_mut_ptr(), packet.as_mut_ptr()) };
    if ret == 0 {
        Ok(Some(packet))
    } else {
        let err = ffmpeg::Error::from(ret);
        if matches!(err, ffmpeg::Error::Eof) {
            Ok(None)
        } else {
            Err(err)
        }
    }
}

fn seek_start(input: &mut ffmpeg::format::context::Input) -> Result<(), ffmpeg::Error> {
    // SAFETY: `input` stays open. Timestamp 0 with the backward flag repositions
    // the demuxer to the start and does not free the context.
    let ret = unsafe { ffi::av_seek_frame(input.as_mut_ptr(), -1, 0, ffi::AVSEEK_FLAG_BACKWARD) };
    if ret < 0 {
        Err(ffmpeg::Error::from(ret))
    } else {
        Ok(())
    }
}

struct LiveDecoder {
    decoder: ffmpeg::decoder::Video,
    hardware: HwBackend,
    /// Kept alive so `get_format` can read it for the whole decode, including seeks.
    _hint: Option<Box<HwHint>>,
}

fn open_video_decoder(
    parameters: &ffmpeg::codec::Parameters,
    path: &Path,
) -> Result<LiveDecoder, Error> {
    let attempts = [HwBackend::Vaapi, HwBackend::Cuda, HwBackend::Software];
    let mut last = String::from("no decoder");
    for backend in attempts {
        match open_one(parameters, backend) {
            Ok(decoder) => {
                tracing::info!("video decoder: {backend:?}");
                return Ok(decoder);
            }
            Err(err) => {
                tracing::debug!("decoder {backend:?} unavailable: {err}");
                last = err;
            }
        }
    }
    Err(Error::open(path, last))
}

fn open_one(
    parameters: &ffmpeg::codec::Parameters,
    backend: HwBackend,
) -> Result<LiveDecoder, String> {
    let mut context = ffmpeg::codec::Context::from_parameters(parameters.clone())
        .map_err(|err| err.to_string())?;
    let hint = if backend == HwBackend::Software {
        None
    } else {
        attach_hardware(&mut context, backend)?
    };
    let decoder = context.decoder().video().map_err(|err| err.to_string())?;
    Ok(LiveDecoder {
        decoder,
        hardware: backend,
        _hint: hint,
    })
}

struct HwHint {
    format: ffi::AVPixelFormat,
    device: *mut ffi::AVBufferRef,
}

impl Drop for HwHint {
    fn drop(&mut self) {
        if !self.device.is_null() {
            // SAFETY: `device` is the AVBufferRef created for this hint, or null.
            // av_buffer_unref releases that one reference.
            unsafe { ffi::av_buffer_unref(&mut self.device) }
        }
    }
}

fn attach_hardware(
    context: &mut ffmpeg::codec::Context,
    backend: HwBackend,
) -> Result<Option<Box<HwHint>>, String> {
    let kind = match backend {
        HwBackend::Vaapi => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
        HwBackend::Cuda => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
        HwBackend::Software => return Ok(None),
    };
    // SAFETY: the context is allocated and not yet opened. `codec` is the
    // static AVCodec chosen from the stream parameters and lives for the
    // process. A null codec is rejected below.
    let codec = unsafe { (*context.as_mut_ptr()).codec };
    let codec = if codec.is_null() {
        let id = unsafe { (*context.as_ptr()).codec_id };
        unsafe { ffi::avcodec_find_decoder(id) }
    } else {
        codec
    };
    if codec.is_null() {
        return Err("no codec".to_owned());
    }
    let format = hw_format(codec, kind).ok_or_else(|| "codec has no hardware config".to_owned())?;
    let mut device = std::ptr::null_mut();
    // SAFETY: av_hwdevice_ctx_create writes a new device reference into
    // `device` or returns a negative code and leaves it null. The device
    // string is null so libav picks the default device for this type.
    let created = unsafe {
        ffi::av_hwdevice_ctx_create(&mut device, kind, std::ptr::null(), std::ptr::null_mut(), 0)
    };
    if created < 0 || device.is_null() {
        return Err(format!("device create returned {created}"));
    }
    let mut hint = Box::new(HwHint { format, device });
    // SAFETY: `hint` is moved into `LiveDecoder` and kept for the whole decode,
    // including seeks that call `get_format` again. `hw_device_ctx` receives
    // its own reference via av_buffer_ref. The heap address of the box does
    // not change when the box is moved.
    unsafe {
        let ctx = context.as_mut_ptr();
        (*ctx).hw_device_ctx = ffi::av_buffer_ref(hint.device);
        (*ctx).opaque = hint.as_mut() as *mut HwHint as *mut _;
        (*ctx).get_format = Some(select_format);
    }
    Ok(Some(hint))
}

fn hw_format(codec: *const ffi::AVCodec, kind: ffi::AVHWDeviceType) -> Option<ffi::AVPixelFormat> {
    let mut index = 0;
    while index < 16 {
        // SAFETY: avcodec_get_hw_config returns a pointer to static config or
        // null when `index` passes the last entry. The pointer is not freed.
        let config = unsafe { ffi::avcodec_get_hw_config(codec, index) };
        if config.is_null() {
            return None;
        }
        let config = unsafe { &*config };
        let method = ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32;
        if config.methods & method != 0 && config.device_type == kind {
            return Some(config.pix_fmt);
        }
        index += 1;
    }
    None
}

unsafe extern "C" fn select_format(
    ctx: *mut ffi::AVCodecContext,
    formats: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    // SAFETY: libav calls this during avcodec_open2 with the context we
    // configured and a NONE-terminated format list. `opaque` points at the
    // HwHint that attach_hardware keeps alive until open returns.
    let want = if ctx.is_null() || (*ctx).opaque.is_null() {
        ffi::AVPixelFormat::AV_PIX_FMT_NONE
    } else {
        let hint = (*ctx).opaque as *const HwHint;
        (*hint).format
    };
    if formats.is_null() {
        return ffi::AVPixelFormat::AV_PIX_FMT_YUV420P;
    }
    let mut cursor = formats;
    let mut fallback = ffi::AVPixelFormat::AV_PIX_FMT_YUV420P;
    let mut seen = false;
    for _ in 0..64 {
        let format = *cursor;
        if format == ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            break;
        }
        if !seen {
            fallback = format;
            seen = true;
        }
        if format == want {
            return format;
        }
        cursor = cursor.add(1);
    }
    fallback
}

fn push_video(
    decoder: &mut ffmpeg::decoder::Video,
    packet: &ffmpeg::Packet,
    scaler: &mut Option<ffmpeg::software::scaling::context::Context>,
    time_base: Rational,
    offset: f64,
    coded_height: u32,
    shared: &Shared,
    last_pts: &mut f64,
) -> Result<(), String> {
    send_video(decoder, Some(packet))?;
    drain_video(
        decoder,
        scaler,
        time_base,
        offset,
        coded_height,
        shared,
        last_pts,
        false,
    )
}

fn drain_video(
    decoder: &mut ffmpeg::decoder::Video,
    scaler: &mut Option<ffmpeg::software::scaling::context::Context>,
    time_base: Rational,
    offset: f64,
    coded_height: u32,
    shared: &Shared,
    last_pts: &mut f64,
    eof: bool,
) -> Result<(), String> {
    if eof {
        decoder.send_eof().map_err(|err| err.to_string())?;
    }
    loop {
        let mut frame = ffmpeg::frame::Video::empty();
        match decoder.receive_frame(&mut frame) {
            Ok(()) => {
                if let Some(picture) = publish_frame(
                    &frame,
                    scaler,
                    time_base,
                    offset,
                    coded_height,
                    shared,
                    last_pts,
                ) {
                    let _ = picture;
                }
                // Dropping `frame` calls av_frame_free, which unreferences it
                // after the planes have been copied.
            }
            Err(ffmpeg::Error::Eof) => return Ok(()),
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => return Ok(()),
            Err(err) => {
                tracing::warn!("receive frame: {err}");
                return Ok(());
            }
        }
    }
}

fn send_video(
    decoder: &mut ffmpeg::decoder::Video,
    packet: Option<&ffmpeg::Packet>,
) -> Result<(), String> {
    let Some(packet) = packet else {
        return Ok(());
    };
    match decoder.send_packet(packet) {
        Ok(()) => Ok(()),
        Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn publish_frame(
    frame: &ffmpeg::frame::Video,
    scaler: &mut Option<ffmpeg::software::scaling::context::Context>,
    time_base: Rational,
    offset: f64,
    coded_height: u32,
    shared: &Shared,
    last_pts: &mut f64,
) -> Option<()> {
    let software = match download(frame) {
        Ok(frame) => frame,
        Err(err) => {
            tracing::warn!("hardware download failed: {err}");
            return None;
        }
    };
    let picture = match planes_of(&software, scaler, coded_height) {
        Ok(picture) => picture,
        Err(err) => {
            tracing::warn!("plane copy failed: {err}");
            return None;
        }
    };
    let pts = offset + frame_seconds(&software, time_base);
    *last_pts = pts;
    let decoded = DecodedVideoFrame {
        pts,
        width: software.width().max(1),
        height: software.height().max(1),
        picture,
    };
    let mut state = lock(shared);
    if state.queue.len() >= RING {
        state.queue.pop_front();
    }
    state.queue.push_back(decoded);
    shared.wake.notify_all();
    Some(())
}

fn download(frame: &ffmpeg::frame::Video) -> Result<ffmpeg::frame::Video, String> {
    let format = frame.format();
    if format != Pixel::VAAPI && format != Pixel::CUDA {
        return Ok(clone_frame(frame));
    }
    let mut software = ffmpeg::frame::Video::empty();
    // SAFETY: both frames are owned AVFrames. av_hwframe_transfer_data writes
    // a software view into `software` and does not take ownership of `frame`.
    // The hardware frame is unreferenced when `frame` is later dropped.
    let ret = unsafe { ffi::av_hwframe_transfer_data(software.as_mut_ptr(), frame.as_ptr(), 0) };
    if ret < 0 {
        Err(format!("transfer returned {ret}"))
    } else {
        Ok(software)
    }
}

fn clone_frame(frame: &ffmpeg::frame::Video) -> ffmpeg::frame::Video {
    let mut copy = ffmpeg::frame::Video::empty();
    // SAFETY: both frames are owned. av_frame_ref bumps the reference on the
    // source buffers so the copy stays valid after the source is unreferenced.
    let ret = unsafe { ffi::av_frame_ref(copy.as_mut_ptr(), frame.as_ptr()) };
    if ret < 0 {
        return ffmpeg::frame::Video::empty();
    }
    copy
}

fn planes_of(
    frame: &ffmpeg::frame::Video,
    scaler: &mut Option<ffmpeg::software::scaling::context::Context>,
    coded_height: u32,
) -> Result<Picture, String> {
    let matrix = matrix_for(frame.color_space(), coded_height);
    match frame.format() {
        Pixel::YUV420P => yuv420(frame, matrix),
        Pixel::NV12 => nv12(frame, matrix),
        _ => {
            let converted = scale_to_yuv(frame, scaler)?;
            yuv420(&converted, matrix)
        }
    }
}

fn scale_to_yuv(
    frame: &ffmpeg::frame::Video,
    scaler: &mut Option<ffmpeg::software::scaling::context::Context>,
) -> Result<ffmpeg::frame::Video, String> {
    let width = frame.width();
    let height = frame.height();
    let format = frame.format();
    let stale = scaler.as_ref().is_none_or(|existing| {
        existing.input().format != format
            || existing.input().width != width
            || existing.input().height != height
    });
    if stale {
        *scaler = Some(
            ffmpeg::software::scaling::context::Context::get(
                format,
                width,
                height,
                Pixel::YUV420P,
                width,
                height,
                ScaleFlags::BILINEAR,
            )
            .map_err(|err| err.to_string())?,
        );
    }
    let scaler = scaler.as_mut().ok_or_else(|| "scaler missing".to_owned())?;
    let mut output = ffmpeg::frame::Video::empty();
    scaler
        .run(frame, &mut output)
        .map_err(|err| err.to_string())?;
    Ok(output)
}

fn yuv420(frame: &ffmpeg::frame::Video, matrix: Matrix) -> Result<Picture, String> {
    Ok(Picture::Yuv420 {
        y: copy_plane(frame, 0)?,
        u: copy_plane(frame, 1)?,
        v: copy_plane(frame, 2)?,
        matrix,
    })
}

fn nv12(frame: &ffmpeg::frame::Video, matrix: Matrix) -> Result<Picture, String> {
    Ok(Picture::Nv12 {
        y: copy_plane(frame, 0)?,
        uv: copy_plane(frame, 1)?,
        matrix,
    })
}

fn copy_plane(frame: &ffmpeg::frame::Video, index: usize) -> Result<Plane, String> {
    if index >= frame.planes() {
        return Err(format!("missing plane {index}"));
    }
    // SAFETY: `index` is in range. linesize is the row stride libav stored on
    // this owned frame. A negative stride is a bottom-up plane we refuse.
    let linesize = unsafe { (*frame.as_ptr()).linesize[index] };
    if linesize <= 0 {
        return Err(format!("plane {index} stride {linesize}"));
    }
    let stride = linesize as usize;
    let width = frame.plane_width(index);
    let height = frame.plane_height(index);
    if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
        return Err(format!("plane {index} is {width}x{height}"));
    }
    let rows = height as usize;
    let need = stride
        .checked_mul(rows)
        .ok_or_else(|| format!("plane {index} overflows"))?;
    let data = frame.data(index);
    if data.len() < need {
        return Err(format!("plane {index} is short"));
    }
    Ok(Plane {
        width,
        height,
        stride,
        bytes: data[..need].to_vec(),
    })
}

fn matrix_for(space: Space, height: u32) -> Matrix {
    match space {
        Space::BT470BG | Space::SMPTE170M | Space::FCC => Matrix::Bt601,
        Space::BT709 | Space::BT2020NCL | Space::BT2020CL => Matrix::Bt709,
        _ if height >= 720 => Matrix::Bt709,
        _ => Matrix::Bt601,
    }
}

fn push_audio(
    decoder: &mut ffmpeg::decoder::Audio,
    packet: &ffmpeg::Packet,
    resampler: &mut Option<ffmpeg::software::resampling::context::Context>,
    shared: &Shared,
) -> Result<(), String> {
    match decoder.send_packet(packet) {
        Ok(()) => {}
        Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {}
        Err(err) => return Err(err.to_string()),
    }
    loop {
        let mut frame = ffmpeg::frame::Audio::empty();
        match decoder.receive_frame(&mut frame) {
            Ok(()) => {
                if let Err(err) = mix_audio(&frame, resampler, shared) {
                    tracing::warn!("resample: {err}");
                }
            }
            Err(ffmpeg::Error::Eof) => return Ok(()),
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => return Ok(()),
            Err(err) => {
                tracing::warn!("receive audio: {err}");
                return Ok(());
            }
        }
    }
}

fn mix_audio(
    frame: &ffmpeg::frame::Audio,
    resampler: &mut Option<ffmpeg::software::resampling::context::Context>,
    shared: &Shared,
) -> Result<(), String> {
    let out_rate = 48_000u32;
    if resampler.is_none() {
        *resampler = Some(
            ffmpeg::software::resampling::context::Context::get(
                frame.format(),
                frame.channel_layout(),
                frame.rate().max(1),
                Sample::F32(SamplePacking::Packed),
                ChannelLayout::STEREO,
                out_rate,
            )
            .map_err(|err| err.to_string())?,
        );
    }
    let resampler = resampler
        .as_mut()
        .ok_or_else(|| "resampler missing".to_owned())?;
    let mut output = ffmpeg::frame::Audio::empty();
    resampler
        .run(frame, &mut output)
        .map_err(|err| err.to_string())?;
    if output.planes() == 0 || output.format() != Sample::F32(SamplePacking::Packed) {
        return Ok(());
    }
    let samples = output.plane::<f32>(0);
    let mut state = lock(shared);
    let cap = out_rate as usize * 2 * 2;
    for sample in samples {
        if state.pcm.len() >= cap {
            break;
        }
        state.pcm.push_back(*sample);
    }
    Ok(())
}

fn frame_seconds(frame: &ffmpeg::frame::Video, time_base: Rational) -> f64 {
    let ticks = frame.timestamp().or(frame.pts()).unwrap_or(0);
    seconds_of(ticks, time_base).unwrap_or(0.0)
}

fn seconds_of(ticks: i64, time_base: Rational) -> Option<f64> {
    if ticks < 0 || time_base.denominator() == 0 {
        return None;
    }
    Some(ticks as f64 * f64::from(time_base.numerator()) / f64::from(time_base.denominator()))
}

fn rate_of(rational: Rational) -> f64 {
    if rational.denominator() == 0 || rational.numerator() <= 0 {
        30.0
    } else {
        f64::from(rational.numerator()) / f64::from(rational.denominator())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_names_the_path() {
        let path = PathBuf::from("/tmp/iris-video-missing.mp4");
        let err = match VideoPlayer::open(
            &path,
            PlaybackOptions {
                muted: true,
                ..PlaybackOptions::default()
            },
        ) {
            Err(err) => err,
            Ok(_player) => panic!("missing file opened"),
        };
        let text = err.to_string();
        assert!(text.contains("iris-video-missing.mp4"), "{text}");
    }

    #[test]
    fn h264_frames_loop_past_the_end() {
        let path = std::env::temp_dir().join(format!("iris-video-{}.mp4", std::process::id()));
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=64x64:rate=10:duration=0.4",
                "-pix_fmt",
                "yuv420p",
                "-c:v",
                "libx264",
                "-an",
            ])
            .arg(&path)
            .status()
            .expect("spawn ffmpeg");
        assert!(status.success(), "ffmpeg status {status}");
        let player = VideoPlayer::open(
            &path,
            PlaybackOptions {
                looping: true,
                muted: true,
                volume: 0.0,
                rate: 1.0,
            },
        )
        .expect("open");
        let info = player.info();
        assert_eq!((info.width, info.height), (64, 64));
        let first = player.warmup().expect("first frame");
        assert_eq!(first.width, 64);
        match &first.picture {
            Picture::Yuv420 { y, .. } | Picture::Nv12 { y, .. } => {
                assert_eq!(y.width, 64);
                assert_eq!(y.height, 64);
                assert!(!y.bytes.is_empty());
            }
        }
        player.note_media_time(3.0);
        let mut looped = None;
        for _ in 0..40 {
            if let Some(frame) = player.take_due(3.0) {
                if frame.pts > 0.5 {
                    looped = Some(frame.pts);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let pts = looped.expect("frame from the next loop");
        assert!(pts > 0.5, "pts {pts}");
        player.set_paused(true);
        let _ = fs::remove_file(path);
    }
}
