//! iced `shader::Program`: Z-up bed, mesh, and orbit camera on Vulkan.

pub use crate::camera::OrbitCamera;

use bambu_geom::TriangleMesh;
use bambu_preview::{ExtrusionRole, ToolpathBuffer};
use iced::mouse;
use iced::widget::shader::{self, Viewport};
use iced::wgpu;
use iced::{Event, Rectangle};

pub const BED_MM: f32 = 256.0;
const PLASTIC: [f32; 3] = [0.93, 0.42, 0.18];
const BED: [f32; 3] = [0.16, 0.17, 0.20];
const GRID: [f32; 3] = [0.28, 0.32, 0.38];
const OUTER_WALL: [f32; 3] = [1.00, 0.86, 0.22];
const INNER_WALL: [f32; 3] = [0.95, 0.52, 0.18];
const INFILL: [f32; 3] = [0.28, 0.78, 0.96];
const SKIRT: [f32; 3] = [0.62, 0.48, 0.88];
const BRIM: [f32; 3] = [0.72, 0.74, 0.78];
const SUPPORT: [f32; 3] = [0.18, 0.82, 0.42];
const SUPPORT_INTERFACE: [f32; 3] = [0.42, 0.94, 0.52];

#[derive(Debug, Clone)]
pub struct ViewportScene {
    pub adapter_label: String,
    pub camera: OrbitCamera,
    pub mesh: TriangleMesh,
    pub toolpaths: ToolpathBuffer,
}

impl Default for ViewportScene {
    fn default() -> Self {
        Self::with_cube(String::new())
    }
}

impl ViewportScene {
    pub fn with_cube(adapter_label: String) -> Self {
        let mut mesh = TriangleMesh::cube(20.0);
        mesh.place_on_bed(BED_MM);
        Self {
            adapter_label,
            camera: OrbitCamera::looking_at_bed(BED_MM),
            mesh,
            toolpaths: ToolpathBuffer::default(),
        }
    }

    pub fn set_mesh(&mut self, mut mesh: TriangleMesh) {
        mesh.place_on_bed(BED_MM);
        self.mesh = mesh;
        self.toolpaths = ToolpathBuffer::default();
        self.camera = OrbitCamera::looking_at_bed(BED_MM);
    }

    pub fn set_toolpaths(&mut self, toolpaths: ToolpathBuffer) {
        self.toolpaths = toolpaths;
    }
}

#[derive(Debug, Default)]
pub struct ViewportState {
    dragging: bool,
    last: Option<iced::Point>,
}

#[derive(Debug, Clone)]
pub enum ViewportEvent {
    Orbit { dx: f32, dy: f32 },
    Zoom(f32),
}

impl<Message> shader::Program<Message> for ViewportScene
where
    Message: From<ViewportEvent> + Clone + 'static,
{
    type State = ViewportState;
    type Primitive = ScenePrimitive;

    fn update(
        &self,
        state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<shader::Action<Message>> {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if cursor.position_over(bounds).is_some() {
                    state.dragging = true;
                    state.last = cursor.position();
                    Some(shader::Action::request_redraw())
                } else {
                    None
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.dragging = false;
                state.last = None;
                None
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => {
                let pos = cursor.position()?;
                let last = state.last.replace(pos)?;
                let dx = pos.x - last.x;
                let dy = pos.y - last.y;
                Some(
                    shader::Action::publish(
                        ViewportEvent::Orbit { dx, dy }.into(),
                    )
                    .and_capture(),
                )
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                cursor.position_over(bounds)?;
                let y = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                Some(shader::Action::publish(ViewportEvent::Zoom(y).into()))
            }
            _ => None,
        }
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.dragging {
            mouse::Interaction::Grabbing
        } else {
            mouse::Interaction::Grab
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: mouse::Cursor,
        bounds: Rectangle,
    ) -> Self::Primitive {
        let mut lines = grid_vertices(BED_MM);
        lines.extend(toolpath_vertices(&self.toolpaths));
        let solid = if self.toolpaths.is_empty() {
            solid_vertices(&self.mesh)
        } else {
            // Hide the solid mesh so inset walls/infill are not occluded.
            bed_quad(BED_MM)
        };
        ScenePrimitive {
            aspect: (bounds.width / bounds.height.max(1.0)).max(0.1),
            camera: self.camera,
            solid,
            lines,
        }
    }
}

#[derive(Debug)]
pub struct ScenePrimitive {
    aspect: f32,
    camera: OrbitCamera,
    solid: Vec<Vertex>,
    lines: Vec<Vertex>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    light_dir: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    color: [f32; 3],
}

pub struct ScenePipeline {
    pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
    vertex_buf: wgpu::Buffer,
    vertex_capacity: u64,
    line_buf: wgpu::Buffer,
    line_capacity: u64,
    solid_count: u32,
    line_count: u32,
    depth_view: Option<wgpu::TextureView>,
    depth_size: (u32, u32),
}

impl shader::Pipeline for ScenePipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self::create(device, format)
    }
}

