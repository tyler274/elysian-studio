//! iced `shader::Program`: Z-up bed, mesh, and orbit camera on Vulkan.

pub use crate::camera::OrbitCamera;

use bambu_config::{BedRect, BedShape};
use bambu_geom::TriangleMesh;
use bambu_preview::{ExtrusionRole, ToolpathBuffer};
use glam::Vec3;
use iced::mouse;
use iced::wgpu;
use iced::widget::shader::{self, Viewport};
use iced::{Event, Rectangle};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

pub const BED_MM: f32 = 256.0;
const PLASTIC: [f32; 3] = [0.93, 0.42, 0.18];
const PAINT_ENFORCER: [f32; 3] = [0.22, 0.86, 0.38];
const PAINT_BLOCKER: [f32; 3] = [0.92, 0.22, 0.28];
const BED: [f32; 3] = [0.16, 0.17, 0.20];
const GRID: [f32; 3] = [0.28, 0.32, 0.38];
const EXCLUDE: [f32; 3] = [0.765, 0.769, 0.769];
const LEFT_ONLY: [f32; 3] = [0.20, 0.30, 0.40];
const RIGHT_ONLY: [f32; 3] = [0.36, 0.26, 0.22];
const LABEL: [f32; 3] = [0.78, 0.78, 0.80];
const AXIS_X: [f32; 3] = [0.92, 0.25, 0.22];
const AXIS_Y: [f32; 3] = [0.28, 0.82, 0.32];
const AXIS_Z: [f32; 3] = [0.28, 0.48, 0.95];
const OUTER_WALL: [f32; 3] = [1.00, 0.86, 0.22];
const INNER_WALL: [f32; 3] = [0.95, 0.52, 0.18];
const INFILL: [f32; 3] = [0.28, 0.78, 0.96];
const SOLID_INFILL: [f32; 3] = [0.95, 0.62, 0.22];
const FLOATING_VERTICAL_SHELL: [f32; 3] = [0.45, 0.72, 0.98];
const TOP_SURFACE: [f32; 3] = [0.98, 0.94, 0.55];
const BOTTOM_SURFACE: [f32; 3] = [0.88, 0.72, 0.28];
const BRIDGE: [f32; 3] = [0.35, 0.55, 0.98];
const SKIRT: [f32; 3] = [0.62, 0.48, 0.88];
const BRIM: [f32; 3] = [0.72, 0.74, 0.78];
const PRIME_TOWER: [f32; 3] = [0.92, 0.28, 0.72];
const SUPPORT: [f32; 3] = [0.18, 0.82, 0.42];
const SUPPORT_INTERFACE: [f32; 3] = [0.42, 0.94, 0.52];
const IRONING: [f32; 3] = [0.92, 0.88, 0.98];

/// Prepare-tab pointer mode. Right-drag orbits; middle-drag pans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaterTool {
    #[default]
    Orbit,
    Move,
    Rotate,
    Scale,
    LayOnFace,
}

impl PlaterTool {
    fn is_transform_drag(self) -> bool {
        matches!(self, Self::Move | Self::Rotate | Self::Scale)
    }
}

/// Selection AABB used to size the XYZ arrows so they stay outside the mesh.
#[derive(Debug, Clone, Copy)]
pub struct AxisGizmo {
    pub origin: Vec3,
    pub half: Vec3,
}

#[derive(Debug)]
pub struct ViewportScene {
    pub adapter_label: String,
    pub camera: OrbitCamera,
    pub mesh: TriangleMesh,
    pub toolpaths: ToolpathBuffer,
    pub preview_layer: u32,
    pub preview_vertices: u32,
    pub hide_infill: bool,
    pub hide_support: bool,
    pub bed_mm: f32,
    pub bed: BedShape,
    /// Keep the solid mesh visible (paint overlay / no toolpaths).
    pub keep_solid: bool,
    pub paint_overlay: Vec<(usize, [f32; 3])>,
    pub tool: PlaterTool,
    pub gizmo: Option<AxisGizmo>,
    gpu: Mutex<Option<CachedGpuMesh>>,
}

#[derive(Clone, Debug)]
struct CachedGpuMesh {
    key: u64,
    solid: Arc<[Vertex]>,
    lines: Arc<[Vertex]>,
    gizmos: Arc<[Vertex]>,
    labels: Arc<[crate::label::LabelVertex]>,
}

impl Default for ViewportScene {
    fn default() -> Self {
        Self::with_cube(String::new())
    }
}

impl ViewportScene {
    pub fn with_cube(adapter_label: String) -> Self {
        Self::with_cube_on_bed(adapter_label, BED_MM)
    }

