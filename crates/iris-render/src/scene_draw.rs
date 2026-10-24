//! Instanced quads for scene layers and particles.
//!
//! `SceneUniforms` matches the Wallpaper Engine block: tightly packed `f32`s.
//! The shader reads that block from a read-only storage buffer so the arrays
//! stay at a stride of 4. A uniform-address-space `array<f32>` would stride
//! each element by 16 bytes and no longer match this struct.

use crate::error::Error;
use iris_pkg::DecodedTexture;

/// Logical size of [`SceneUniforms`]. The GPU buffer is rounded up to 16 bytes.
pub const SCENE_UNIFORM_SIZE: usize = 344;

/// Size of one [`TelemetryUniforms`] block.
pub const TELEMETRY_UNIFORM_SIZE: usize = 96;

/// Scene block, previous telemetry, current telemetry, and a blend factor.
pub const FRAME_UNIFORM_SIZE: usize = SCENE_UNIFORM_SIZE + TELEMETRY_UNIFORM_SIZE * 2 + 16;

const INSTANCE_STRIDE: usize = 96;
const MAX_INSTANCES: usize = 10_000;

/// How a scene layer composites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerBlend {
    /// Standard alpha.
    Alpha,
    /// One-one, for glow.
    Additive,
    /// Destination-color multiply.
    Multiply,
    /// Screen.
    Screen,
}

/// One instanced range inside [`SceneDraw::instances`].
pub struct DrawSpan {
    /// Blend pipeline.
    pub blend: LayerBlend,
    /// Index into the uploaded scene textures. `u32::MAX` is the white pixel.
    pub texture: u32,
    /// First instance in the shared buffer.
    pub start: u32,
    /// Instances in this range.
    pub count: u32,
}

/// One frame of scene draws. `instances` is tightly packed 96-byte records.
pub struct SceneDraw<'a> {
    /// Seconds since the wallpaper started.
    pub time: f32,
    /// Seconds since the previous presented frame.
    pub delta_time: f32,
    /// Pointer in output pixels.
    pub pointer: [f32; 2],
    /// Swapchain size in physical pixels.
    pub viewport: [f32; 2],
    /// Smoothed 16-band spectrum, each bin in `0.0..=1.0`.
    pub audio_spectrum_16: [f32; 16],
    /// Smoothed 64-band spectrum, each bin in `0.0..=1.0`.
    pub audio_spectrum_64: [f32; 64],
    /// Clear color, straight RGBA.
    pub clear: [f32; 4],
    /// Concatenated instance records.
    pub instances: &'a [u8],
    /// Draw ranges into `instances`.
    pub spans: &'a [DrawSpan],
    /// Previous and current hardware sample, blended in the shader.
    pub telemetry: TelemetryFrame,
}

/// CPU copy of the scene uniform block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneUniforms {
    /// Seconds since the wallpaper started.
    pub time: f32,
    /// Seconds since the previous presented frame.
    pub delta_time: f32,
    /// Pointer in output pixels.
    pub pointer_position: [f32; 2],
    /// Swapchain size in physical pixels.
    pub viewport_size: [f32; 2],
    /// 16-band spectrum.
    pub audio_spectrum_16: [f32; 16],
    /// 64-band spectrum.
    pub audio_spectrum_64: [f32; 64],
}

impl SceneUniforms {
    /// Pack the block in field order.
    pub fn to_bytes(&self) -> [u8; SCENE_UNIFORM_SIZE] {
        let mut bytes = [0u8; SCENE_UNIFORM_SIZE];
        write_f32(&mut bytes, 0, self.time);
        write_f32(&mut bytes, 4, self.delta_time);
        write_f32(&mut bytes, 8, self.pointer_position[0]);
        write_f32(&mut bytes, 12, self.pointer_position[1]);
        write_f32(&mut bytes, 16, self.viewport_size[0]);
        write_f32(&mut bytes, 20, self.viewport_size[1]);
        for (index, value) in self.audio_spectrum_16.iter().enumerate() {
            write_f32(&mut bytes, 24 + index * 4, *value);
        }
        for (index, value) in self.audio_spectrum_64.iter().enumerate() {
            write_f32(&mut bytes, 88 + index * 4, *value);
        }
        bytes
    }
}

