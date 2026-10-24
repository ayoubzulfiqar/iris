//! One swapchain, one textured triangle, and the wallpaper uniform buffer.

use std::sync::Arc;

use crate::error::Error;
use crate::scene_draw::{SceneDraw, SceneGpu};
use crate::shader::SHADER;
use crate::uniform::{placement, Uniforms, UNIFORM_SIZE};
use crate::yuv::{sample_mode, YuvFrame, YuvPlane, MODE_RGBA};
use crate::Fit;
use iris_pkg::DecodedTexture;

/// Values that change every presented frame. The pointer lives on the surface.
pub struct DrawParams {
    /// Seconds since the wallpaper started.
    pub time: f32,
    /// Seconds since the previous presented frame, already scaled by playback rate.
    pub delta_time: f32,
    /// How the image meets the output.
    pub fit: Fit,
    /// Image width in pixels.
    pub image_width: u32,
    /// Image height in pixels.
    pub image_height: u32,
}

/// A configured swapchain and the resources for one output.
pub struct WallpaperSurface {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    plane_y: wgpu::Texture,
    plane_u: wgpu::Texture,
    plane_v: wgpu::Texture,
    texture_bind: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    picture: PictureSize,
    color_mode: f32,
    pointer: [f32; 2],
    scene: Option<SceneGpu>,
}

enum PictureSize {
    Rgba { width: u32, height: u32 },
    Yuv { width: u32, height: u32, mode: f32 },
}