    pub fn with_cube_on_bed(adapter_label: String, bed_mm: f32) -> Self {
        let bed = BedShape::square(bed_mm.clamp(80.0, 512.0));
        let bed_mm = bed.orbit_mm();
        Self {
            adapter_label,
            camera: OrbitCamera::looking_at_center(
                Vec3::new(bed.center().0, bed.center().1, 0.0),
                bed_mm,
            ),
            mesh: TriangleMesh::cube(20.0),
            toolpaths: ToolpathBuffer::default(),
            preview_layer: 0,
            preview_vertices: 0,
            hide_infill: false,
            hide_support: false,
            bed_mm,
            bed,
            keep_solid: false,
            paint_overlay: Vec::new(),
            tool: PlaterTool::Orbit,
            gizmo: None,
            gpu: Mutex::new(None),
        }
    }

    pub fn set_bed_mm(&mut self, bed_mm: f32) {
        self.set_bed_shape(BedShape::square(bed_mm));
    }

    pub fn set_bed_shape(&mut self, bed: BedShape) {
        self.bed = bed;
        self.bed_mm = self.bed.orbit_mm();
        let (cx, cy) = self.bed.center();
        self.camera = OrbitCamera::looking_at_center(Vec3::new(cx, cy, 0.0), self.bed_mm);
    }

    /// Replace the solid mesh in world space. Does not recenter or move the camera.
    pub fn set_mesh(&mut self, mesh: TriangleMesh) {
        self.mesh = mesh;
        self.toolpaths = ToolpathBuffer::default();
        self.preview_layer = 0;
        self.preview_vertices = 0;
        self.paint_overlay.clear();
    }

    pub fn preview_z(&self) -> f32 {
        self.toolpaths
            .layer_zs
            .get(self.preview_layer as usize)
            .copied()
            .or_else(|| self.toolpaths.layer_zs.last().copied())
            .unwrap_or(f32::MAX)
    }

    pub fn set_toolpaths(&mut self, toolpaths: ToolpathBuffer) {
        self.preview_layer = toolpaths.layer_zs.len().saturating_sub(1) as u32;
        self.preview_vertices = toolpaths.vertices.len() as u32;
        self.toolpaths = toolpaths;
    }

    /// Camera is excluded: orbit/pan must not rebuild or re-upload vertex buffers.
    fn geom_key(&self) -> u64 {
        // `DefaultHasher` is randomly keyed on every `new()`, so the GPU cache
        // would miss (and re-tessellate / `write_buffer`) on every iced redraw.
        let mut hasher = FnvHasher::default();
        self.mesh.vertices.len().hash(&mut hasher);
        self.mesh.indices.len().hash(&mut hasher);
        hash_vec3_samples(&mut hasher, &self.mesh.vertices);
        self.toolpaths.vertices.len().hash(&mut hasher);
        self.preview_layer.hash(&mut hasher);
        self.preview_vertices.hash(&mut hasher);
        self.hide_infill.hash(&mut hasher);
        self.hide_support.hash(&mut hasher);
        self.keep_solid.hash(&mut hasher);
        self.paint_overlay.len().hash(&mut hasher);
        if let Some((idx, color)) = self.paint_overlay.first() {
            idx.hash(&mut hasher);
            color[0].to_bits().hash(&mut hasher);
        }
        if let Some((idx, color)) = self.paint_overlay.last() {
            idx.hash(&mut hasher);
            color[0].to_bits().hash(&mut hasher);
        }
        match self.gizmo {
            Some(g) => {
                1u8.hash(&mut hasher);
                hash_vec3(&mut hasher, g.origin);
                hash_vec3(&mut hasher, g.half);
            }
            None => 0u8.hash(&mut hasher),
        }
        let (x0, y0, x1, y1) = self.bed.printable_aabb();
        x0.to_bits().hash(&mut hasher);
        y0.to_bits().hash(&mut hasher);
        x1.to_bits().hash(&mut hasher);
        y1.to_bits().hash(&mut hasher);
        hasher.finish()
    }

    fn cached_gpu_mesh(&self) -> CachedGpuMesh {
        let key = self.geom_key();
        let mut cache = self.gpu.lock().unwrap_or_else(|err| err.into_inner());
        if cache.as_ref().is_none_or(|cached| cached.key != key) {
            *cache = Some(self.tessellate(key));
        }
        cache.as_ref().expect("tessellate filled the cache").clone()
    }