fn write_f32(bytes: &mut [u8], offset: usize, value: f32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Hardware gauges for `g_Telemetry`. Packed `f32`s, 96 bytes.
///
/// `bytemuck::Pod` would require `unsafe impl` in this crate. The bytes are
/// written field by field so the layout matches the WGSL storage struct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TelemetryUniforms {
    /// Average CPU, `0.0..=1.0`.
    pub cpu_global: f32,
    /// Used RAM over installed RAM.
    pub ram_fraction: f32,
    /// Receive rate, `0.0..=1.0`.
    pub net_rx_rate: f32,
    /// Transmit rate, `0.0..=1.0`.
    pub net_tx_rate: f32,
    /// Battery fraction. `1.0` when no pack is present.
    pub battery_level: f32,
    /// `1` while discharging.
    pub is_on_battery: u32,
    /// Unused. Kept so the core array starts at a 16-byte boundary inside the block.
    pub padding: [f32; 2],
    /// Up to 16 logical processors.
    pub cpu_cores: [f32; 16],
}

impl TelemetryUniforms {
    /// Idle desktop: no load, treated as plugged in.
    pub fn neutral() -> Self {
        Self {
            cpu_global: 0.0,
            ram_fraction: 0.0,
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            battery_level: 1.0,
            is_on_battery: 0,
            padding: [0.0, 0.0],
            cpu_cores: [0.0; 16],
        }
    }

    /// Pack the block in field order.
    pub fn to_bytes(self) -> [u8; TELEMETRY_UNIFORM_SIZE] {
        let mut bytes = [0u8; TELEMETRY_UNIFORM_SIZE];
        write_f32(&mut bytes, 0, self.cpu_global);
        write_f32(&mut bytes, 4, self.ram_fraction);
        write_f32(&mut bytes, 8, self.net_rx_rate);
        write_f32(&mut bytes, 12, self.net_tx_rate);
        write_f32(&mut bytes, 16, self.battery_level);
        write_u32(&mut bytes, 20, self.is_on_battery);
        write_f32(&mut bytes, 24, self.padding[0]);
        write_f32(&mut bytes, 28, self.padding[1]);
        for (index, value) in self.cpu_cores.iter().enumerate() {
            write_f32(&mut bytes, 32 + index * 4, *value);
        }
        bytes
    }
}

/// The two polls the shader blends, plus how far the frame is toward `current`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TelemetryFrame {
    /// Sample from the previous poll.
    pub previous: TelemetryUniforms,
    /// Latest poll.
    pub current: TelemetryUniforms,
    /// `0.0` at the latest poll, `1.0` when the next poll is due.
    pub blend: f32,
}

impl TelemetryFrame {
    /// Both samples idle and fully blended.
    pub fn neutral() -> Self {
        Self {
            previous: TelemetryUniforms::neutral(),
            current: TelemetryUniforms::neutral(),
            blend: 1.0,
        }
    }
}

/// Scene block followed by the telemetry pair.
pub fn pack_frame(scene: &SceneUniforms, telemetry: &TelemetryFrame) -> [u8; FRAME_UNIFORM_SIZE] {
    let mut bytes = [0u8; FRAME_UNIFORM_SIZE];
    let scene_bytes = scene.to_bytes();
    bytes[..SCENE_UNIFORM_SIZE].copy_from_slice(&scene_bytes);
    let previous = telemetry.previous.to_bytes();
    let current = telemetry.current.to_bytes();
    let current_at = SCENE_UNIFORM_SIZE + TELEMETRY_UNIFORM_SIZE;
    bytes[SCENE_UNIFORM_SIZE..current_at].copy_from_slice(&previous);
    bytes[current_at..current_at + TELEMETRY_UNIFORM_SIZE].copy_from_slice(&current);
    let blend_at = current_at + TELEMETRY_UNIFORM_SIZE;
    write_f32(&mut bytes, blend_at, telemetry.blend);
    bytes
}

const SCENE_SHADER: &str = r#"
struct TelemetryUniforms {
    cpu_global: f32,
    ram_fraction: f32,
    net_rx_rate: f32,
    net_tx_rate: f32,
    battery_level: f32,
    is_on_battery: u32,
    padding: vec2<f32>,
    cpu_cores: array<f32, 16>,
}

struct SceneUniforms {
    time: f32,
    delta_time: f32,
    pointer_position: vec2<f32>,
    viewport_size: vec2<f32>,
    audio_spectrum_16: array<f32, 16>,
    audio_spectrum_64: array<f32, 64>,
    telemetry_previous: TelemetryUniforms,
    telemetry_current: TelemetryUniforms,
    telemetry_blend: f32,
    telemetry_pad0: f32,
    telemetry_pad1: f32,
    telemetry_pad2: f32,
}

