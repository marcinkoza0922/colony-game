//! wgpu renderer: one instanced textured-quad pipeline with nearest sampling.
//! The world and the UI are two batches of quads with different transforms.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use winit::window::Window;

/// One sprite instance. Positions/sizes in batch-space pixels; uv in 0..1.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Quad {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub uv: [f32; 4],
    pub color: [f32; 4],
    /// x: lit (0/1), y: outdoor (receives sky/sun) (0/1).
    pub light: [f32; 4],
    /// Map-space position (x, y, z in levels) used for point lights.
    pub wpos: [f32; 4],
}

impl Quad {
    pub fn unlit(pos: [f32; 2], size: [f32; 2], uv: [f32; 4], color: [f32; 4]) -> Quad {
        Quad { pos, size, uv, color, light: [0.0; 4], wpos: [0.0; 4] }
    }
}

pub const MAX_LIGHTS: usize = 64;

/// Scene lighting for one frame. Directions are in camera space
/// (x right, y up, z toward the viewer); `map_to_cam` converts map-space
/// vectors (x east, y south, z up in levels).
#[derive(Clone, Debug)]
pub struct Lighting {
    pub sun_dir: [f32; 3],
    pub sun_col: [f32; 3],
    pub sky: [f32; 3],
    pub cave: [f32; 3],
    pub map_to_cam: [[f32; 3]; 3],
    /// (map position, radius, colour)
    pub lights: Vec<([f32; 3], f32, [f32; 3])>,
}

impl Default for Lighting {
    fn default() -> Self {
        Lighting {
            sun_dir: [0.0, 0.7, 0.7],
            sun_col: [0.6, 0.6, 0.6],
            sky: [0.6, 0.6, 0.6],
            cave: [0.2, 0.2, 0.2],
            map_to_cam: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            lights: Vec::new(),
        }
    }
}

/// screen_px = batch_px * scale + offset
#[derive(Clone, Copy, Debug)]
pub struct Xf {
    pub scale: f32,
    pub offset: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    screen: [f32; 2],
    scale: f32,
    nlights: f32,
    offset: [f32; 2],
    _pad2: [f32; 2],
    sun_dir: [f32; 4],
    sun_col: [f32; 4],
    sky: [f32; 4],
    cave: [f32; 4],
    m: [[f32; 4]; 3],
    lights: [[f32; 4]; MAX_LIGHTS],
    lcols: [[f32; 4]; MAX_LIGHTS],
}

const SHADER: &str = r#"
struct Globals {
    screen: vec2<f32>, scale: f32, nlights: f32, offset: vec2<f32>, _pad2: vec2<f32>,
    sun_dir: vec4<f32>, sun_col: vec4<f32>, sky: vec4<f32>, cave: vec4<f32>,
    m: array<vec4<f32>, 3>,
    lights: array<vec4<f32>, 64>,
    lcols: array<vec4<f32>, 64>,
};
@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(1) @binding(2) var ntex: texture_2d<f32>;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) light: vec4<f32>,
    @location(3) wpos: vec4<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32,
      @location(0) pos: vec2<f32>, @location(1) size: vec2<f32>,
      @location(2) uv: vec4<f32>, @location(3) color: vec4<f32>,
      @location(4) light: vec4<f32>, @location(5) wpos: vec4<f32>) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    let c = corners[vi];
    let px = (pos + c * size) * g.scale + g.offset;
    var o: VOut;
    o.pos = vec4(px.x / g.screen.x * 2.0 - 1.0, 1.0 - px.y / g.screen.y * 2.0, 0.0, 1.0);
    o.uv = mix(uv.xy, uv.zw, c);
    o.color = color;
    o.light = light;
    o.wpos = wpos;
    return o;
}

fn to_cam(v: vec3<f32>) -> vec3<f32> {
    return vec3(dot(g.m[0].xyz, v), dot(g.m[1].xyz, v), dot(g.m[2].xyz, v));
}

@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, samp, i.uv) * i.color;
    if (t.a < 0.01) { discard; }
    if (i.light.x < 0.5) { return t; }
    let raw = textureSample(ntex, samp, i.uv).xyz * 2.0 - 1.0;
    if (length(raw) < 0.3) {
        // Emissive pixel: full bright regardless of light.
        return vec4(t.rgb * 1.1, t.a);
    }
    let n = normalize(raw);
    var l = g.cave.rgb;
    if (i.light.y > 0.5) {
        l = g.sky.rgb + g.sun_col.rgb * max(dot(n, g.sun_dir.xyz), 0.0);
    }
    let count = i32(g.nlights);
    for (var k = 0; k < count; k = k + 1) {
        let d = g.lights[k].xyz - i.wpos.xyz;
        let dist = length(d);
        let r = g.lights[k].w;
        if (dist < r) {
            let att = (1.0 - dist / r) * (1.0 - dist / r);
            let dir = normalize(to_cam(d) + vec3(0.0, 0.0, 0.001));
            l = l + g.lcols[k].rgb * att * (0.35 + 0.65 * max(dot(n, dir), 0.0));
        }
    }
    // Quantise light into bands to keep the pixel-art look.
    l = floor(l * 10.0 + 0.5) / 10.0;
    return vec4(t.rgb * l, t.a);
}
"#;

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: Option<wgpu::Surface<'static>>,
    pub config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    globals: [wgpu::Buffer; 2],
    globals_bg: [wgpu::BindGroup; 2],
    tex_layout: wgpu::BindGroupLayout,
    tex_bg: Option<wgpu::BindGroup>,
    sampler: wgpu::Sampler,
    instances: wgpu::Buffer,
    capacity: usize,
    pub atlas_size: (u32, u32),
}