    fn tessellate(&self, key: u64) -> CachedGpuMesh {
        let mut lines = grid_vertices(&self.bed);
        lines.extend(toolpath_vertices(
            &self.toolpaths,
            self.preview_z(),
            self.preview_vertices as usize,
            self.hide_infill,
            self.hide_support,
        ));
        let mut solid = bed_solids(&self.bed);
        if self.toolpaths.is_empty() || self.keep_solid {
            solid.extend(mesh_vertices(&self.mesh, PLASTIC));
            solid.extend(overlay_vertices(&self.mesh, &self.paint_overlay));
        }
        let gizmos = self
            .gizmo
            .map(|g| gizmo_arrows(g.origin, g.half))
            .unwrap_or_default();
        CachedGpuMesh {
            key,
            solid: Arc::from(solid),
            lines: Arc::from(lines),
            gizmos: Arc::from(gizmos),
            labels: Arc::from(crate::label::plate_labels(&self.bed, LABEL)),
        }
    }
}

/// Deterministic FNV-1a. `std::collections::hash_map::DefaultHasher` is not.
struct FnvHasher(u64);

impl Default for FnvHasher {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for FnvHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(PRIME);
        }
    }
}

fn hash_vec3(hasher: &mut impl Hasher, v: Vec3) {
    v.x.to_bits().hash(hasher);
    v.y.to_bits().hash(hasher);
    v.z.to_bits().hash(hasher);
}

fn hash_vec3_samples(hasher: &mut impl Hasher, verts: &[Vec3]) {
    if verts.is_empty() {
        return;
    }
    let step = (verts.len() / 16).max(1);
    for v in verts.iter().step_by(step) {
        hash_vec3(hasher, *v);
    }
    hash_vec3(hasher, *verts.last().expect("non-empty"));
}

#[derive(Debug, Default)]
pub struct ViewportState {
    dragging: Option<DragKind>,
    last: Option<iced::Point>,
    press: Option<iced::Point>,
}

#[derive(Debug, Clone, Copy)]
enum DragKind {
    Orbit,
    Pan,
    Tool,
}

#[derive(Debug, Clone)]
pub enum ViewportEvent {
    Orbit {
        dx: f32,
        dy: f32,
    },
    /// World-XY delta so the z=0 hit under the cursor stays put (Studio pan).
    Pan {
        world_x: f32,
        world_y: f32,
    },
    /// Pixel pan used when the ray misses the bed.
    PanScreen {
        dx: f32,
        dy: f32,
    },
    Zoom(f32),
    Click {
        ndc_x: f32,
        ndc_y: f32,
        aspect: f32,
    },
    DragStart {
        ndc_x: f32,
        ndc_y: f32,
        aspect: f32,
    },
    Drag {
        ndc_x: f32,
        ndc_y: f32,
        aspect: f32,
    },
    DragEnd,
}

fn is_pan_button(button: mouse::Button) -> bool {
    matches!(button, mouse::Button::Middle | mouse::Button::Other(2))
}

fn pane_aspect(bounds: Rectangle) -> f32 {
    (bounds.width / bounds.height.max(1.0)).max(0.1)
}

