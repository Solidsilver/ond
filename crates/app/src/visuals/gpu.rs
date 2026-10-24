//! The visualizer on the GPU, as an egui paint callback with its own wgpu
//! pipelines. Two offscreen textures take turns: each frame the previous
//! picture is drawn into the other one faded, slightly zoomed, turned and
//! drifted (the feedback that turns moving lights into trails), the lights
//! are added on top, and the result is drawn into egui's pass.
//!
//! Everything lives in egui-wgpu's callback resources and is made the first
//! time it's needed, for the target format egui renders to; the offscreen
//! textures follow the size drawn at.

use super::visualizer::{Feedback, Light};
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor, wgpu};

/// The offscreen picture's format (filterable and blendable everywhere).
const OFFSCREEN: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Lights a frame can carry.
const MAX_LIGHTS: usize = 256;
/// Bytes per light: centre, radius (x, y), color.
const LIGHT_BYTES: usize = 8 * 4;

const SHADER: &str = r#"
struct U {
    fade: f32,
    zoom: f32,
    turn: f32,
    aspect: f32,
    drift: vec2<f32>,
    linear_out: f32,
    _pad: f32,
};
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> u: U;

struct Full {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// One triangle that covers the viewport.
@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> Full {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Full;
    o.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}

// Last frame, faded, scaled up about the centre, turned and drifted.
@fragment
fn fs_feedback(in: Full) -> @location(0) vec4<f32> {
    var c = in.uv - vec2<f32>(0.5, 0.5);
    c.x = c.x * u.aspect;
    let s = sin(u.turn);
    let k = cos(u.turn);
    c = vec2<f32>(c.x * k - c.y * s, c.x * s + c.y * k) / u.zoom;
    c.x = c.x / u.aspect;
    let prev = textureSample(tex, samp, c + vec2<f32>(0.5, 0.5) + u.drift).rgb;
    // The small subtraction lets the dimmest trail reach black: on an 8-bit
    // picture, scaling alone would round it back up forever and leave a haze.
    return vec4<f32>(max(prev * u.fade - vec3<f32>(1.5 / 255.0), vec3<f32>(0.0)), 1.0);
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// The picture into egui's pass.
@fragment
fn fs_show(in: Full) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, in.uv).rgb;
    if (u.linear_out > 0.5) {
        return vec4<f32>(to_linear(c), 1.0);
    }
    return vec4<f32>(c, 1.0);
}

struct Lit {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_light(@builtin(vertex_index) i: u32, @location(0) centre: vec2<f32>, @location(1) radius: vec2<f32>, @location(2) color: vec4<f32>) -> Lit {
    var corners = array<vec2<f32>, 6>(vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0), vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0));
    let k = corners[i];
    var o: Lit;
    o.pos = vec4<f32>(centre + k * radius, 0.0, 1.0);
    o.local = k;
    o.color = color;
    return o;
}

// A soft round glow, added to what's there.
@fragment
fn fs_light(in: Lit) -> @location(0) vec4<f32> {
    let d = dot(in.local, in.local);
    let a = exp(-d * 3.5) * (1.0 - smoothstep(0.7, 1.0, d)) * in.color.a;
    return vec4<f32>(in.color.rgb * a, a);
}
"#;

/// What one frame draws: the lights (in the scene's units), the feedback
/// for this frame's length, and the size drawn at.
pub struct VisualizerCallback {
    /// egui's render target format.
    pub format: wgpu::TextureFormat,
    pub size_px: [u32; 2],
    pub lights: Vec<Light>,
    pub feedback: Feedback,
    /// Start from black (the view just opened).
    pub clear: bool,
}

struct Pictures {
    size: [u32; 2],
    views: [wgpu::TextureView; 2],
    /// Bind groups that sample each picture.
    reads: [wgpu::BindGroup; 2],
    /// The one holding the latest frame.
    current: usize,
}

struct Gpu {
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    lights: wgpu::Buffer,
    feedback: wgpu::RenderPipeline,
    light: wgpu::RenderPipeline,
    show: wgpu::RenderPipeline,
    pictures: Option<Pictures>,
    /// Lights in the buffer this frame.
    count: u32,
}

fn bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

