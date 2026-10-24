//! Default-output loopback for the scene FFT.
//!
//! The capture thread is the only owner of the COM interfaces. A failure is
//! logged by [`Loopback::start`]'s caller path inside the thread and leaves
//! the spectrum at zero. The daemon does not treat that as a startup error.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::Error;

const FLOAT_SUBTYPE: u128 = 0x0000_0003_0000_0010_8000_00aa_0038_9b71;
const PCM_SUBTYPE: u128 = 0x0000_0001_0000_0010_8000_00aa_0038_9b71;

/// Stop flag and the thread that pushes samples into [`AudioEngine`].
pub struct Loopback {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Loopback {
    /// Start loopback and call `on_samples` with mono frames.
    ///
    /// A missing device or a COM error is logged on the capture thread. The
    /// returned handle still drops cleanly, and the spectrum stays at zero
    /// when no samples arrive.
    pub fn start(on_samples: impl Fn(&[f32], u32) + Send + Sync + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let on_samples: Arc<dyn Fn(&[f32], u32) + Send + Sync> = Arc::new(on_samples);
        let thread = thread::Builder::new()
            .name("iris-wasapi".to_owned())
            .spawn(move || {
                if let Err(err) = capture_loop(&flag, &on_samples) {
                    tracing::warn!("wasapi loopback: {err}");
                }
            })
            .ok();
        Self { stop, thread }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Layout {
    channels: u16,
    rate: u32,
    bits: u16,
    block: u16,
    float: bool,
    pcm: bool,
}

#[allow(unsafe_code)]
fn capture_loop(
    stop: &AtomicBool,
    on_samples: &Arc<dyn Fn(&[f32], u32) + Send + Sync>,
) -> Result<(), Error> {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    // SAFETY: this thread calls CoInitializeEx before any other COM call and
    // CoUninitialize before it returns. COINIT_MULTITHREADED matches a worker
    // thread that does not own a window. S_FALSE means COM was already
    // initialized in this mode, which is still success.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|err| Error::Audio(format!("CoInitializeEx failed: {err}")))?;
    let run = capture_after_init(stop, on_samples);
    // SAFETY: pairs with the successful CoInitializeEx on this thread.
    unsafe { CoUninitialize() };
    run
}

#[allow(unsafe_code)]
fn capture_after_init(
    stop: &AtomicBool,
    on_samples: &Arc<dyn Fn(&[f32], u32) + Send + Sync>,
) -> Result<(), Error> {
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
        MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};

    // SAFETY: COM is initialized on this thread. MMDeviceEnumerator is the
    // system CLSID. A null outer pointer creates a fresh enumerator. The
    // returned interface is released when it drops at the end of this function.
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|err| Error::Audio(format!("device enumerator: {err}")))?;
    // SAFETY: enumerator is a live IMMDeviceEnumerator. eRender + eConsole is
    // the default playback endpoint. The device is released when dropped.
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .map_err(|err| Error::Audio(format!("default render endpoint: {err}")))?;
    // SAFETY: device is the endpoint just returned. Activate with a null
    // property bag asks for the standard IAudioClient. CLSCTX_ALL matches the
    // in-process audio engine.
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|err| Error::Audio(format!("IAudioClient: {err}")))?;
    // SAFETY: GetMixFormat allocates with CoTaskMemAlloc. The pointer is freed
    // with CoTaskMemFree on every path below, including after Initialize has
    // copied the format it needs. The header is read with read_unaligned
    // because WAVEFORMATEX is packed.
    let format = unsafe { client.GetMixFormat() }
        .map_err(|err| Error::Audio(format!("GetMixFormat: {err}")))?;
    if format.is_null() {
        return Err(Error::Audio(
            "GetMixFormat returned a null format".to_owned(),
        ));
    }
    let layout = read_layout(format);
    // SAFETY: Initialize reads `format` and does not keep the pointer. The
    // shared-mode loopback flag captures the render mix. A zero duration asks
    // for the engine's default period. The session GUID is null.
    let initialized = unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            0,
            0,
            format,
            None,
        )
    };
    // SAFETY: `format` is the block GetMixFormat allocated and is not used again.
    unsafe { CoTaskMemFree(Some(format as *const core::ffi::c_void)) };
    let layout = layout?;
    initialized.map_err(|err| Error::Audio(format!("Initialize loopback: {err}")))?;
    if layout.channels == 0 || layout.block == 0 || layout.rate == 0 {
        return Err(Error::Audio(
            "mix format has no channels or rate".to_owned(),
        ));
    }
    // SAFETY: GetService returns the capture client for this initialized
    // IAudioClient. It is used only on this thread and dropped before
    // CoUninitialize.
    let capture: IAudioCaptureClient = unsafe { client.GetService() }
        .map_err(|err| Error::Audio(format!("IAudioCaptureClient: {err}")))?;
    // SAFETY: Start begins the loopback stream owned by `client` on this thread.
    unsafe { client.Start() }.map_err(|err| Error::Audio(format!("audio Start: {err}")))?;

    while !stop.load(Ordering::Relaxed) {
        let packet = unsafe { capture.GetNextPacketSize() };
        let frames_ready = match packet {
            Ok(frames) => frames,
            Err(err) => {
                let _ = unsafe { client.Stop() };
                return Err(Error::Audio(format!("GetNextPacketSize: {err}")));
            }
        };
        if frames_ready == 0 {
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        if let Err(err) = read_packet(&capture, on_samples.as_ref(), &layout) {
            let _ = unsafe { client.Stop() };
            return Err(err);
        }
    }
    // SAFETY: Stop is the pair of Start, still on this thread, before the
    // interfaces drop.
    let _ = unsafe { client.Stop() };
    Ok(())
}