fn cursor_ndc(bounds: Rectangle, pos: iced::Point) -> (f32, f32, f32) {
    let aspect = pane_aspect(bounds);
    let ndc_x = ((pos.x - bounds.x) / bounds.width.max(1.0)) * 2.0 - 1.0;
    let ndc_y = 1.0 - ((pos.y - bounds.y) / bounds.height.max(1.0)) * 2.0;
    (ndc_x, ndc_y, aspect)
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
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let pos = cursor.position_over(bounds)?;
                let kind = if is_pan_button(*button) {
                    DragKind::Pan
                } else if *button == mouse::Button::Right
                    || (*button == mouse::Button::Left && !self.tool.is_transform_drag())
                {
                    DragKind::Orbit
                } else if *button == mouse::Button::Left {
                    DragKind::Tool
                } else {
                    return None;
                };
                state.dragging = Some(kind);
                state.last = Some(pos);
                state.press = Some(pos);
                if matches!(kind, DragKind::Tool) {
                    let (ndc_x, ndc_y, aspect) = cursor_ndc(bounds, pos);
                    Some(shader::Action::publish(
                        ViewportEvent::DragStart {
                            ndc_x,
                            ndc_y,
                            aspect,
                        }
                        .into(),
                    ))
                } else {
                    Some(shader::Action::request_redraw())
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(button)) => {
                let kind = state.dragging.take();
                let press = state.press.take();
                state.last = None;
                let click = if let (Some(press), Some(pos)) = (press, cursor.position()) {
                    let dx = pos.x - press.x;
                    let dy = pos.y - press.y;
                    dx * dx + dy * dy < 16.0
                } else {
                    false
                };
                if matches!(kind, Some(DragKind::Tool)) {
                    return Some(shader::Action::publish(ViewportEvent::DragEnd.into()));
                }
                if click && *button == mouse::Button::Left {
                    if let Some(pos) = cursor.position_over(bounds).or(cursor.position()) {
                        let (ndc_x, ndc_y, aspect) = cursor_ndc(bounds, pos);
                        return Some(shader::Action::publish(
                            ViewportEvent::Click {
                                ndc_x,
                                ndc_y,
                                aspect,
                            }
                            .into(),
                        ));
                    }
                }
                None
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => match state.dragging {
                Some(DragKind::Orbit) => {
                    let pos = cursor.position()?;
                    let last = state.last.replace(pos)?;
                    let dx = pos.x - last.x;
                    let dy = pos.y - last.y;
                    Some(
                        shader::Action::publish(ViewportEvent::Orbit { dx, dy }.into())
                            .and_capture(),
                    )
                }
                Some(DragKind::Pan) => {
                    let pos = cursor.position()?;
                    let last = state.last.replace(pos)?;
                    let (nx, ny, aspect) = cursor_ndc(bounds, pos);
                    let (lx, ly, _) = cursor_ndc(bounds, last);
                    let event = match (
                        self.camera.hit_z0(nx, ny, aspect),
                        self.camera.hit_z0(lx, ly, aspect),
                    ) {
                        (Some(cur), Some(prev)) => ViewportEvent::Pan {
                            world_x: prev.x - cur.x,
                            world_y: prev.y - cur.y,
                        },
                        _ => ViewportEvent::PanScreen {
                            dx: pos.x - last.x,
                            dy: pos.y - last.y,
                        },
                    };
                    Some(shader::Action::publish(event.into()).and_capture())
                }
                Some(DragKind::Tool) => {
                    let pos = cursor.position()?;
                    state.last = Some(pos);
                    let (ndc_x, ndc_y, aspect) = cursor_ndc(bounds, pos);
                    Some(
                        shader::Action::publish(
                            ViewportEvent::Drag {
                                ndc_x,
                                ndc_y,
                                aspect,
                            }
                            .into(),
                        )
                        .and_capture(),
                    )
                }
                None => None,
            },
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
        if state.dragging.is_some() {
            mouse::Interaction::Grabbing
        } else if self.tool.is_transform_drag() || self.tool == PlaterTool::LayOnFace {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::Grab
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: mouse::Cursor,
        _bounds: Rectangle,
    ) -> Self::Primitive {
        let mesh = self.cached_gpu_mesh();
        ScenePrimitive {
            camera: self.camera,
            geom_key: mesh.key,
            solid: mesh.solid,
            lines: mesh.lines,
            gizmos: mesh.gizmos,
            labels: mesh.labels,
        }
    }
}

#[derive(Debug)]
pub struct ScenePrimitive {
    camera: OrbitCamera,
    geom_key: u64,
    solid: Arc<[Vertex]>,
    lines: Arc<[Vertex]>,
    gizmos: Arc<[Vertex]>,
    labels: Arc<[crate::label::LabelVertex]>,
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
    gizmo_pipeline: wgpu::RenderPipeline,
    label_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    label_bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
    vertex_buf: wgpu::Buffer,
    vertex_capacity: u64,
    line_buf: wgpu::Buffer,
    line_capacity: u64,
    gizmo_buf: wgpu::Buffer,
    gizmo_capacity: u64,
    label_buf: wgpu::Buffer,
    label_capacity: u64,
    solid_count: u32,
    line_count: u32,
    gizmo_count: u32,
    label_count: u32,
    depth_view: Option<wgpu::TextureView>,
    depth_size: (u32, u32),
    _atlas: wgpu::Texture,
    uploaded_key: u64,
}

impl shader::Pipeline for ScenePipeline {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self::create(device, queue, format)
    }
}