fn g_Telemetry() -> TelemetryUniforms {
    let t = clamp(scene.telemetry_blend, 0.0, 1.0);
    var out: TelemetryUniforms;
    out.cpu_global = mix(scene.telemetry_previous.cpu_global, scene.telemetry_current.cpu_global, t);
    out.ram_fraction = mix(scene.telemetry_previous.ram_fraction, scene.telemetry_current.ram_fraction, t);
    out.net_rx_rate = mix(scene.telemetry_previous.net_rx_rate, scene.telemetry_current.net_rx_rate, t);
    out.net_tx_rate = mix(scene.telemetry_previous.net_tx_rate, scene.telemetry_current.net_tx_rate, t);
    out.battery_level = mix(scene.telemetry_previous.battery_level, scene.telemetry_current.battery_level, t);
    out.is_on_battery = scene.telemetry_current.is_on_battery;
    out.padding = vec2<f32>(0.0, 0.0);
    for (var i = 0u; i < 16u; i = i + 1u) {
        out.cpu_cores[i] = mix(scene.telemetry_previous.cpu_cores[i], scene.telemetry_current.cpu_cores[i], t);
    }
    return out;
}

@group(0) @binding(0) var<storage, read> scene: SceneUniforms;
@group(1) @binding(0) var layer_tex: texture_2d<f32>;
@group(1) @binding(1) var layer_samp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
}

