//! The visualizer on the GPU, as an egui paint callback with its own wgpu
//! pipelines. Two offscreen pictures take turns: each frame the previous one
//! is drawn into the other moved along the scene's flow field, blurred a
//! touch and faded (the feedback that turns moving lights into trails), the
//! lights are added on top (dark spheres between two layers of them), and the
//! result is drawn into egui's pass: through a colour map, or as light over a
//! backdrop.
//!
//! The pictures are half-float where the adapter can render to and blend
//! half-float textures (no banding as trails fade), and 8-bit with a little
//! dither otherwise. They're drawn at most [`MAX_SIDE`] pixels on the long
//! side and scaled up: the trails are soft anyway, and it keeps the cost
//! steady on a big display or an integrated GPU.
//!
//! Everything lives in egui-wgpu's callback resources and is made the first
//! time it's needed; the pictures follow the size drawn at.

use super::visualizer::{Dot, Field, Look, Picture};
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor, wgpu};

/// The pictures' longest side, in pixels.
pub const MAX_SIDE: u32 = 1440;
/// Floats per light: centre and axis, radius and core (plus 2 for a joined
/// stroke), color.
const DOT_FLOATS: usize = 10;
/// Floats in the uniforms (20 vec4s; see `U` in the shader).
const UNIFORM_FLOATS: usize = 80;

/// Where the visualizer draws: egui's target format, and the format of its
/// own pictures (half-float where the adapter supports it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub format: wgpu::TextureFormat,
    pub offscreen: wgpu::TextureFormat,
}

impl Target {
    pub fn new(format: wgpu::TextureFormat, adapter: &wgpu::Adapter) -> Self {
        let f = adapter.get_texture_format_features(wgpu::TextureFormat::Rgba16Float);
        let usable = f.allowed_usages.contains(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING)
            && f.flags.contains(wgpu::TextureFormatFeatureFlags::FILTERABLE | wgpu::TextureFormatFeatureFlags::BLENDABLE);
        if !usable {
            tracing::info!("visualizer: no half-float render targets on this adapter; its pictures are 8-bit");
        }
        Self { format, offscreen: if usable { wgpu::TextureFormat::Rgba16Float } else { wgpu::TextureFormat::Rgba8Unorm } }
    }

    /// 8-bit pictures, for a renderer whose adapter isn't to hand.
    #[cfg(test)]
    pub fn plain(format: wgpu::TextureFormat) -> Self {
        Self { format, offscreen: wgpu::TextureFormat::Rgba8Unorm }
    }

    fn float(&self) -> bool {
        self.offscreen == wgpu::TextureFormat::Rgba16Float
    }
}

/// The pictures' size for a view `size` pixels big.
pub fn picture_size(size: [u32; 2]) -> [u32; 2] {
    let long = size[0].max(size[1]);
    if long <= MAX_SIDE {
        return size;
    }
    let k = MAX_SIDE as f32 / long as f32;
    size.map(|v| ((v as f32 * k).round() as u32).max(1))
}