impl ScenePipeline {
    fn create(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
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

        let gizmo_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bambu-gpu-gizmo-pipeline"),
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
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
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
        let gizmo_buf = empty_vertex_buffer(device, 1024, "bambu-gpu-gizmo-verts");
        let (label_pipeline, label_bind_group, label_buf, atlas_texture) =
            label_gpu(device, queue, format, &uniform_buf);

        Self {
            pipeline,
            line_pipeline,
            gizmo_pipeline,
            label_pipeline,
            bind_group,
            label_bind_group,
            uniform_buf,
            vertex_buf,
            vertex_capacity: 4096,
            line_buf,
            line_capacity: 4096,
            gizmo_buf,
            gizmo_capacity: 1024,
            label_buf,
            label_capacity: 256,
            solid_count: 0,
            line_count: 0,
            gizmo_count: 0,
            label_count: 0,
            depth_view: None,
            depth_size: (0, 0),
            _atlas: atlas_texture,
            uploaded_key: u64::MAX,
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
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let size = viewport.physical_size();
        pipeline.ensure_depth(device, size.width, size.height);

        // Match the pane we `set_viewport` to in `render`, not the full window.
        let aspect = pane_aspect(*bounds);
        let proj = glam::Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, aspect, 1.0, 4000.0);
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

        if pipeline.uploaded_key != self.geom_key {
            upload_vertices(
                device,
                queue,
                &mut pipeline.vertex_buf,
                &mut pipeline.vertex_capacity,
                &self.solid,
                "bambu-gpu-solid-verts",
            );
            upload_vertices(
                device,
                queue,
                &mut pipeline.line_buf,
                &mut pipeline.line_capacity,
                &self.lines,
                "bambu-gpu-line-verts",
            );
            upload_vertices(
                device,
                queue,
                &mut pipeline.gizmo_buf,
                &mut pipeline.gizmo_capacity,
                &self.gizmos,
                "bambu-gpu-gizmo-verts",
            );
            upload_labels(
                device,
                queue,
                &mut pipeline.label_buf,
                &mut pipeline.label_capacity,
                &self.labels,
            );
            pipeline.uploaded_key = self.geom_key;
        }
        pipeline.solid_count = self.solid.len() as u32;
        pipeline.line_count = self.lines.len() as u32;
        pipeline.gizmo_count = self.gizmos.len() as u32;
        pipeline.label_count = self.labels.len() as u32;
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

        // iced's `draw()` path already sets this; our depth pass does not.
        // Without it, NDC maps to the full window and the cube looks stretched.
        let vp_w = clip_bounds.width.max(1) as f32;
        let vp_h = clip_bounds.height.max(1) as f32;
        pass.set_viewport(
            clip_bounds.x as f32,
            clip_bounds.y as f32,
            vp_w,
            vp_h,
            0.0,
            1.0,
        );
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
        if pipeline.gizmo_count > 0 {
            pass.set_pipeline(&pipeline.gizmo_pipeline);
            pass.set_bind_group(0, &pipeline.bind_group, &[]);
            pass.set_vertex_buffer(0, pipeline.gizmo_buf.slice(..));
            pass.draw(0..pipeline.gizmo_count, 0..1);
        }
        if pipeline.label_count > 0 {
            pass.set_pipeline(&pipeline.label_pipeline);
            pass.set_bind_group(0, &pipeline.label_bind_group, &[]);
            pass.set_vertex_buffer(0, pipeline.label_buf.slice(..));
            pass.draw(0..pipeline.label_count, 0..1);
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

fn empty_label_buffer(device: &wgpu::Device, count: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bambu-gpu-label-verts"),
        size: count * std::mem::size_of::<crate::label::LabelVertex>() as u64,
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

fn upload_labels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut wgpu::Buffer,
    capacity: &mut u64,
    verts: &[crate::label::LabelVertex],
) {
    let needed = verts.len() as u64;
    if needed == 0 {
        return;
    }
    if needed > *capacity {
        *capacity = needed.next_power_of_two().max(64);
        *buffer = empty_label_buffer(device, *capacity);
    }
    queue.write_buffer(buffer, 0, bytemuck::cast_slice(verts));
}

fn label_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    uniform_buf: &wgpu::Buffer,
) -> (
    wgpu::RenderPipeline,
    wgpu::BindGroup,
    wgpu::Buffer,
    wgpu::Texture,
) {
    let atlas = crate::label::atlas();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bambu-gpu-label-atlas"),
        size: wgpu::Extent3d {
            width: atlas.width,
            height: atlas.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &atlas.pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(atlas.stride),
            rows_per_image: Some(atlas.height),
        },
        wgpu::Extent3d {
            width: atlas.width,
            height: atlas.height,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("bambu-gpu-label-sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("bambu-gpu-label"),
        source: wgpu::ShaderSource::Wgsl(include_str!("label.wgsl").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bambu-gpu-label-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bambu-gpu-label-bg"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("bambu-gpu-label-pl"),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let vertex_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<crate::label::LabelVertex>() as wgpu::BufferAddress,
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
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 20,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32x3,
            },
        ],
    };
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("bambu-gpu-label-pipeline"),
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
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });
    (
        pipeline,
        bind_group,
        empty_label_buffer(device, 256),
        texture,
    )
}