impl ScenePipeline {
    fn create(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bambu-gpu-solid"),
            source: wgpu::ShaderSource::Wgsl(include_str!("solid.wgsl").into()),
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bambu-gpu-uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bambu-gpu-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bambu-gpu-bg"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bambu-gpu-pl"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bambu-gpu-solid-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: std::slice::from_ref(&vertex_layout),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bambu-gpu-line-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[vertex_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let vertex_buf = empty_vertex_buffer(device, 4096, "bambu-gpu-solid-verts");
        let line_buf = empty_vertex_buffer(device, 4096, "bambu-gpu-line-verts");

        Self {
            pipeline,
            line_pipeline,
            bind_group,
            uniform_buf,
            vertex_buf,
            vertex_capacity: 4096,
            line_buf,
            line_capacity: 4096,
            solid_count: 0,
            line_count: 0,
            depth_view: None,
            depth_size: (0, 0),
        }
    }

    fn ensure_depth(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.depth_size == (width, height) && self.depth_view.is_some() {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bambu-gpu-depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.depth_view = Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.depth_size = (width, height);
    }
}

impl shader::Primitive for ScenePrimitive {
    type Pipeline = ScenePipeline;

    fn prepare(
        &self,
        pipeline: &mut ScenePipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let size = viewport.physical_size();
        pipeline.ensure_depth(device, size.width, size.height);

        let proj = glam::Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, self.aspect, 1.0, 4000.0);
        let view = self.camera.view_matrix();
        let model = glam::Mat4::IDENTITY;
        let mvp = proj * view * model;
        let light = (self.camera.eye() - self.camera.target).normalize();
        queue.write_buffer(
            &pipeline.uniform_buf,
            0,
            bytemuck::bytes_of(&Uniforms {
                mvp: mvp.to_cols_array_2d(),
                model: model.to_cols_array_2d(),
                light_dir: [light.x, light.y, light.z, 0.0],
            }),
        );

        upload_vertices(
            device,
            queue,
            &mut pipeline.vertex_buf,
            &mut pipeline.vertex_capacity,
            &self.solid,
            "bambu-gpu-solid-verts",
        );
        pipeline.solid_count = self.solid.len() as u32;

        upload_vertices(
            device,
            queue,
            &mut pipeline.line_buf,
            &mut pipeline.line_capacity,
            &self.lines,
            "bambu-gpu-line-verts",
        );
        pipeline.line_count = self.lines.len() as u32;
    }

    fn draw(&self, _pipeline: &ScenePipeline, _render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        false
    }

    fn render(
        &self,
        pipeline: &ScenePipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let Some(depth_view) = pipeline.depth_view.as_ref() else {
            return;
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("bambu-gpu-viewport"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.07,
                        g: 0.075,
                        b: 0.09,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_scissor_rect(
            clip_bounds.x,
            clip_bounds.y,
            clip_bounds.width.max(1),
            clip_bounds.height.max(1),
        );

        if pipeline.solid_count > 0 {
            pass.set_pipeline(&pipeline.pipeline);
            pass.set_bind_group(0, &pipeline.bind_group, &[]);
            pass.set_vertex_buffer(0, pipeline.vertex_buf.slice(..));
            pass.draw(0..pipeline.solid_count, 0..1);
        }
        if pipeline.line_count > 0 {
            pass.set_pipeline(&pipeline.line_pipeline);
            pass.set_bind_group(0, &pipeline.bind_group, &[]);
            pass.set_vertex_buffer(0, pipeline.line_buf.slice(..));
            pass.draw(0..pipeline.line_count, 0..1);
        }
    }
}

fn empty_vertex_buffer(device: &wgpu::Device, count: u64, label: &str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: count * std::mem::size_of::<Vertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn upload_vertices(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut wgpu::Buffer,
    capacity: &mut u64,
    verts: &[Vertex],
    label: &str,
) {
    let needed = verts.len() as u64;
    if needed == 0 {
        return;
    }
    if needed > *capacity {
        *capacity = needed.next_power_of_two().max(64);
        *buffer = empty_vertex_buffer(device, *capacity, label);
    }
    queue.write_buffer(buffer, 0, bytemuck::cast_slice(verts));
}

fn solid_vertices(mesh: &TriangleMesh) -> Vec<Vertex> {
    let mut out = bed_quad(BED_MM);
    out.extend(mesh_vertices(mesh, PLASTIC));
    out
}

fn mesh_vertices(mesh: &TriangleMesh, color: [f32; 3]) -> Vec<Vertex> {
    let mut out = Vec::with_capacity(mesh.indices.len() * 3);
    for idx in &mesh.indices {
        let [a, b, c] = mesh.triangle(*idx);
        let n = (b - a).cross(c - a).normalize_or_zero();
        let n = [n.x, n.y, n.z];
        for p in [a, b, c] {
            out.push(Vertex {
                position: [p.x, p.y, p.z],
                normal: n,
                color,
            });
        }
    }
    out
}

fn bed_quad(bed: f32) -> Vec<Vertex> {
    let z = -0.15_f32;
    let n = [0.0, 0.0, 1.0];
    let pts = [
        [0.0, 0.0, z],
        [bed, 0.0, z],
        [bed, bed, z],
        [0.0, bed, z],
    ];
    let tris = [[0, 1, 2], [0, 2, 3]];
    let mut out = Vec::with_capacity(6);
    for t in tris {
        for i in t {
            out.push(Vertex {
                position: pts[i],
                normal: n,
                color: BED,
            });
        }
    }
    out
}

fn grid_vertices(bed: f32) -> Vec<Vertex> {
    let step = 10.0_f32;
    let z = 0.05_f32;
    let n = [0.0, 0.0, 1.0];
    let mut out = Vec::new();
    let mut x = 0.0;
    while x <= bed + 0.01 {
        push_line(&mut out, [x, 0.0, z], [x, bed, z], n, GRID);
        x += step;
    }
    let mut y = 0.0;
    while y <= bed + 0.01 {
        push_line(&mut out, [0.0, y, z], [bed, y, z], n, GRID);
        y += step;
    }
    out
}

fn toolpath_vertices(buf: &ToolpathBuffer) -> Vec<Vertex> {
    let n = [0.0, 0.0, 1.0];
    buf.vertices
        .iter()
        .map(|v| Vertex {
            position: [v.position.x, v.position.y, v.position.z + 0.08],
            normal: n,
            color: match v.role {
                ExtrusionRole::OuterWall => OUTER_WALL,
                ExtrusionRole::InnerWall => INNER_WALL,
                ExtrusionRole::Infill => INFILL,
                ExtrusionRole::Skirt => SKIRT,
                ExtrusionRole::Brim => BRIM,
                ExtrusionRole::Support => SUPPORT,
                ExtrusionRole::SupportInterface => SUPPORT_INTERFACE,
            },
        })
        .collect()
}

fn push_line(
    out: &mut Vec<Vertex>,
    a: [f32; 3],
    b: [f32; 3],
    normal: [f32; 3],
    color: [f32; 3],
) {
    out.push(Vertex {
        position: a,
        normal,
        color,
    });
    out.push(Vertex {
        position: b,
        normal,
        color,
    });
}