const SHADER: &str = r#"
struct Flow { a: vec4<f32>, b: vec4<f32>, c: vec4<f32> };
struct U {
    // fade, flow mix, time, -
    frame: vec4<f32>,
    // blur (texels), floor (taken off each frame), linear output, mapped
    misc: vec4<f32>,
    // palette mix, dither, texel size
    look: vec4<f32>,
    // half the view in scene units, brightness, -
    view: vec4<f32>,
    fa: Flow,
    fb: Flow,
    pa: array<vec4<f32>, 4>,
    pb: array<vec4<f32>, 4>,
    back: array<vec4<f32>, 2>,
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

fn rot(p: vec2<f32>, a: f32) -> vec2<f32> {
    let s = sin(a);
    let c = cos(a);
    return vec2<f32>(p.x * c - p.y * s, p.x * s + p.y * c);
}

// Where the flow field brings the point at `q` from (scene units).
fn source(q: vec2<f32>, f: Flow, t: f32) -> vec2<f32> {
    let p = q - f.c.zw;
    let r = length(p);
    let th = atan2(p.y, p.x);
    let zoom = f.a.x * (1.0 + f.b.y * cos(f.b.x * th + t * 0.4)) * (1.0 + f.b.w * sin(f.b.z * r - t * 1.7));
    let turn = f.a.y + f.a.z * r + f.a.w / (r + 0.2);
    var s = rot(p, -turn) / zoom;
    s = s - f.c.y * vec2<f32>(sin(p.y * f.c.x + t), cos(p.x * f.c.x * 0.8 + t * 1.3));
    return s + f.c.zw;
}

fn grab(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(tex, samp, uv, 0.0);
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

// Last frame, moved along the flow field (part way to the next one),
// blurred a touch and faded. Outside the old picture is black.
@fragment
fn fs_feedback(in: Full) -> @location(0) vec4<f32> {
    let span = vec2<f32>(2.0 * u.view.x, -2.0 * u.view.y);
    let q = (in.uv - 0.5) * span;
    let t = u.frame.z;
    let s = mix(source(q, u.fa, t), source(q, u.fb, t), u.frame.y);
    let uv = s / span + 0.5;
    let d = u.look.zw * u.misc.x;
    var c = grab(uv).rgb * 0.5;
    c += (grab(uv + d).rgb + grab(uv - d).rgb + grab(uv + vec2<f32>(d.x, -d.y)).rgb + grab(uv + vec2<f32>(-d.x, d.y)).rgb) * 0.125;
    let edge = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
    c *= clamp(edge * 200.0, 0.0, 1.0);
    // The small subtraction lets the dimmest trail reach black (on an 8-bit
    // picture, scaling alone would round it back up forever and leave a haze);
    // the dither keeps 8-bit trails from banding.
    let n = (hash(in.pos.xy + vec2<f32>(t * 61.0, t * 37.0)) - 0.5) * u.look.y;
    return vec4<f32>(max(c * u.frame.x - vec3<f32>(u.misc.y) + vec3<f32>(n), vec3<f32>(0.0)), 0.0);
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// A colour map: brightness through four stops (rgb, position).
fn ramp(i: f32, s0: vec4<f32>, s1: vec4<f32>, s2: vec4<f32>, s3: vec4<f32>) -> vec3<f32> {
    if (i <= s1.w) {
        return mix(s0.rgb, s1.rgb, clamp((i - s0.w) / max(s1.w - s0.w, 1e-4), 0.0, 1.0));
    }
    if (i <= s2.w) {
        return mix(s1.rgb, s2.rgb, (i - s1.w) / max(s2.w - s1.w, 1e-4));
    }
    return mix(s2.rgb, s3.rgb, clamp((i - s2.w) / max(s3.w - s2.w, 1e-4), 0.0, 1.0));
}

// Bright light rolls off instead of clipping.
fn tone(x: vec3<f32>) -> vec3<f32> {
    let knee = vec3<f32>(0.6);
    return select(x, knee + 0.4 * (1.0 - exp(-(x - knee) / 0.4)), x > knee);
}

// The picture into egui's pass.
@fragment
fn fs_show(in: Full) -> @location(0) vec4<f32> {
    let px = grab(in.uv);
    var c: vec3<f32>;
    if (u.misc.w > 0.5) {
        // Brightness eases toward the map's top, so piled-up light stays colored.
        let i = 1.0 - exp(-1.4 * max(px.r, max(px.g, px.b)));
        c = mix(ramp(i, u.pa[0], u.pa[1], u.pa[2], u.pa[3]), ramp(i, u.pb[0], u.pb[1], u.pb[2], u.pb[3]), u.look.x);
    } else {
        let r = length((in.uv - 0.5) * 2.0);
        let back = mix(u.back[0].rgb, u.back[1].rgb, smoothstep(0.0, 1.3, r));
        c = back * (1.0 - clamp(px.a, 0.0, 1.0)) + tone(px.rgb);
    }
    c = c * u.view.z;
    if (u.misc.z > 0.5) {
        return vec4<f32>(to_linear(c), 1.0);
    }
    return vec4<f32>(c, 1.0);
}

struct Lit {
    @builtin(position) pos: vec4<f32>,
    // Across the light, in radii: x along the streak, y across it.
    @location(0) local: vec2<f32>,
    // The streak's half-length, in radii, and how crisp (plus 2 for a joined stroke).
    @location(1) shape: vec2<f32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_dot(@builtin(vertex_index) i: u32, @location(0) at_axis: vec4<f32>, @location(1) size: vec2<f32>, @location(2) color: vec4<f32>) -> Lit {
    var corners = array<vec2<f32>, 6>(vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0), vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0));
    let k = corners[i];
    let len = length(at_axis.zw);
    let dir = select(vec2<f32>(1.0, 0.0), at_axis.zw / max(len, 1e-6), len > 1e-6);
    let r = max(size.x, 1e-5);
    let p = at_axis.xy + dir * k.x * (len + r) + vec2<f32>(-dir.y, dir.x) * k.y * r;
    var o: Lit;
    o.pos = vec4<f32>(p / u.view.xy, 0.0, 1.0);
    o.local = vec2<f32>(k.x * (len + r) / r, k.y);
    o.shape = vec2<f32>(len / r, size.y);
    o.color = color;
    return o;
}

// How much of a light covers this point: a soft glow, or a crisp dot.
fn profile(d: f32, crisp: f32) -> f32 {
    let soft = exp(-d * d * 3.5);
    let hard = 1.0 - smoothstep(0.55, 1.0, d);
    return mix(soft, hard, crisp) * (1.0 - smoothstep(0.85, 1.0, d));
}

fn cover(in: Lit) -> f32 {
    if (in.shape.y >= 2.0) {
        // A stroke of a line: full across its length, then a half-radius
        // ramp either side of each end, so where two strokes meet they sum
        // to one instead of doubling.
        let along = clamp(in.shape.x + 0.5 - abs(in.local.x), 0.0, 1.0);
        return profile(abs(in.local.y), in.shape.y - 2.0) * along;
    }
    let x = max(abs(in.local.x) - in.shape.x, 0.0);
    return profile(sqrt(x * x + in.local.y * in.local.y), in.shape.y);
}

// A light, added to what's there.
@fragment
fn fs_dot(in: Lit) -> @location(0) vec4<f32> {
    let a = cover(in) * in.color.a;
    return vec4<f32>(in.color.rgb * a, 0.0);
}

// A dark sphere over what's there, solid to a crisp edge; its alpha marks
// it, so the backdrop doesn't show through it either.
@fragment
fn fs_shade(in: Lit) -> @location(0) vec4<f32> {
    let d = length(in.local);
    return vec4<f32>(0.0, 0.0, 0.0, (1.0 - smoothstep(0.9, 1.0, d)) * in.color.a);
}
"#;

/// What one frame draws: the scene's picture, at `size_px`.
pub struct VisualizerCallback {
    pub target: Target,
    pub size_px: [u32; 2],
    /// Half the view in scene units.
    pub half: [f32; 2],
    pub picture: Picture,
    /// Start from black (the view just opened, or switched visualizer).
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
    target: Target,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    dots: wgpu::Buffer,
    /// Lights the buffer holds.
    capacity: usize,
    feedback: wgpu::RenderPipeline,
    dot: wgpu::RenderPipeline,
    shade: wgpu::RenderPipeline,
    show: wgpu::RenderPipeline,
    pictures: Option<Pictures>,
    /// The lights' bytes for the buffer, kept so a frame doesn't allocate.
    staging: Vec<u8>,
}

/// `values` as the bytes a buffer takes, appended to `out`.
fn put_bytes(out: &mut Vec<u8>, values: &[f32]) {
    for v in values {
        out.extend_from_slice(&v.to_ne_bytes());
    }
}

fn field_floats(f: &Field) -> [f32; 12] {
    [f.zoom, f.turn, f.twist, f.vortex, f.petals, f.petal, f.ripples, f.ripple, f.waves, f.wave, f.centre[0], f.centre[1]]
}

/// The shader's uniforms for `picture`.
fn uniforms(c: &VisualizerCallback, texel: [f32; 2]) -> [f32; UNIFORM_FLOATS] {
    let p = &c.picture;
    let w = &p.warp;
    let fade = if c.clear { 0.0 } else { w.fade };
    let float = c.target.float();
    let (floor, dither) = if float { (0.0008, 0.0) } else { (1.5 / 255.0, 1.0 / 255.0) };
    let linear = if c.target.format.is_srgb() { 1.0 } else { 0.0 };
    let mut u = [0.0; UNIFORM_FLOATS];
    let mut at = 0;
    let mut put = |values: &[f32]| {
        u[at..at + values.len()].copy_from_slice(values);
        at += values.len();
    };
    put(&[fade, w.mix, p.time, 0.0]);
    let (mapped, pal_mix, pa, pb, back) = match p.look {
        Look::Mapped { from, to, mix } => (1.0, mix, from.0, to.0, [[0.0; 3]; 2]),
        Look::Lit { centre, edge } => (0.0, 0.0, [[0.0; 4]; 4], [[0.0; 4]; 4], [centre, edge]),
    };
    put(&[w.blur, floor, linear, mapped]);
    put(&[pal_mix, dither, texel[0], texel[1]]);
    put(&[c.half[0], c.half[1], p.bright, 0.0]);
    put(&field_floats(&w.from));
    put(&field_floats(&w.to));
    put(pa.as_flattened());
    put(pb.as_flattened());
    for b in back {
        put(&[b[0], b[1], b[2], 0.0]);
    }
    assert_eq!(at, UNIFORM_FLOATS, "the uniforms are 20 vec4s");
    u
}

fn dot_floats(d: &Dot) -> [f32; DOT_FLOATS] {
    let core = d.core + if d.joined { 2.0 } else { 0.0 };
    [d.at[0], d.at[1], d.axis[0], d.axis[1], d.radius, core, d.color[0], d.color[1], d.color[2], d.color[3]]
}

impl Gpu {
    fn new(device: &wgpu::Device, target: Target) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("visualizer"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("visualizer"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
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
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("visualizer"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let full = |entry: &str, format: wgpu::TextureFormat| {
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
                    targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let lights = |entry: &str, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_dot"),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: (DOT_FLOATS * 4) as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x2, 2 => Float32x4],
                    })],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState { format: target.offscreen, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let component = |src_factor, dst_factor| wgpu::BlendComponent { src_factor, dst_factor, operation: wgpu::BlendOperation::Add };
        use wgpu::BlendFactor::{One, OneMinusSrcAlpha, Zero};
        // Lights add to the color and leave the alpha (where dark spheres are) alone.
        let dot = lights("fs_dot", wgpu::BlendState { color: component(One, One), alpha: component(Zero, One) });
        let shade = lights("fs_shade", wgpu::BlendState { color: component(One, OneMinusSrcAlpha), alpha: component(One, OneMinusSrcAlpha) });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("visualizer"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("visualizer"),
            size: (UNIFORM_FLOATS * 4) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let capacity = 4096;
        Self {
            target,
            layout,
            sampler,
            uniforms,
            dots: Self::dot_buffer(device, capacity),
            capacity,
            feedback: full("fs_feedback", target.offscreen),
            dot,
            shade,
            show: full("fs_show", target.format),
            pictures: None,
            staging: Vec::new(),
        }
    }

    fn dot_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("visualizer lights"),
            size: (capacity * DOT_FLOATS * 4) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Makes the two pictures at `size` (black) when the size changes, and
    /// hands back the old ones: this frame's feedback pass reads the last
    /// picture from them, scaled, so a resize doesn't blank the view.
    fn resize(&mut self, device: &wgpu::Device, size: [u32; 2]) -> Option<Pictures> {
        if self.pictures.as_ref().is_none_or(|p| p.size != size) {
            let view = || {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("visualizer picture"),
                        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: self.target.offscreen,
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
            return self.pictures.replace(Pictures { size, views, reads, current: 0 });
        }
        None
    }
}

impl CallbackTrait for VisualizerCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if self.size_px[0] == 0 || self.size_px[1] == 0 {
            return Vec::new();
        }
        if resources.get::<Gpu>().is_none_or(|g| g.target != self.target) {
            resources.insert(Gpu::new(device, self.target));
        }
        let Some(gpu) = resources.get_mut::<Gpu>() else { return Vec::new() };
        let size = picture_size(self.size_px);
        let mut u = [0u8; UNIFORM_FLOATS * 4];
        for (b, v) in u.as_chunks_mut::<4>().0.iter_mut().zip(uniforms(self, [1.0 / size[0] as f32, 1.0 / size[1] as f32])) {
            b.copy_from_slice(&v.to_ne_bytes());
        }
        queue.write_buffer(&gpu.uniforms, 0, &u);
        let p = &self.picture;
        let counts = [p.dots.len(), p.shades.len(), p.front.len()];
        let total: usize = counts.iter().sum();
        if total > gpu.capacity {
            gpu.capacity = total.next_power_of_two();
            gpu.dots = Gpu::dot_buffer(device, gpu.capacity);
        }
        gpu.staging.clear();
        for d in p.dots.iter().chain(&p.shades).chain(&p.front) {
            put_bytes(&mut gpu.staging, &dot_floats(d));
        }
        if !gpu.staging.is_empty() {
            queue.write_buffer(&gpu.dots, 0, &gpu.staging);
        }

        let (feedback, dot, shade, buffer) = (gpu.feedback.clone(), gpu.dot.clone(), gpu.shade.clone(), gpu.dots.clone());
        let old = gpu.resize(device, size);
        let Some(pics) = gpu.pictures.as_mut() else { return Vec::new() };
        let next = 1 - pics.current;
        // What the feedback pass reads: the last picture, from the old pair
        // just after a resize (sampled by position, so its size doesn't matter).
        let last = old.as_ref().map_or(&pics.reads[pics.current], |o| &o.reads[o.current]);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("visualizer"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &pics.views[next],
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                ..Default::default()
            });
            pass.set_bind_group(0, last, &[]);
            pass.set_pipeline(&feedback);
            pass.draw(0..3, 0..1);
            if total > 0 {
                pass.set_vertex_buffer(0, buffer.slice(..));
                let mut start = 0u32;
                for (count, pipeline) in counts.iter().zip([&dot, &shade, &dot]) {
                    let end = start + *count as u32;
                    if end > start {
                        pass.set_pipeline(pipeline);
                        pass.draw(0..6, start..end);
                    }
                    start = end;
                }
            }
        }
        pics.current = next;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_views_are_drawn_smaller_and_scaled_up() {
        assert_eq!(picture_size([1280, 800]), [1280, 800]);
        assert_eq!(picture_size([2880, 1800]), [1440, 900]);
        assert_eq!(picture_size([1824, 2736]), [960, 1440]);
    }

    #[test]
    fn the_uniforms_fill_their_buffer() {
        let pic = Picture::new(Default::default(), Look::Lit { centre: [0.1; 3], edge: [0.0; 3] }, 1.0);
        let cb =
            VisualizerCallback { target: Target::plain(wgpu::TextureFormat::Bgra8UnormSrgb), size_px: [10, 10], half: [1.6, 1.0], picture: pic, clear: true };
        let u = uniforms(&cb, [0.1, 0.1]);
        assert_eq!(u[0], 0.0, "cleared: nothing carries over");
        assert_eq!(u[6], 1.0, "an sRGB target gets linear color");
        assert_eq!(u[5], 1.5 / 255.0, "8-bit pictures lose a little each frame");
        assert_eq!(&u[72..75], &[0.1; 3], "the backdrop");
    }
}
