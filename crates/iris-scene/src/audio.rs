//! Desktop capture, a Hanning window, and a log-band FFT.
//!
//! The capture callback only pushes samples. A second thread runs the FFT and
//! publishes the result through [`arc_swap::ArcSwap`]. The frame loop copies
//! that snapshot and does not take the capture lock.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use arc_swap::ArcSwap;
#[cfg(not(windows))]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

const FFT_LEN: usize = 1024;
const RING_CAP: usize = 16_384;
const MIN_HZ: f32 = 20.0;
const MAX_HZ: f32 = 20_000.0;

/// Normalized spectra. Every bin is in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioFftData {
    /// Bass, mid, and treble grouped into 16 log bands.
    pub bands_16: [f32; 16],
    /// The same energy grouped into 64 log bands.
    pub bands_64: [f32; 64],
}

impl Default for AudioFftData {
    fn default() -> Self {
        Self {
            bands_16: [0.0; 16],
            bands_64: [0.0; 64],
        }
    }
}

struct Capture {
    samples: VecDeque<f32>,
    rate: u32,
}

/// Process-wide spectrum. The first call starts capture when a device exists.
pub struct AudioEngine {
    latest: Arc<ArcSwap<AudioFftData>>,
    capture: Arc<Mutex<Capture>>,
}

impl AudioEngine {
    /// Shared engine. A missing input device leaves the spectrum at zero.
    pub fn shared() -> &'static AudioEngine {
        static ENGINE: OnceLock<AudioEngine> = OnceLock::new();
        ENGINE.get_or_init(AudioEngine::start)
    }

    /// Latest smoothed spectrum.
    pub fn snapshot(&self) -> AudioFftData {
        **self.latest.load()
    }

    /// Push mono loopback samples. Windows WASAPI calls this; cpal is not opened there.
    pub fn push_loopback(&self, samples: &[f32], rate: u32) {
        let Ok(mut guard) = self.capture.lock() else {
            return;
        };
        if rate > 0 {
            guard.rate = rate;
        }
        push_mono(&mut guard, samples);
    }

    fn start() -> Self {
        let latest = Arc::new(ArcSwap::from_pointee(AudioFftData::default()));
        let capture = Arc::new(Mutex::new(Capture {
            samples: VecDeque::with_capacity(RING_CAP),
            rate: 48_000,
        }));
        #[cfg(not(windows))]
        if let Some(stream) = open_capture(Arc::clone(&capture)) {
            // The capture callback owns the ring. The stream has to outlive
            // this function, and `cpal::Stream` is not `Sync`, so it cannot
            // sit in the process-wide engine.
            std::mem::forget(stream);
        }
        let fft_latest = Arc::clone(&latest);
        let fft_capture = Arc::clone(&capture);
        thread::Builder::new()
            .name("iris-fft".to_owned())
            .spawn(move || fft_loop(fft_capture, fft_latest))
            .ok();
        Self { latest, capture }
    }
}

/// Protocol generation, so this crate's dependency on `iris-core` stays real.
pub fn engine_protocol() -> u32 {
    iris_core::PROTOCOL_VERSION
}

#[cfg(not(windows))]
fn open_capture(capture: Arc<Mutex<Capture>>) -> Option<cpal::Stream> {
    let host = cpal::default_host();
    let device = monitor_device(&host).or_else(|| host.default_input_device())?;
    let supported = device.default_input_config().ok()?;
    if supported.sample_format() != cpal::SampleFormat::F32 {
        tracing::warn!(
            "audio capture format is {:?}; spectrum stays at zero",
            supported.sample_format()
        );
        return None;
    }
    let channels = supported.channels().max(1);
    let rate = supported.sample_rate().0;
    if let Ok(mut guard) = capture.lock() {
        guard.rate = rate;
    }
    let config: cpal::StreamConfig = supported.into();
    let stream = device
        .build_input_stream(
            &config,
            move |data: &[f32], _| push_samples(&capture, data, channels),
            |err| tracing::warn!("audio capture: {err}"),
            None,
        )
        .ok()?;
    stream.play().ok()?;
    Some(stream)
}

#[cfg(not(windows))]
fn monitor_device(host: &cpal::Host) -> Option<cpal::Device> {
    let devices = host.input_devices().ok()?;
    for device in devices {
        let name = device.name().unwrap_or_default().to_ascii_lowercase();
        if name.contains("monitor") || name.contains("loopback") {
            return Some(device);
        }
    }
    None
}

#[cfg(not(windows))]
fn push_samples(capture: &Mutex<Capture>, data: &[f32], channels: u16) {
    let Ok(mut guard) = capture.try_lock() else {
        return;
    };
    let step = usize::from(channels.max(1));
    let mut mono = Vec::new();
    for frame in data.chunks(step) {
        mono.push(frame.first().copied().unwrap_or(0.0));
    }
    push_mono(&mut guard, &mono);
}

fn push_mono(guard: &mut Capture, samples: &[f32]) {
    for sample in samples {
        if guard.samples.len() == RING_CAP {
            guard.samples.pop_front();
        }
        guard.samples.push_back(*sample);
    }
}