fn overlay_vertices(mesh: &TriangleMesh, paints: &[(usize, [f32; 3])]) -> Vec<Vertex> {
    let center = mesh_center(mesh);
    let mut out = Vec::new();
    for &(i, color) in paints {
        let Some(idx) = mesh.indices.get(i).copied() else {
            continue;
        };
        let [a, b, c] = mesh.triangle(idx);
        let (pts, n) = outward_triangle(a, b, c, center);
        let lift = n * 0.08;
        let n3 = [n.x, n.y, n.z];
        for p in pts {
            let p = p + lift;
            out.push(Vertex {
                position: [p.x, p.y, p.z],
                normal: n3,
                color,
            });
        }
    }
    out
}

pub fn paint_overlay_color(enforcer: bool) -> [f32; 3] {
    if enforcer {
        PAINT_ENFORCER
    } else {
        PAINT_BLOCKER
    }
}

fn mesh_center(mesh: &TriangleMesh) -> Vec3 {
    mesh.aabb()
        .map(|a| (a.min + a.max) * 0.5)
        .unwrap_or(Vec3::ZERO)
}

/// CCW from the outside of the AABB so GPU back-face culling keeps the shell.
pub fn outward_triangle(a: Vec3, b: Vec3, c: Vec3, center: Vec3) -> ([Vec3; 3], Vec3) {
    let n = (b - a).cross(c - a);
    let centroid = (a + b + c) / 3.0;
    if n.dot(centroid - center) < 0.0 {
        ([a, c, b], (-n).normalize_or_zero())
    } else {
        ([a, b, c], n.normalize_or_zero())
    }
}

fn mesh_vertices(mesh: &TriangleMesh, color: [f32; 3]) -> Vec<Vertex> {
    let center = mesh_center(mesh);
    let mut out = Vec::with_capacity(mesh.indices.len() * 3);
    for idx in &mesh.indices {
        let [a, b, c] = mesh.triangle(*idx);
        let (pts, n) = outward_triangle(a, b, c, center);
        let n = [n.x, n.y, n.z];
        for p in pts {
            out.push(Vertex {
                position: [p.x, p.y, p.z],
                normal: n,
                color,
            });
        }
    }
    out
}

fn bed_solids(bed: &BedShape) -> Vec<Vertex> {
    let mut out = fill_poly(&bed.printable, -0.15, BED);
    let (left, right) = bed.visible_only_rects();
    if let Some(rect) = left {
        out.extend(fill_rect(rect, -0.08, LEFT_ONLY));
    }
    if let Some(rect) = right {
        out.extend(fill_rect(rect, -0.08, RIGHT_ONLY));
    }
    if bed.exclude.len() >= 3 {
        out.extend(fill_poly(&bed.exclude, -0.05, EXCLUDE));
    }
    out
}

fn fill_rect(rect: BedRect, z: f32, color: [f32; 3]) -> Vec<Vertex> {
    fill_poly(
        &[
            (rect.x, rect.y),
            (rect.max_x(), rect.y),
            (rect.max_x(), rect.max_y()),
            (rect.x, rect.max_y()),
        ],
        z,
        color,
    )
}

fn fill_poly(pts: &[(f32, f32)], z: f32, color: [f32; 3]) -> Vec<Vertex> {
    if pts.len() < 3 {
        return Vec::new();
    }
    let n = [0.0, 0.0, 1.0];
    let mut out = Vec::with_capacity((pts.len() - 2) * 3);
    for i in 1..pts.len().saturating_sub(1) {
        for &(x, y) in &[pts[0], pts[i], pts[i + 1]] {
            out.push(Vertex {
                position: [x, y, z],
                normal: n,
                color,
            });
        }
    }
    out
}

fn grid_vertices(bed: &BedShape) -> Vec<Vertex> {
    let (x0, y0, x1, y1) = bed.printable_aabb();
    let step = 10.0_f32;
    let z = 0.05_f32;
    let n = [0.0, 0.0, 1.0];
    let mut out = Vec::new();
    let mut x = (x0 / step).floor() * step;
    while x <= x1 + 0.01 {
        if x >= x0 - 0.01 {
            push_line(&mut out, [x, y0, z], [x, y1, z], n, GRID);
        }
        x += step;
    }
    let mut y = (y0 / step).floor() * step;
    while y <= y1 + 0.01 {
        if y >= y0 - 0.01 {
            push_line(&mut out, [x0, y, z], [x1, y, z], n, GRID);
        }
        y += step;
    }
    out
}