#[allow(unsafe_code)]
fn read_layout(format: *mut windows::Win32::Media::Audio::WAVEFORMATEX) -> Result<Layout, Error> {
    use windows::core::GUID;
    use windows::Win32::Media::Audio::{WAVEFORMATEX, WAVEFORMATEXTENSIBLE};

    // SAFETY: `format` is non-null and points at a CoTaskMemAlloc block that
    // remains allocated until the caller frees it after this function returns.
    // The copy does not hold a reference into the packed struct.
    let header: WAVEFORMATEX = unsafe { format.read_unaligned() };
    let channels = header.nChannels;
    let rate = header.nSamplesPerSec;
    let bits = header.wBitsPerSample;
    let block = header.nBlockAlign;
    let tag = header.wFormatTag;
    let mut float = tag == 3;
    let mut pcm = tag == 1;
    if tag == 0xFFFE {
        let extra = usize::from(header.cbSize);
        if extra + std::mem::size_of::<WAVEFORMATEX>()
            >= std::mem::size_of::<WAVEFORMATEXTENSIBLE>()
        {
            // SAFETY: the extensible header is the same allocation, and cbSize
            // says the subtype fields are present. The read is unaligned.
            let ext: WAVEFORMATEXTENSIBLE =
                unsafe { (format as *const WAVEFORMATEXTENSIBLE).read_unaligned() };
            let subtype = ext.SubFormat;
            float = subtype == GUID::from_u128(FLOAT_SUBTYPE);
            pcm = subtype == GUID::from_u128(PCM_SUBTYPE);
        }
    }
    let _ = bits;
    Ok(Layout {
        channels,
        rate,
        bits,
        block,
        float,
        pcm,
    })
}

#[allow(unsafe_code)]
fn read_packet(
    capture: &windows::Win32::Media::Audio::IAudioCaptureClient,
    on_samples: &dyn Fn(&[f32], u32),
    layout: &Layout,
) -> Result<(), Error> {
    let mut data = std::ptr::null_mut();
    let mut frames = 0u32;
    let mut flags = 0u32;
    // SAFETY: GetBuffer's pointer is valid until ReleaseBuffer on this same
    // client. The frame count and flags are local. Device-position pointers
    // are omitted. The bytes are copied into `mono` before ReleaseBuffer.
    unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }
        .map_err(|err| Error::Audio(format!("GetBuffer: {err}")))?;
    let silent = flags & 2 != 0;
    let bytes = usize::from(layout.block).saturating_mul(frames as usize);
    let mono = if silent || data.is_null() {
        vec![0.0; frames as usize]
    } else {
        // SAFETY: `data` points at `bytes` of sample memory owned by the
        // capture client until ReleaseBuffer below. The slice is not stored.
        let slice = unsafe { std::slice::from_raw_parts(data, bytes) };
        mono_from(slice, frames, layout)
    };
    // SAFETY: ReleaseBuffer returns the packet acquired above. `frames` is the
    // count GetBuffer reported. `mono` already owns its copy.
    let released = unsafe { capture.ReleaseBuffer(frames) };
    on_samples(&mono, layout.rate);
    released.map_err(|err| Error::Audio(format!("ReleaseBuffer: {err}")))
}

fn mono_from(bytes: &[u8], frames: u32, layout: &Layout) -> Vec<f32> {
    let channels = usize::from(layout.channels.max(1));
    let mut mono = Vec::with_capacity(frames as usize);
    if layout.float && layout.bits == 32 {
        let samples: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        for frame in samples.chunks(channels) {
            let sum: f32 = frame.iter().copied().sum();
            mono.push(sum / channels as f32);
        }
    } else if layout.pcm && layout.bits == 16 {
        let samples: Vec<f32> = bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0)
            .collect();
        for frame in samples.chunks(channels) {
            let sum: f32 = frame.iter().copied().sum();
            mono.push(sum / channels as f32);
        }
    }
    mono
}