impl WallpaperSurface {
    pub(crate) fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        surface: wgpu::Surface<'static>,
        adapter: &wgpu::Adapter,
        width: u32,
        height: u32,
    ) -> Result<Self, Error> {
        let capabilities = surface.get_capabilities(adapter);
        let format = choose_format(&capabilities.formats).ok_or(Error::Format)?;
        let alpha = choose_alpha(&capabilities.alpha_modes);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: alpha,
            view_formats: Vec::new(),
        };
        surface.configure(&device, &config);

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("iris-uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("iris-texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("iris-uniforms"),
            size: UNIFORM_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("iris-uniforms"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform_buffer,
                    offset: 0,
                    size: None,
                }),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("iris-linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let (plane_y, plane_u, plane_v, texture_bind) =
            placeholder_texture(&device, &queue, &texture_layout, &sampler)?;
        let pipeline = build_pipeline(&device, &uniform_layout, &texture_layout, format);

        Ok(Self {
            device,
            queue,
            surface,
            config,
            pipeline,
            uniform_buffer,
            uniform_bind,
            texture_layout,
            plane_y,
            plane_u,
            plane_v,
            texture_bind,
            sampler,
            picture: PictureSize::Rgba {
                width: 1,
                height: 1,
            },
            color_mode: MODE_RGBA,
            pointer: [0.0, 0.0],
            scene: None,
        })
    }

    /// Swapchain size in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Replace the scene layer textures. Index 0 is the first image.
    pub fn upload_scene_images(&mut self, images: &[&DecodedTexture]) -> Result<(), Error> {
        self.ensure_scene();
        let device = Arc::clone(&self.device);
        let queue = Arc::clone(&self.queue);
        let gpu = self
            .scene
            .as_mut()
            .ok_or_else(|| Error::Gpu("scene pipeline is missing".to_owned()))?;
        gpu.upload_images(&device, &queue, images)
    }

    /// Draw scene sprites and particles into the swapchain.
    pub fn draw_scene(&mut self, draw: &SceneDraw<'_>) -> Result<(), Error> {
        self.ensure_scene();
        let frame = self.acquire()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let device = Arc::clone(&self.device);
        let queue = Arc::clone(&self.queue);
        let command = {
            let gpu = self
                .scene
                .as_mut()
                .ok_or_else(|| Error::Gpu("scene pipeline is missing".to_owned()))?;
            gpu.record(&device, &queue, &view, draw)
        };
        self.queue.submit([command]);
        let suboptimal = frame.suboptimal;
        frame.present();
        if suboptimal {
            self.surface.configure(&self.device, &self.config);
        }
        Ok(())
    }

    fn ensure_scene(&mut self) {
        if self.scene.is_none() {
            self.scene = Some(SceneGpu::new(&self.device, &self.queue, self.config.format));
        }
    }

    /// Reconfigure the swapchain. The pipeline stays in place.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), Error> {
        let width = width.max(1);
        let height = height.max(1);
        if self.config.width == width && self.config.height == height {
            return Ok(());
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        Ok(())
    }

    /// Replace the sampled image. A size change allocates a new texture.
    pub fn upload(&mut self, image: &DecodedTexture) -> Result<(), Error> {
        let limit = self.device.limits().max_texture_dimension_2d;
        if image.width > limit || image.height > limit {
            return Err(Error::Upload(format!(
                "{}x{} exceeds the device limit of {limit}",
                image.width, image.height
            )));
        }
        let (bytes, stride) = pad_rows(image.width, image.height, &image.rgba)?;
        let same = matches!(
            self.picture,
            PictureSize::Rgba { width, height } if width == image.width && height == image.height
        );
        if !same {
            self.plane_y = make_texture(
                &self.device,
                image.width,
                image.height,
                wgpu::TextureFormat::Rgba8UnormSrgb,
                "iris-rgba",
            );
            self.plane_u = make_texture(&self.device, 1, 1, wgpu::TextureFormat::R8Unorm, "iris-u");
            self.plane_v = make_texture(&self.device, 1, 1, wgpu::TextureFormat::R8Unorm, "iris-v");
            self.rebuild_bind();
            self.picture = PictureSize::Rgba {
                width: image.width,
                height: image.height,
            };
        }
        self.color_mode = MODE_RGBA;
        write_plane(
            &self.queue,
            &self.plane_y,
            image.width,
            image.height,
            stride,
            &bytes,
        );
        Ok(())
    }

    /// Replace the sampled picture with YUV or NV12 planes. The shader converts them.
    pub fn upload_video(&mut self, frame: &YuvFrame<'_>) -> Result<(), Error> {
        let mode = sample_mode(frame);
        let (y, chroma_bytes) = match frame {
            YuvFrame::Yuv420 { y, u, v, .. } => {
                self.stage_yuv(y, mode, u.width, u.height, v.width, v.height, false)?;
                write_sampled(&self.queue, &self.plane_y, y, 1)?;
                write_sampled(&self.queue, &self.plane_u, u, 1)?;
                write_sampled(&self.queue, &self.plane_v, v, 1)?;
                return Ok(());
            }
            YuvFrame::Nv12 { y, uv, .. } => (y, uv),
        };
        self.stage_yuv(y, mode, chroma_bytes.width, chroma_bytes.height, 1, 1, true)?;
        write_sampled(&self.queue, &self.plane_y, y, 1)?;
        write_sampled(&self.queue, &self.plane_u, chroma_bytes, 2)?;
        Ok(())
    }

    fn stage_yuv(
        &mut self,
        y: &YuvPlane<'_>,
        mode: f32,
        u_width: u32,
        u_height: u32,
        v_width: u32,
        v_height: u32,
        nv12: bool,
    ) -> Result<(), Error> {
        let limit = self.device.limits().max_texture_dimension_2d;
        if y.width > limit || y.height > limit {
            return Err(Error::Upload(format!(
                "{}x{} exceeds the device limit of {limit}",
                y.width, y.height
            )));
        }
        let same = matches!(
            self.picture,
            PictureSize::Yuv { width, height, mode: current }
                if width == y.width && height == y.height && (current - mode).abs() < f32::EPSILON
        );
        if !same {
            self.plane_y = make_texture(
                &self.device,
                y.width,
                y.height,
                wgpu::TextureFormat::R8Unorm,
                "iris-y",
            );
            if nv12 {
                self.plane_u = make_texture(
                    &self.device,
                    u_width.max(1),
                    u_height.max(1),
                    wgpu::TextureFormat::Rg8Unorm,
                    "iris-uv",
                );
                self.plane_v =
                    make_texture(&self.device, 1, 1, wgpu::TextureFormat::R8Unorm, "iris-v");
            } else {
                self.plane_u = make_texture(
                    &self.device,
                    u_width.max(1),
                    u_height.max(1),
                    wgpu::TextureFormat::R8Unorm,
                    "iris-u",
                );
                self.plane_v = make_texture(
                    &self.device,
                    v_width.max(1),
                    v_height.max(1),
                    wgpu::TextureFormat::R8Unorm,
                    "iris-v",
                );
            }
            self.rebuild_bind();
            self.picture = PictureSize::Yuv {
                width: y.width,
                height: y.height,
                mode,
            };
        }
        self.color_mode = mode;
        Ok(())
    }

    fn rebuild_bind(&mut self) {
        self.texture_bind = bind_planes(
            &self.device,
            &self.texture_layout,
            &self.sampler,
            &self.plane_y,
            &self.plane_u,
            &self.plane_v,
        );
    }

    /// Pointer in output pixels. The daemon leaves this at the origin.
    pub fn set_pointer(&mut self, pointer: [f32; 2]) {
        self.pointer = pointer;
    }

    /// Write uniforms, draw the triangle, and present.
    ///
    /// `Outdated` and `Lost` reconfigure the swapchain and try once more.
    /// A suboptimal acquire is presented, then the swapchain is reconfigured.
    pub fn draw(&mut self, params: DrawParams) -> Result<(), Error> {
        self.write_uniforms(&params)?;
        let frame = self.acquire()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("iris-frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("iris-wallpaper"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.texture_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        let suboptimal = frame.suboptimal;
        frame.present();
        if suboptimal {
            self.surface.configure(&self.device, &self.config);
        }
        Ok(())
    }

    fn write_uniforms(&self, params: &DrawParams) -> Result<(), Error> {
        let placed = placement(
            params.fit,
            params.image_width,
            params.image_height,
            self.config.width,
            self.config.height,
        );
        let uniforms = Uniforms {
            time: params.time,
            delta_time: params.delta_time,
            mouse: self.pointer,
            resolution: [self.config.width as f32, self.config.height as f32],
            padding: [self.color_mode, 0.0],
            mvp: placed.mvp,
            uv_scale: placed.uv_scale,
            uv_offset: placed.uv_offset,
        };
        let bytes = uniforms.to_bytes()?;
        self.queue.write_buffer(&self.uniform_buffer, 0, &bytes);
        Ok(())
    }

    fn acquire(&mut self) -> Result<wgpu::SurfaceTexture, Error> {
        match self.surface.get_current_texture() {
            Ok(texture) => Ok(texture),
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                self.surface.configure(&self.device, &self.config);
                self.surface
                    .get_current_texture()
                    .map_err(|err| Error::Surface(err.to_string()))
            }
            Err(err) => Err(Error::Surface(err.to_string())),
        }
    }
}