fn fft_loop(capture: Arc<Mutex<Capture>>, latest: Arc<ArcSwap<AudioFftData>>) {
    let mut previous = AudioFftData::default();
    loop {
        thread::sleep(Duration::from_millis(16));
        let (rate, window) = {
            let Ok(guard) = capture.lock() else {
                continue;
            };
            if guard.samples.len() < FFT_LEN {
                continue;
            }
            let mut window = [0.0f32; FFT_LEN];
            let start = guard.samples.len() - FFT_LEN;
            for (index, sample) in guard.samples.iter().skip(start).take(FFT_LEN).enumerate() {
                window[index] = *sample;
            }
            (guard.rate.max(1), window)
        };
        let measured = bands_from_samples(&window, rate);
        previous = smooth_spectrum(&previous, &measured);
        latest.store(Arc::new(previous));
    }
}

/// Window `samples`, run a forward FFT, and group the magnitudes into log bands.
pub fn bands_from_samples(samples: &[f32], sample_rate: u32) -> AudioFftData {
    let n = samples.len();
    if n < 8 || sample_rate == 0 {
        return AudioFftData::default();
    }
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let mut buffer = Vec::with_capacity(n);
    for (index, sample) in samples.iter().enumerate() {
        buffer.push(Complex::new(sample * hanning(index, n), 0.0));
    }
    fft.process(&mut buffer);
    let mut raw_16 = [0.0f32; 16];
    let mut count_16 = [0u32; 16];
    let mut raw_64 = [0.0f32; 64];
    let mut count_64 = [0u32; 64];
    let bins = n / 2;
    for bin in 1..bins {
        let freq = bin as f32 * sample_rate as f32 / n as f32;
        let magnitude = buffer[bin].norm();
        if let Some(index) = band_index(freq, 16) {
            raw_16[index] += magnitude;
            count_16[index] += 1;
        }
        if let Some(index) = band_index(freq, 64) {
            raw_64[index] += magnitude;
            count_64[index] += 1;
        }
    }
    AudioFftData {
        bands_16: normalize(raw_16, count_16),
        bands_64: normalize(raw_64, count_64),
    }
}

fn normalize<const N: usize>(raw: [f32; N], count: [u32; N]) -> [f32; N] {
    let mut out = [0.0f32; N];
    let mut peak = 0.0f32;
    for index in 0..N {
        if count[index] > 0 {
            out[index] = raw[index] / count[index] as f32;
            peak = peak.max(out[index]);
        }
    }
    if peak <= 1.0e-6 {
        return [0.0; N];
    }
    for value in &mut out {
        *value = (*value / peak).clamp(0.0, 1.0);
    }
    out
}

/// Attack when the band rises, decay when it falls.
pub fn smooth_sample(previous: f32, next: f32) -> f32 {
    let coeff = if next > previous { 0.65 } else { 0.12 };
    (previous + (next - previous) * coeff).clamp(0.0, 1.0)
}

fn smooth_spectrum(previous: &AudioFftData, next: &AudioFftData) -> AudioFftData {
    let mut out = AudioFftData::default();
    for index in 0..16 {
        out.bands_16[index] = smooth_sample(previous.bands_16[index], next.bands_16[index]);
    }
    for index in 0..64 {
        out.bands_64[index] = smooth_sample(previous.bands_64[index], next.bands_64[index]);
    }
    out
}

fn hanning(index: usize, len: usize) -> f32 {
    if len <= 1 {
        return 1.0;
    }
    let x = index as f32 / (len - 1) as f32;
    0.5 - 0.5 * (std::f32::consts::TAU * x).cos()
}

fn band_index(freq: f32, bands: usize) -> Option<usize> {
    if !(MIN_HZ..=MAX_HZ).contains(&freq) || bands == 0 {
        return None;
    }
    let t = (freq / MIN_HZ).ln() / (MAX_HZ / MIN_HZ).ln();
    let index = (t * bands as f32).floor() as usize;
    Some(index.min(bands - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_440_hz_sine_peaks_in_the_low_mid_band() {
        let rate = 44_100u32;
        let mut samples = [0.0f32; FFT_LEN];
        for (index, sample) in samples.iter_mut().enumerate() {
            let t = index as f32 / rate as f32;
            *sample = (t * 440.0 * std::f32::consts::TAU).sin();
        }
        let data = bands_from_samples(&samples, rate);
        let peak = data
            .bands_16
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(index, _)| index)
            .unwrap_or(0);
        assert!((6..=8).contains(&peak), "peak band {peak}");
        assert!(data.bands_16.iter().all(|band| (0.0..=1.0).contains(band)));
        assert!(data.bands_64.iter().all(|band| (0.0..=1.0).contains(band)));
    }

    #[test]
    fn smoothing_rises_quickly_and_falls_slowly() {
        let up = smooth_sample(0.0, 1.0);
        assert!(up > 0.5 && up < 1.0, "{up}");
        let down = smooth_sample(1.0, 0.0);
        assert!(down > 0.7 && down < 1.0, "{down}");
        assert_eq!(smooth_sample(2.0, 2.0), 1.0);
    }
}
