//! One process-wide wgpu instance, device, and queue.
//!
//! The device is created on the first successful attach, using that window as
//! the compatible surface. Later outputs call [`RenderContext::attach`] and
//! share the device. `request_device` may be called only once per adapter.

use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::error::Error;
use crate::surface::WallpaperSurface;

/// Shared GPU context for every wallpaper surface.
///
/// `wgpu` 23 device and queue handles are not `Clone`. `Arc` keeps one device
/// for every output.
pub struct RenderContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
}

impl RenderContext {
    /// Create the instance, device, and the first surface.
    pub fn open<W>(window: W, width: u32, height: u32) -> Result<(Self, WallpaperSurface), Error>
    where
        W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
    {
        let (width, height) = nonzero(width, height)?;
        let backends = if cfg!(windows) {
            wgpu::Backends::DX12 | wgpu::Backends::VULKAN
        } else {
            wgpu::Backends::VULKAN | wgpu::Backends::GL
        };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window)
            .map_err(|err| Error::Surface(err.to_string()))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .ok_or(Error::AdapterMissing)?;
        let descriptor = wgpu::DeviceDescriptor {
            label: Some("iris"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            memory_hints: wgpu::MemoryHints::Performance,
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&descriptor, None))
            .map_err(|err| Error::Device(err.to_string()))?;
        let device = Arc::new(device);
        let queue = Arc::new(queue);
        let wallpaper = WallpaperSurface::new(
            Arc::clone(&device),
            Arc::clone(&queue),
            surface,
            &adapter,
            width,
            height,
        )?;
        let context = Self {
            instance,
            adapter,
            device,
            queue,
        };
        Ok((context, wallpaper))
    }

    /// Create another surface on the existing device.
    pub fn attach<W>(&self, window: W, width: u32, height: u32) -> Result<WallpaperSurface, Error>
    where
        W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
    {
        let (width, height) = nonzero(width, height)?;
        let surface = self
            .instance
            .create_surface(window)
            .map_err(|err| Error::Surface(err.to_string()))?;
        WallpaperSurface::new(
            Arc::clone(&self.device),
            Arc::clone(&self.queue),
            surface,
            &self.adapter,
            width,
            height,
        )
    }

    /// The shared device.
    pub fn device(&self) -> &wgpu::Device {
        self.device.as_ref()
    }

    /// The shared queue.
    pub fn queue(&self) -> &wgpu::Queue {
        self.queue.as_ref()
    }
}

fn nonzero(width: u32, height: u32) -> Result<(u32, u32), Error> {
    if width == 0 || height == 0 {
        return Err(Error::EmptyTarget);
    }
    Ok((width, height))
}