fn gizmo_arrows(origin: Vec3, half: Vec3) -> Vec<Vertex> {
    let mut out = Vec::new();
    axis_arrow(&mut out, origin, Vec3::X, axis_len(half.x), AXIS_X);
    axis_arrow(&mut out, origin, Vec3::Y, axis_len(half.y), AXIS_Y);
    axis_arrow(&mut out, origin, Vec3::Z, axis_len(half.z), AXIS_Z);
    out
}

fn axis_len(half: f32) -> f32 {
    (half.abs() * 1.35 + 12.0).max(16.0)
}

fn axis_arrow(out: &mut Vec<Vertex>, origin: Vec3, dir: Vec3, len: f32, color: [f32; 3]) {
    let dir = dir.normalize_or_zero();
    if dir.length_squared() < 1e-8 || len < 1.0 {
        return;
    }
    let head_h = (len * 0.22).clamp(5.0, 28.0);
    let shaft_len = (len - head_h).max(len * 0.6);
    let shaft_r = (len * 0.032).clamp(0.55, 4.0);
    let head_r = shaft_r * 2.7;
    let shaft_end = origin + dir * shaft_len;
    let tip = origin + dir * (shaft_len + head_h);
    push_cylinder(out, origin, shaft_end, dir, shaft_r, color, 12);
    push_cone(out, shaft_end, tip, dir, head_r, color, 12);
}

fn axis_basis(dir: Vec3) -> (Vec3, Vec3) {
    let helper = if dir.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let right = dir.cross(helper).normalize_or_zero();
    let up = right.cross(dir).normalize_or_zero();
    (right, up)
}

fn push_cylinder(
    out: &mut Vec<Vertex>,
    a: Vec3,
    b: Vec3,
    dir: Vec3,
    radius: f32,
    color: [f32; 3],
    segs: usize,
) {
    let (right, up) = axis_basis(dir);
    let segs = segs.max(6);
    for i in 0..segs {
        let t0 = i as f32 / segs as f32 * std::f32::consts::TAU;
        let t1 = (i + 1) as f32 / segs as f32 * std::f32::consts::TAU;
        let r0 = right * t0.cos() + up * t0.sin();
        let r1 = right * t1.cos() + up * t1.sin();
        let a0 = a + r0 * radius;
        let a1 = a + r1 * radius;
        let b0 = b + r0 * radius;
        let b1 = b + r1 * radius;
        push_lit_tri(out, a0, a1, b1, color, r0 + r1);
        push_lit_tri(out, a0, b1, b0, color, r0 + r1);
    }
}

fn push_cone(
    out: &mut Vec<Vertex>,
    base: Vec3,
    tip: Vec3,
    dir: Vec3,
    radius: f32,
    color: [f32; 3],
    segs: usize,
) {
    let (right, up) = axis_basis(dir);
    let segs = segs.max(6);
    for i in 0..segs {
        let t0 = i as f32 / segs as f32 * std::f32::consts::TAU;
        let t1 = (i + 1) as f32 / segs as f32 * std::f32::consts::TAU;
        let r0 = right * t0.cos() + up * t0.sin();
        let r1 = right * t1.cos() + up * t1.sin();
        let a0 = base + r0 * radius;
        let a1 = base + r1 * radius;
        push_lit_tri(out, a0, a1, tip, color, r0 + r1 + dir);
        push_lit_tri(out, a0, a1, base, color, -dir);
    }
}

fn push_lit_tri(out: &mut Vec<Vertex>, a: Vec3, b: Vec3, c: Vec3, color: [f32; 3], outward: Vec3) {
    let mut pts = [a, b, c];
    let mut n = (b - a).cross(c - a);
    if n.dot(outward) < 0.0 {
        pts.swap(1, 2);
        n = -n;
    }
    let n = n.normalize_or_zero();
    let n3 = [n.x, n.y, n.z];
    for p in pts {
        out.push(Vertex {
            position: [p.x, p.y, p.z],
            normal: n3,
            color,
        });
    }
}