fn choose_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    const PREFERRED: [wgpu::TextureFormat; 2] = [
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ];
    const FALLBACK: [wgpu::TextureFormat; 2] = [
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Rgba8Unorm,
    ];
    for format in PREFERRED {
        if formats.contains(&format) {
            return Some(format);
        }
    }
    for format in FALLBACK {
        if formats.contains(&format) {
            return Some(format);
        }
    }
    None
}

fn choose_alpha(modes: &[wgpu::CompositeAlphaMode]) -> wgpu::CompositeAlphaMode {
    for mode in [
        wgpu::CompositeAlphaMode::Opaque,
        wgpu::CompositeAlphaMode::Auto,
        wgpu::CompositeAlphaMode::Inherit,
    ] {
        if modes.contains(&mode) {
            return mode;
        }
    }
    modes
        .first()
        .copied()
        .unwrap_or(wgpu::CompositeAlphaMode::Auto)
}

fn build_pipeline(
    device: &wgpu::Device,
    uniform_layout: &wgpu::BindGroupLayout,
    texture_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("iris-wallpaper"),
        source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(SHADER)),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("iris-wallpaper"),
        bind_group_layouts: &[uniform_layout, texture_layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("iris-wallpaper"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    })
}

fn placeholder_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
) -> Result<(wgpu::Texture, wgpu::Texture, wgpu::Texture, wgpu::BindGroup), Error> {
    let plane_y = make_texture(
        device,
        1,
        1,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        "iris-rgba",
    );
    let plane_u = make_texture(device, 1, 1, wgpu::TextureFormat::R8Unorm, "iris-u");
    let plane_v = make_texture(device, 1, 1, wgpu::TextureFormat::R8Unorm, "iris-v");
    // `bytes_per_row` is 256, so the upload buffer has to cover that whole row.
    let mut placeholder = vec![0u8; 256];
    placeholder[..4].copy_from_slice(&[255, 0, 255, 255]);
    write_plane(queue, &plane_y, 1, 1, 256, &placeholder);
    let bind = bind_planes(device, layout, sampler, &plane_y, &plane_u, &plane_v);
    Ok((plane_y, plane_u, plane_v, bind))
}

fn make_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn bind_planes(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    plane_y: &wgpu::Texture,
    plane_u: &wgpu::Texture,
    plane_v: &wgpu::Texture,
) -> wgpu::BindGroup {
    let y = plane_y.create_view(&wgpu::TextureViewDescriptor::default());
    let u = plane_u.create_view(&wgpu::TextureViewDescriptor::default());
    let v = plane_v.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("iris-image"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&y),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&u),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&v),
            },
        ],
    })
}