@vertex
fn vs_main(
    @builtin(vertex_index) index: u32,
    @location(0) c0: vec4<f32>,
    @location(1) c1: vec4<f32>,
    @location(2) c2: vec4<f32>,
    @location(3) c3: vec4<f32>,
    @location(4) tint: vec4<f32>,
    @location(5) uv_rect: vec4<f32>,
) -> VsOut {
    var corner = vec2<f32>(-0.5, -0.5);
    var uv = vec2<f32>(0.0, 1.0);
    if (index == 1u) {
        corner = vec2<f32>(0.5, -0.5);
        uv = vec2<f32>(1.0, 1.0);
    } else if (index == 2u) {
        corner = vec2<f32>(0.5, 0.5);
        uv = vec2<f32>(1.0, 0.0);
    } else if (index == 3u) {
        corner = vec2<f32>(-0.5, 0.5);
        uv = vec2<f32>(0.0, 0.0);
    }
    let model = mat4x4<f32>(c0, c1, c2, c3);
    let placed = model * vec4<f32>(corner, 0.0, 1.0);
    let width = max(scene.viewport_size.x, 1.0);
    let height = max(scene.viewport_size.y, 1.0);
    var out: VsOut;
    out.clip = vec4<f32>(placed.x / width * 2.0, placed.y / height * 2.0, 0.0, 1.0);
    out.uv = mix(uv_rect.xy, uv_rect.zw, uv);
    let pulse = scene.audio_spectrum_16[0] + scene.audio_spectrum_64[0] * 0.0;
    let live = scene.time * 0.0 + scene.delta_time * 0.0 + scene.pointer_position.x * 0.0;
    let gauges = g_Telemetry();
    let hud = gauges.cpu_global + gauges.ram_fraction + gauges.net_rx_rate + gauges.net_tx_rate
        + gauges.battery_level + f32(gauges.is_on_battery) + gauges.cpu_cores[0];
    out.tint = vec4<f32>(tint.rgb * (1.0 + 0.15 * pulse), tint.a + live + hud * 0.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(layer_tex, layer_samp, in.uv);
    return vec4<f32>(sampled.rgb * in.tint.rgb, sampled.a * in.tint.a);
}
"#;

pub(crate) struct SceneGpu {
    pipelines: [wgpu::RenderPipeline; 4],
    uniform_buffer: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    instance_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    white: wgpu::Texture,
    white_bind: wgpu::BindGroup,
    textures: Vec<wgpu::Texture>,
    binds: Vec<wgpu::BindGroup>,
}

impl SceneGpu {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("iris-scene-uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("iris-scene-texture"),
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
            ],
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("iris-scene-uniforms"),
            size: FRAME_UNIFORM_SIZE as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("iris-scene-uniforms"),
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
            label: Some("iris-scene"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let white = make_rgba(device, 1, 1, "iris-scene-white");
        let mut pixel = vec![0u8; 256];
        pixel[..4].copy_from_slice(&[255, 255, 255, 255]);
        write_rgba(queue, &white, 1, 1, 256, &pixel);
        let white_bind = bind_texture(device, &texture_layout, &sampler, &white);
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("iris-scene-instances"),
            size: (MAX_INSTANCES * INSTANCE_STRIDE) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("iris-scene-indices"),
            size: 12,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&index_buffer, 0, bytemuck_indices());
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("iris-scene"),
            source: wgpu::ShaderSource::Wgsl(SCENE_SHADER.into()),
        });
        let pipelines = [
            pipeline(
                device,
                &module,
                &uniform_layout,
                &texture_layout,
                format,
                LayerBlend::Alpha,
            ),
            pipeline(
                device,
                &module,
                &uniform_layout,
                &texture_layout,
                format,
                LayerBlend::Additive,
            ),
            pipeline(
                device,
                &module,
                &uniform_layout,
                &texture_layout,
                format,
                LayerBlend::Multiply,
            ),
            pipeline(
                device,
                &module,
                &uniform_layout,
                &texture_layout,
                format,
                LayerBlend::Screen,
            ),
        ];
        Self {
            pipelines,
            uniform_buffer,
            uniform_bind,
            texture_layout,
            instance_buffer,
            index_buffer,
            sampler,
            white,
            white_bind,
            textures: Vec::new(),
            binds: Vec::new(),
        }
    }

    pub(crate) fn upload_images(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        images: &[&DecodedTexture],
    ) -> Result<(), Error> {
        self.textures.clear();
        self.binds.clear();
        let limit = device.limits().max_texture_dimension_2d;
        for image in images {
            if image.width > limit || image.height > limit {
                return Err(Error::Upload(format!(
                    "{}x{} exceeds the device limit of {limit}",
                    image.width, image.height
                )));
            }
            let (bytes, stride) = pad_rgba(image.width, image.height, &image.rgba)?;
            let texture = make_rgba(device, image.width, image.height, "iris-scene-layer");
            write_rgba(queue, &texture, image.width, image.height, stride, &bytes);
            let bind = bind_texture(device, &self.texture_layout, &self.sampler, &texture);
            self.textures.push(texture);
            self.binds.push(bind);
        }
        Ok(())
    }

    pub(crate) fn record(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        draw: &SceneDraw<'_>,
    ) -> wgpu::CommandBuffer {
        let uniforms = SceneUniforms {
            time: draw.time,
            delta_time: draw.delta_time,
            pointer_position: draw.pointer,
            viewport_size: draw.viewport,
            audio_spectrum_16: draw.audio_spectrum_16,
            audio_spectrum_64: draw.audio_spectrum_64,
        };
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            &pack_frame(&uniforms, &draw.telemetry),
        );
        let usable = draw.instances.len() / INSTANCE_STRIDE;
        let usable = usable.min(MAX_INSTANCES);
        if usable > 0 {
            let bytes = usable * INSTANCE_STRIDE;
            queue.write_buffer(&self.instance_buffer, 0, &draw.instances[..bytes]);
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("iris-scene"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("iris-scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(draw.clear[0]),
                            g: f64::from(draw.clear[1]),
                            b: f64::from(draw.clear[2]),
                            a: f64::from(draw.clear[3]),
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let _texture_owner = &self.white;
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
            pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            for span in draw.spans {
                let end = span.start.saturating_add(span.count);
                if span.count == 0 || end as usize > usable {
                    continue;
                }
                pass.set_pipeline(&self.pipelines[blend_index(span.blend)]);
                let bind = self
                    .binds
                    .get(span.texture as usize)
                    .unwrap_or(&self.white_bind);
                pass.set_bind_group(1, bind, &[]);
                pass.draw_indexed(0..6, 0, span.start..end);
            }
        }
        encoder.finish()
    }
}

fn blend_index(blend: LayerBlend) -> usize {
    match blend {
        LayerBlend::Alpha => 0,
        LayerBlend::Additive => 1,
        LayerBlend::Multiply => 2,
        LayerBlend::Screen => 3,
    }
}

fn bytemuck_indices() -> &'static [u8] {
    // Six little-endian u16 indices: 0,1,2, 0,2,3. Written without a Pod derive
    // so this crate can keep forbidding unsafe.
    &[0, 0, 1, 0, 2, 0, 0, 0, 2, 0, 3, 0]
}