fn toolpath_vertices(
    buf: &ToolpathBuffer,
    max_z: f32,
    max_vertices: usize,
    hide_infill: bool,
    hide_support: bool,
) -> Vec<Vertex> {
    let n = [0.0, 0.0, 1.0];
    buf.visible(max_z, max_vertices, |role| match role {
        ExtrusionRole::Infill | ExtrusionRole::SolidInfill if hide_infill => true,
        ExtrusionRole::Support | ExtrusionRole::SupportInterface if hide_support => true,
        _ => false,
    })
    .map(|v| Vertex {
        position: [v.position.x, v.position.y, v.position.z + 0.08],
        normal: n,
        color: match v.role {
            ExtrusionRole::OuterWall => OUTER_WALL,
            ExtrusionRole::InnerWall => INNER_WALL,
            ExtrusionRole::Infill => INFILL,
            ExtrusionRole::SolidInfill => SOLID_INFILL,
            ExtrusionRole::FloatingVerticalShell => FLOATING_VERTICAL_SHELL,
            ExtrusionRole::TopSurface => TOP_SURFACE,
            ExtrusionRole::BottomSurface => BOTTOM_SURFACE,
            ExtrusionRole::Bridge => BRIDGE,
            ExtrusionRole::Skirt => SKIRT,
            ExtrusionRole::Brim => BRIM,
            ExtrusionRole::PrimeTower => PRIME_TOWER,
            ExtrusionRole::Support => SUPPORT,
            ExtrusionRole::SupportInterface => SUPPORT_INTERFACE,
            ExtrusionRole::Ironing => IRONING,
        },
    })
    .collect()
}

fn push_line(out: &mut Vec<Vertex>, a: [f32; 3], b: [f32; 3], normal: [f32; 3], color: [f32; 3]) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_geom::TriangleMesh;

    #[test]
    fn cube_display_normals_point_outward() {
        let mesh = TriangleMesh::cube(20.0);
        let center = Vec3::splat(10.0);
        let mut saw_top = false;
        for idx in &mesh.indices {
            let [a, b, c] = mesh.triangle(*idx);
            let (_, n) = outward_triangle(a, b, c, center);
            let centroid = (a + b + c) / 3.0;
            assert!(
                n.dot(centroid - center) > 0.0,
                "display normal must point away from the cube center"
            );
            if centroid.z > 19.0 {
                assert!(n.z > 0.8, "top face normal {n:?}");
                saw_top = true;
            }
        }
        assert!(saw_top);
    }

    #[test]
    fn h2c_bed_emits_left_only_fill() {
        let bed = BedShape {
            printable: vec![(0.0, 0.0), (330.0, 0.0), (330.0, 320.0), (0.0, 320.0)],
            exclude: Vec::new(),
            extruder_areas: vec![
                vec![(0.0, 0.0), (325.0, 0.0), (325.0, 320.0), (0.0, 320.0)],
                vec![(25.0, 0.0), (330.0, 0.0), (330.0, 320.0), (25.0, 320.0)],
            ],
        };
        let solids = bed_solids(&bed);
        assert!(solids.len() >= 12, "printable + left-only quads");
        assert!(
            solids.iter().any(|v| v.color == LEFT_ONLY),
            "left-only strip should be filled"
        );
        assert!(!crate::label::plate_labels(&bed, LABEL).is_empty());
    }

    #[test]
    fn gizmo_arrows_are_solid_not_lines() {
        let verts = gizmo_arrows(Vec3::ZERO, Vec3::splat(10.0));
        assert!(
            verts.len() > 36,
            "shaft + cone should be tessellated, got {}",
            verts.len()
        );
        assert_eq!(verts.len() % 3, 0);
    }

    #[test]
    fn gizmo_arrows_grow_with_aabb() {
        let small = gizmo_arrows(Vec3::ZERO, Vec3::splat(10.0));
        let large = gizmo_arrows(Vec3::ZERO, Vec3::splat(40.0));
        let tip = |verts: &[Vertex]| verts.iter().map(|v| v.position[0]).fold(f32::MIN, f32::max);
        assert!(
            tip(&large) > tip(&small) + 20.0,
            "X arrow should lengthen with the mesh, small {} large {}",
            tip(&small),
            tip(&large)
        );
        assert!(tip(&large) > 40.0 * 0.5, "tip must stick out past the AABB");
    }

    #[test]
    fn camera_motion_does_not_change_geom_key() {
        let mut scene = ViewportScene::with_cube("test".into());
        assert_eq!(scene.geom_key(), scene.geom_key());
        let before = scene.geom_key();
        scene.camera.orbit(12.0, -4.0);
        scene.camera.pan_xy(5.0, -3.0);
        scene.camera.zoom(-1.0);
        assert_eq!(before, scene.geom_key());
        let first = scene.cached_gpu_mesh();
        scene.camera.orbit(-3.0, 1.0);
        let second = scene.cached_gpu_mesh();
        assert!(
            Arc::ptr_eq(&first.solid, &second.solid) && Arc::ptr_eq(&first.lines, &second.lines),
            "orbit must reuse tessellated GPU meshes"
        );
        scene.set_mesh(TriangleMesh::cube(24.0));
        assert_ne!(before, scene.geom_key());
    }
}