fn write_plane(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    stride: u32,
    bytes: &[u8],
) {
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(stride),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

fn write_sampled(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    plane: &YuvPlane<'_>,
    texel_bytes: usize,
) -> Result<(), Error> {
    let width_bytes = usize::try_from(plane.width)
        .ok()
        .and_then(|width| width.checked_mul(texel_bytes))
        .ok_or_else(|| Error::Upload(format!("plane width {} overflows", plane.width)))?;
    let (bytes, stride) = align_rows(width_bytes, plane.height, plane.stride, plane.data)?;
    let stride = u32::try_from(stride)
        .map_err(|_| Error::Upload(format!("stride {stride} does not fit in u32")))?;
    write_plane(
        queue,
        texture,
        plane.width.max(1),
        plane.height.max(1),
        stride,
        &bytes,
    );
    Ok(())
}

fn align_rows(
    width_bytes: usize,
    height: u32,
    stride: usize,
    data: &[u8],
) -> Result<(Vec<u8>, usize), Error> {
    if width_bytes == 0 || height == 0 {
        return Err(Error::Upload("plane has a zero dimension".to_owned()));
    }
    if stride < width_bytes {
        return Err(Error::Upload(format!(
            "stride {stride} is shorter than {width_bytes}"
        )));
    }
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let padded = width_bytes.div_ceil(align).saturating_mul(align);
    let rows = usize::try_from(height)
        .map_err(|_| Error::Upload(format!("height {height} does not fit in usize")))?;
    let bytes = padded
        .checked_mul(rows)
        .ok_or_else(|| Error::Upload("padded plane overflows".to_owned()))?;
    let mut out = vec![0u8; bytes];
    for y in 0..rows {
        let src = y
            .checked_mul(stride)
            .ok_or_else(|| Error::Upload("source row overflows".to_owned()))?;
        let dst = y
            .checked_mul(padded)
            .ok_or_else(|| Error::Upload("padded row overflows".to_owned()))?;
        let src_end = src
            .checked_add(width_bytes)
            .ok_or_else(|| Error::Upload("source row end overflows".to_owned()))?;
        let dst_end = dst
            .checked_add(width_bytes)
            .ok_or_else(|| Error::Upload("padded row end overflows".to_owned()))?;
        let source = data.get(src..src_end).ok_or_else(|| {
            Error::Upload(format!(
                "plane is {} bytes, row {y} needs {src_end}",
                data.len()
            ))
        })?;
        let dest = out.get_mut(dst..dst_end).ok_or_else(|| {
            Error::Upload(format!(
                "padded plane is {bytes} bytes, row {y} needs {dst_end}"
            ))
        })?;
        dest.copy_from_slice(source);
    }
    Ok((out, padded))
}

fn pad_rows(width: u32, height: u32, rgba: &[u8]) -> Result<(Vec<u8>, u32), Error> {
    let row = usize::try_from(width)
        .ok()
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| Error::Upload(format!("row stride for width {width} overflows")))?;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let padded = row.div_ceil(align).saturating_mul(align);
    let rows = usize::try_from(height)
        .map_err(|_| Error::Upload(format!("height {height} does not fit in usize")))?;
    let bytes = padded
        .checked_mul(rows)
        .ok_or_else(|| Error::Upload("padded image overflows".to_owned()))?;
    let stride = u32::try_from(padded)
        .map_err(|_| Error::Upload(format!("padded stride {padded} does not fit in u32")))?;
    let mut out = vec![0u8; bytes];
    for y in 0..rows {
        let src = y
            .checked_mul(row)
            .ok_or_else(|| Error::Upload("source row overflows".to_owned()))?;
        let dst = y
            .checked_mul(padded)
            .ok_or_else(|| Error::Upload("padded row overflows".to_owned()))?;
        let src_end = src
            .checked_add(row)
            .ok_or_else(|| Error::Upload("source row end overflows".to_owned()))?;
        let dst_end = dst
            .checked_add(row)
            .ok_or_else(|| Error::Upload("padded row end overflows".to_owned()))?;
        let source = rgba.get(src..src_end).ok_or_else(|| {
            Error::Upload(format!(
                "rgba buffer is {} bytes, row {y} needs {src_end}",
                rgba.len()
            ))
        })?;
        let dest = out.get_mut(dst..dst_end).ok_or_else(|| {
            Error::Upload(format!(
                "padded buffer is {bytes} bytes, row {y} needs {dst_end}"
            ))
        })?;
        dest.copy_from_slice(source);
    }
    Ok((out, stride))
}