impl Gpu {
    pub fn new(window: Option<Arc<Window>>, width: u32, height: u32) -> Gpu {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = window.map(|w| instance.create_surface(w).expect("create surface"));
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface.as_ref(),
            force_fallback_adapter: false,
        }))
        .expect("no GPU adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("device"),
            ..Default::default()
        }))
        .expect("request device");

        let format = match &surface {
            Some(s) => {
                let caps = s.get_capabilities(&adapter);
                caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0])
            }
            None => wgpu::TextureFormat::Rgba8Unorm,
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        if let Some(s) = &surface {
            s.configure(&device, &config);
        }

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprite"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let glob_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
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
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tex"),
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
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite"),
            bind_group_layouts: &[&glob_layout, &tex_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprite"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Quad>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4],
                }],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let mk_glob = || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("globals"),
                size: std::mem::size_of::<Globals>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let globals = [mk_glob(), mk_glob()];
        let globals_bg = [0, 1].map(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("globals"),
                layout: &glob_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals[i].as_entire_binding() }],
            })
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let capacity = 1 << 16;
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (capacity * std::mem::size_of::<Quad>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Gpu {
            device,
            queue,
            surface,
            config,
            pipeline,
            globals,
            globals_bg,
            tex_layout,
            tex_bg: None,
            sampler,
            instances,
            capacity,
            atlas_size: (1, 1),
        }
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        self.config.width = w.max(1);
        self.config.height = h.max(1);
        if let Some(s) = &self.surface {
            s.configure(&self.device, &self.config);
        }
    }

    pub fn upload_atlas(&mut self, w: u32, h: u32, rgba: &[u8], normals: &[u8]) {
        let mk = |data: &[u8], label| {
            self.device
                .create_texture_with_data(
                    &self.queue,
                    &wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    data,
                )
                .create_view(&Default::default())
        };
        let view = mk(rgba, "atlas");
        let nview = mk(normals, "atlas_n");
        self.tex_bg = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas"),
            layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&nview) },
            ],
        }));
        self.atlas_size = (w, h);
    }

    fn encode(&mut self, view: &wgpu::TextureView, size: (u32, u32), batches: &[(&[Quad], Xf)], lit: &Lighting, clear: [f64; 3]) -> wgpu::CommandEncoder {
        let total: usize = batches.iter().map(|(q, _)| q.len()).sum();
        if total > self.capacity {
            self.capacity = total.next_power_of_two();
            self.instances = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("instances"),
                size: (self.capacity * std::mem::size_of::<Quad>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        let mut offset = 0u64;
        for (q, _) in batches {
            if !q.is_empty() {
                self.queue.write_buffer(&self.instances, offset, bytemuck::cast_slice(q));
            }
            offset += std::mem::size_of_val(*q) as u64;
        }
        for (i, (_, xf)) in batches.iter().enumerate().take(2) {
            let v4 = |v: [f32; 3]| [v[0], v[1], v[2], 0.0];
            let mut g = Globals {
                screen: [size.0 as f32, size.1 as f32],
                scale: xf.scale,
                nlights: lit.lights.len().min(MAX_LIGHTS) as f32,
                offset: xf.offset,
                _pad2: [0.0; 2],
                sun_dir: v4(lit.sun_dir),
                sun_col: v4(lit.sun_col),
                sky: v4(lit.sky),
                cave: v4(lit.cave),
                m: [v4(lit.map_to_cam[0]), v4(lit.map_to_cam[1]), v4(lit.map_to_cam[2])],
                lights: [[0.0; 4]; MAX_LIGHTS],
                lcols: [[0.0; 4]; MAX_LIGHTS],
            };
            for (k, (p, r, c)) in lit.lights.iter().take(MAX_LIGHTS).enumerate() {
                g.lights[k] = [p[0], p[1], p[2], *r];
                g.lcols[k] = v4(*c);
            }
            self.queue.write_buffer(&self.globals[i], 0, bytemuck::bytes_of(&g));
        }
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0], g: clear[1], b: clear[2], a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some(tbg) = &self.tex_bg {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(1, tbg, &[]);
                pass.set_vertex_buffer(0, self.instances.slice(..));
                let mut first = 0u32;
                for (i, (q, _)) in batches.iter().enumerate().take(2) {
                    pass.set_bind_group(0, &self.globals_bg[i], &[]);
                    let n = q.len() as u32;
                    if n > 0 {
                        pass.draw(0..6, first..first + n);
                    }
                    first += n;
                }
            }
        }
        enc
    }

    /// Renders to the window surface.
    pub fn render(&mut self, batches: &[(&[Quad], Xf)], lit: &Lighting, clear: [f64; 3]) {
        let Some(surface) = &self.surface else { return };
        let frame = match surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                let (w, h) = (self.config.width, self.config.height);
                self.resize(w, h);
                return;
            }
            Err(e) => {
                eprintln!("surface error: {e:?}");
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let size = (self.config.width, self.config.height);
        let enc = self.encode(&view, size, batches, lit, clear);
        self.queue.submit([enc.finish()]);
        frame.present();
    }

    /// Renders offscreen and returns tightly packed RGBA8 pixels.
    pub fn render_offscreen(&mut self, w: u32, h: u32, batches: &[(&[Quad], Xf)], lit: &Lighting, clear: [f64; 3]) -> Vec<u8> {
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let mut enc = self.encode(&view, (w, h), batches, lit, clear);
        let bpr = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bpr * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let data = slice.get_mapped_range();
        let bgra = matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let row = &data[(y * bpr) as usize..(y * bpr + w * 4) as usize];
            if bgra {
                for px in row.chunks(4) {
                    out.extend_from_slice(&[px[2], px[1], px[0], 255]);
                }
            } else {
                for px in row.chunks(4) {
                    out.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
            }
        }
        out
    }
}