fn pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    uniform_layout: &wgpu::BindGroupLayout,
    texture_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    blend: LayerBlend,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("iris-scene"),
        bind_group_layouts: &[uniform_layout, texture_layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("iris-scene"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: INSTANCE_STRIDE as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 16,
                        shader_location: 1,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 32,
                        shader_location: 2,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 48,
                        shader_location: 3,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 64,
                        shader_location: 4,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 80,
                        shader_location: 5,
                    },
                ],
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend_state(blend)),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    })
}

fn blend_state(blend: LayerBlend) -> wgpu::BlendState {
    match blend {
        LayerBlend::Alpha => wgpu::BlendState::ALPHA_BLENDING,
        LayerBlend::Additive => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        },
        LayerBlend::Multiply => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        },
        LayerBlend::Screen => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrc,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        },
    }
}

fn make_rgba(device: &wgpu::Device, width: u32, height: u32, label: &str) -> wgpu::Texture {
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
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn bind_texture(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    texture: &wgpu::Texture,
) -> wgpu::BindGroup {
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("iris-scene-texture"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn write_rgba(
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

fn pad_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<(Vec<u8>, u32), Error> {
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
        let src = y * row;
        let dst = y * padded;
        let source = rgba.get(src..src + row).ok_or_else(|| {
            Error::Upload(format!(
                "rgba buffer is {} bytes, row {y} needs {}",
                rgba.len(),
                src + row
            ))
        })?;
        out[dst..dst + row].copy_from_slice(source);
    }
    Ok((out, stride))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_uniform_places_the_spectra_at_the_packed_offsets() {
        let mut uniforms = SceneUniforms {
            time: 1.0,
            delta_time: 2.0,
            pointer_position: [3.0, 4.0],
            viewport_size: [5.0, 6.0],
            audio_spectrum_16: [0.0; 16],
            audio_spectrum_64: [0.0; 64],
        };
        uniforms.audio_spectrum_16[0] = 0.25;
        uniforms.audio_spectrum_64[63] = 0.5;
        let bytes = uniforms.to_bytes();
        assert_eq!(bytes.len(), SCENE_UNIFORM_SIZE);
        assert_eq!(&bytes[0..4], &1.0f32.to_le_bytes());
        assert_eq!(&bytes[24..28], &0.25f32.to_le_bytes());
        assert_eq!(&bytes[88 + 63 * 4..88 + 64 * 4], &0.5f32.to_le_bytes());
    }

    #[test]
    fn scene_shader_parses() {
        wgpu::naga::front::wgsl::parse_str(SCENE_SHADER).expect("scene shader");
    }

    #[test]
    fn telemetry_sits_after_the_scene_block() {
        let mut current = TelemetryUniforms::neutral();
        current.cpu_global = 0.25;
        current.cpu_cores[15] = 0.5;
        current.is_on_battery = 1;
        let frame = TelemetryFrame {
            previous: TelemetryUniforms::neutral(),
            current,
            blend: 0.5,
        };
        let packed = pack_frame(
            &SceneUniforms {
                time: 0.0,
                delta_time: 0.0,
                pointer_position: [0.0, 0.0],
                viewport_size: [0.0, 0.0],
                audio_spectrum_16: [0.0; 16],
                audio_spectrum_64: [0.0; 64],
            },
            &frame,
        );
        assert_eq!(packed.len(), FRAME_UNIFORM_SIZE);
        assert_eq!(
            &packed[SCENE_UNIFORM_SIZE + TELEMETRY_UNIFORM_SIZE
                ..SCENE_UNIFORM_SIZE + TELEMETRY_UNIFORM_SIZE + 4],
            &0.25f32.to_le_bytes()
        );
        let core = SCENE_UNIFORM_SIZE + TELEMETRY_UNIFORM_SIZE + 32 + 15 * 4;
        assert_eq!(&packed[core..core + 4], &0.5f32.to_le_bytes());
        let flag = SCENE_UNIFORM_SIZE + TELEMETRY_UNIFORM_SIZE + 20;
        assert_eq!(&packed[flag..flag + 4], &1u32.to_le_bytes());
        let blend_at = SCENE_UNIFORM_SIZE + TELEMETRY_UNIFORM_SIZE * 2;
        assert_eq!(&packed[blend_at..blend_at + 4], &0.5f32.to_le_bytes());
    }
}