impl Gpu {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("visualizer"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("visualizer"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { multisampled: false, sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2 },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("visualizer"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let lights_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("visualizer lights"), bind_group_layouts: &[], immediate_size: 0 });
        let full = |entry: &str, target: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_full"), buffers: &[], compilation_options: Default::default() },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState { format: target, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let feedback = full("fs_feedback", OFFSCREEN);
        let show = full("fs_show", format);
        let additive = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let light = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("visualizer lights"),
            layout: Some(&lights_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_light"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: LIGHT_BYTES as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
                })],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_light"),
                targets: &[Some(wgpu::ColorTargetState { format: OFFSCREEN, blend: Some(wgpu::BlendState { color: additive, alpha: additive }), write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("visualizer"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("visualizer"), size: 32, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let lights = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("visualizer lights"),
            size: (MAX_LIGHTS * LIGHT_BYTES) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { format, layout, sampler, uniforms, lights, feedback, light, show, pictures: None, count: 0 }
    }

    /// The two pictures at `size`, made (black) when the size changes.
    fn pictures(&mut self, device: &wgpu::Device, size: [u32; 2]) -> &mut Pictures {
        if self.pictures.as_ref().is_none_or(|p| p.size != size) {
            let view = || {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("visualizer picture"),
                        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: OFFSCREEN,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default())
            };
            let views = [view(), view()];
            let read = |v: &wgpu::TextureView| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("visualizer read"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(v) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        wgpu::BindGroupEntry { binding: 2, resource: self.uniforms.as_entire_binding() },
                    ],
                })
            };
            let reads = [read(&views[0]), read(&views[1])];
            self.pictures = Some(Pictures { size, views, reads, current: 0 });
        }
        self.pictures.as_mut().expect("just made")
    }
}

impl CallbackTrait for VisualizerCallback {
    fn prepare(&self, device: &wgpu::Device, queue: &wgpu::Queue, _screen: &ScreenDescriptor, encoder: &mut wgpu::CommandEncoder, resources: &mut CallbackResources) -> Vec<wgpu::CommandBuffer> {
        if self.size_px[0] == 0 || self.size_px[1] == 0 {
            return Vec::new();
        }
        if resources.get::<Gpu>().is_none_or(|g| g.format != self.format) {
            resources.insert(Gpu::new(device, self.format));
        }
        let Some(gpu) = resources.get_mut::<Gpu>() else { return Vec::new() };
        let aspect = self.size_px[0] as f32 / self.size_px[1] as f32;
        let f = &self.feedback;
        let fade = if self.clear { 0.0 } else { f.fade };
        queue.write_buffer(&gpu.uniforms, 0, &bytes(&[fade, f.zoom, f.turn, aspect, f.drift[0], f.drift[1], if self.format.is_srgb() { 1.0 } else { 0.0 }, 0.0]));
        // The scene's units span the shorter side; the pass's span its own.
        let (sx, sy) = if aspect >= 1.0 { (1.0 / aspect, 1.0) } else { (1.0, aspect) };
        let lights: Vec<f32> = self.lights.iter().take(MAX_LIGHTS).flat_map(|l| [l.x * sx, l.y * sy, l.radius * sx, l.radius * sy, l.color[0], l.color[1], l.color[2], l.color[3]]).collect();
        gpu.count = (lights.len() / 8) as u32;
        queue.write_buffer(&gpu.lights, 0, &bytes(&lights));

        let (count, light_pipe, feedback_pipe, light_buf) = (gpu.count, gpu.light.clone(), gpu.feedback.clone(), gpu.lights.clone());
        let p = gpu.pictures(device, self.size_px);
        let next = 1 - p.current;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("visualizer"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &p.views[next],
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                ..Default::default()
            });
            pass.set_pipeline(&feedback_pipe);
            pass.set_bind_group(0, &p.reads[p.current], &[]);
            pass.draw(0..3, 0..1);
            if count > 0 {
                pass.set_pipeline(&light_pipe);
                pass.set_vertex_buffer(0, light_buf.slice(..));
                pass.draw(0..6, 0..count);
            }
        }
        p.current = next;
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &CallbackResources) {
        let Some(gpu) = resources.get::<Gpu>() else { return };
        let Some(p) = &gpu.pictures else { return };
        pass.set_pipeline(&gpu.show);
        pass.set_bind_group(0, &p.reads[p.current], &[]);
        pass.draw(0..3, 0..1);
    }
}

/// The paint callback for one frame, filling `rect`.
pub fn shape(rect: egui::Rect, callback: VisualizerCallback) -> egui::Shape {
    egui::Shape::Callback(egui_wgpu::Callback::new_paint_callback(rect, callback))
}
