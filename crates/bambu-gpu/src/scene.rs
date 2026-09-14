//! iced `shader::Program`: Z-up bed, mesh, and orbit camera on Vulkan.

pub use crate::camera::{CameraView, OrbitCamera};

use bambu_config::{BedRect, BedShape};
use bambu_geom::{Aabb3, TriangleMesh};
use bambu_preview::{ExtrusionRole, ToolpathBuffer};
use glam::{Mat4, Vec3};
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

const SAMPLE_COUNT: u32 = 4;

fn msaa_state() -> wgpu::MultisampleState {
    wgpu::MultisampleState {
        count: SAMPLE_COUNT,
        mask: !0,
        alpha_to_coverage_enabled: false,
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoAxis {
    X,
    Y,
    Z,
}

impl AxisGizmo {
    pub fn pick_axis(self, origin: Vec3, dir: Vec3) -> Option<GizmoAxis> {
        let dir = dir.normalize_or_zero();
        if dir.length_squared() < 1e-12 {
            return None;
        }
        let inv = Vec3::new(
            if dir.x.abs() > 1e-8 {
                1.0 / dir.x
            } else {
                f32::INFINITY
            },
            if dir.y.abs() > 1e-8 {
                1.0 / dir.y
            } else {
                f32::INFINITY
            },
            if dir.z.abs() > 1e-8 {
                1.0 / dir.z
            } else {
                f32::INFINITY
            },
        );
        let mut best = f32::MAX;
        let mut hit = None;
        for (axis, unit, half) in [
            (GizmoAxis::X, Vec3::X, self.half.x),
            (GizmoAxis::Y, Vec3::Y, self.half.y),
            (GizmoAxis::Z, Vec3::Z, self.half.z),
        ] {
            let len = axis_len(half);
            let radius = (len * 0.08).clamp(2.0, 10.0);
            let tip = self.origin + unit * len;
            let aabb = Aabb3 {
                min: self.origin.min(tip) - Vec3::splat(radius),
                max: self.origin.max(tip) + Vec3::splat(radius),
            };
            if aabb.intersects_ray(origin, inv) {
                let t = (self.origin - origin).dot(dir);
                if t > 0.0 && t < best {
                    best = t;
                    hit = Some(axis);
                }
            }
        }
        hit
    }
}

/// One printable volume in world space, clustered for Fast raster.
#[derive(Debug, Clone)]
pub struct SceneSolid {
    pub mesh: TriangleMesh,
    pub meshlets: Vec<crate::meshlet::Meshlet>,
}

impl SceneSolid {
    pub fn from_mesh(mesh: TriangleMesh) -> Self {
        let meshlets = crate::meshlet::clusterize(&mesh);
        Self { mesh, meshlets }
    }
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
    /// Per-volume world meshes (not flattened).
    pub solids: Vec<SceneSolid>,
    /// Hardware `ray_query` shading when the iced device has RT.
    pub realistic: bool,
    gpu: Mutex<Option<CachedGpuMesh>>,
}

#[derive(Clone, Copy, Debug)]
struct MeshletDraw {
    start: u32,
    count: u32,
    aabb: Aabb3,
}

#[derive(Clone, Debug)]
struct CachedGpuMesh {
    key: u64,
    solid: Arc<[Vertex]>,
    lines: Arc<[Vertex]>,
    gizmos: Arc<[Vertex]>,
    labels: Arc<[crate::label::LabelVertex]>,
    meshlets: Arc<[MeshletDraw]>,
    bed_verts: u32,
    overlay_start: u32,
    overlay_count: u32,
    rt_positions: Arc<[[f32; 4]]>,
    rt_indices: Arc<[u32]>,
    rt_instances: Arc<[crate::rt::RtInstance]>,
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
            solids: vec![SceneSolid::from_mesh(TriangleMesh::cube(20.0))],
            realistic: true,
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
        self.solids = vec![SceneSolid::from_mesh(mesh.clone())];
        self.mesh = mesh;
        self.toolpaths = ToolpathBuffer::default();
        self.preview_layer = 0;
        self.preview_vertices = 0;
        self.paint_overlay.clear();
    }

    /// Replace plate volumes without merging them into one GPU mesh.
    pub fn set_solids(&mut self, meshes: Vec<TriangleMesh>) {
        self.solids = meshes.into_iter().map(SceneSolid::from_mesh).collect();
        self.mesh = TriangleMesh::default();
        for solid in &self.solids {
            self.mesh.append(&solid.mesh);
        }
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
        self.solids.len().hash(&mut hasher);
        for solid in &self.solids {
            solid.mesh.vertices.len().hash(&mut hasher);
            solid.mesh.indices.len().hash(&mut hasher);
            hash_vec3_samples(&mut hasher, &solid.mesh.vertices);
            solid.meshlets.len().hash(&mut hasher);
        }
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
        let bed_verts = solid.len() as u32;
        let mut meshlets = Vec::new();
        let show_solids = self.toolpaths.is_empty() || self.keep_solid;
        if show_solids {
            for s in &self.solids {
                if s.meshlets.is_empty() {
                    let start = solid.len() as u32;
                    let verts = mesh_vertices(&s.mesh, PLASTIC);
                    let count = verts.len() as u32;
                    solid.extend(verts);
                    meshlets.push(MeshletDraw {
                        start,
                        count,
                        aabb: s.mesh.aabb().unwrap_or(Aabb3::empty()),
                    });
                } else {
                    for m in &s.meshlets {
                        let start = solid.len() as u32;
                        let verts = mesh_vertices_range(&s.mesh, m.first_tri, m.tri_count, PLASTIC);
                        let count = verts.len() as u32;
                        solid.extend(verts);
                        meshlets.push(MeshletDraw {
                            start,
                            count,
                            aabb: m.aabb,
                        });
                    }
                }
            }
        }
        let overlay_start = solid.len() as u32;
        if show_solids {
            solid.extend(overlay_vertices(&self.mesh, &self.paint_overlay));
        }
        let overlay_count = solid.len() as u32 - overlay_start;
        let gizmos = self
            .gizmo
            .map(|g| gizmo_arrows(g.origin, g.half))
            .unwrap_or_default();
        let (rt_positions, rt_indices, rt_instances) = self.rt_geometry();
        CachedGpuMesh {
            key,
            solid: Arc::from(solid),
            lines: Arc::from(lines),
            gizmos: Arc::from(gizmos),
            labels: Arc::from(crate::label::plate_labels(&self.bed, LABEL)),
            meshlets: Arc::from(meshlets),
            bed_verts,
            overlay_start,
            overlay_count,
            rt_positions: Arc::from(rt_positions),
            rt_indices: Arc::from(rt_indices),
            rt_instances: Arc::from(rt_instances),
        }
    }

    fn rt_geometry(&self) -> (Vec<[f32; 4]>, Vec<u32>, Vec<crate::rt::RtInstance>) {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        let mut instances = Vec::new();
        let (x0, y0, x1, y1) = self.bed.printable_aabb();
        let base = positions.len() as u32;
        positions.extend([
            [x0, y0, 0.0, 1.0],
            [x1, y0, 0.0, 1.0],
            [x1, y1, 0.0, 1.0],
            [x0, y1, 0.0, 1.0],
        ]);
        let first = indices.len() as u32;
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        instances.push(crate::rt::RtInstance {
            first_index: first,
            index_count: 6,
        });
        if self.toolpaths.is_empty() || self.keep_solid {
            let room = crate::rt::MAX_INSTANCES.saturating_sub(instances.len() as u32) as usize;
            for solid in self.solids.iter().take(room) {
                if solid.mesh.indices.is_empty() {
                    continue;
                }
                let base = positions.len() as u32;
                for v in &solid.mesh.vertices {
                    positions.push([v.x, v.y, v.z, 1.0]);
                }
                let first = indices.len() as u32;
                for tri in &solid.mesh.indices {
                    indices.push(base + tri[0]);
                    indices.push(base + tri[1]);
                    indices.push(base + tri[2]);
                }
                instances.push(crate::rt::RtInstance {
                    first_index: first,
                    index_count: (solid.mesh.indices.len() * 3) as u32,
                });
            }
        }
        (positions, indices, instances)
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
            meshlets: mesh.meshlets,
            bed_verts: mesh.bed_verts,
            overlay_start: mesh.overlay_start,
            overlay_count: mesh.overlay_count,
            rt_positions: mesh.rt_positions,
            rt_indices: mesh.rt_indices,
            rt_instances: mesh.rt_instances,
            realistic: self.realistic,
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
    meshlets: Arc<[MeshletDraw]>,
    bed_verts: u32,
    overlay_start: u32,
    overlay_count: u32,
    rt_positions: Arc<[[f32; 4]]>,
    rt_indices: Arc<[u32]>,
    rt_instances: Arc<[crate::rt::RtInstance]>,
    realistic: bool,
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
    color_format: wgpu::TextureFormat,
    msaa_view: Option<wgpu::TextureView>,
    depth_view: Option<wgpu::TextureView>,
    depth_size: (u32, u32),
    _msaa: Option<wgpu::Texture>,
    _depth: Option<wgpu::Texture>,
    _atlas: wgpu::Texture,
    uploaded_key: u64,
    rt: Mutex<Option<crate::rt::RtGpu>>,
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
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
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

        let vertex_buffers = [Some(vertex_layout)];

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bambu-gpu-solid-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &vertex_buffers,
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
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: msaa_state(),
            multiview_mask: None,
            cache: None,
        });

        let gizmo_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bambu-gpu-gizmo-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &vertex_buffers,
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
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: msaa_state(),
            multiview_mask: None,
            cache: None,
        });

        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bambu-gpu-line-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &vertex_buffers,
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
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: msaa_state(),
            multiview_mask: None,
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
            color_format: format,
            msaa_view: None,
            depth_view: None,
            depth_size: (0, 0),
            _msaa: None,
            _depth: None,
            _atlas: atlas_texture,
            uploaded_key: u64::MAX,
            rt: Mutex::new(crate::rt::RtGpu::try_new(device, format, SAMPLE_COUNT)),
        }
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.depth_size == (width, height)
            && self.depth_view.is_some()
            && self.msaa_view.is_some()
        {
            return;
        }
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let msaa = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bambu-gpu-msaa"),
            size,
            mip_level_count: 1,
            sample_count: SAMPLE_COUNT,
            dimension: wgpu::TextureDimension::D2,
            format: self.color_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bambu-gpu-depth"),
            size,
            mip_level_count: 1,
            sample_count: SAMPLE_COUNT,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.msaa_view = Some(msaa.create_view(&wgpu::TextureViewDescriptor::default()));
        self.depth_view = Some(depth.create_view(&wgpu::TextureViewDescriptor::default()));
        self._msaa = Some(msaa);
        self._depth = Some(depth);
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
        pipeline.ensure_targets(device, size.width, size.height);

        // Match the pane we `set_viewport` to in `render`, not the full window.
        let aspect = pane_aspect(*bounds);
        let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, aspect, 1.0, 4000.0);
        let view = self.camera.view_matrix();
        let model = Mat4::IDENTITY;
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

        if self.realistic {
            let mut rt = pipeline.rt.lock().unwrap_or_else(|err| err.into_inner());
            if let Some(rt) = rt.as_mut() {
                let scale = viewport.scale_factor();
                let rw = (bounds.width * scale).round().max(1.0) as u32;
                let rh = (bounds.height * scale).round().max(1.0) as u32;
                rt.resize(device, rw, rh);
                rt.write_camera(queue, view, proj, light, self.camera.eye());
                rt.upload_geom(
                    device,
                    queue,
                    self.geom_key,
                    &self.rt_positions,
                    &self.rt_indices,
                    &self.rt_instances,
                );
                rt.ensure_bind_group(device);
            }
        }
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
        let Some(msaa_view) = pipeline.msaa_view.as_ref() else {
            return;
        };
        let Some(depth_view) = pipeline.depth_view.as_ref() else {
            return;
        };

        let pane_w = clip_bounds.width.max(1);
        let pane_h = clip_bounds.height.max(1);
        let mut rt = pipeline.rt.lock().unwrap_or_else(|err| err.into_inner());
        if self.realistic {
            if let Some(gpu) = rt.as_mut() {
                gpu.build_and_trace(encoder, &self.rt_instances);
            }
        }
        let do_rt = self.realistic && rt.as_ref().is_some_and(|gpu| gpu.is_ready());

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("bambu-gpu-viewport"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: msaa_view,
                resolve_target: Some(target),
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.07,
                        g: 0.075,
                        b: 0.09,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        // iced's `draw()` path already sets this; our depth pass does not.
        // Without it, NDC maps to the full window and the cube looks stretched.
        let vp_w = pane_w as f32;
        let vp_h = pane_h as f32;
        pass.set_viewport(
            clip_bounds.x as f32,
            clip_bounds.y as f32,
            vp_w,
            vp_h,
            0.0,
            1.0,
        );
        pass.set_scissor_rect(clip_bounds.x, clip_bounds.y, pane_w, pane_h);

        if do_rt {
            if let Some(gpu) = rt.as_ref() {
                gpu.blit(&mut pass);
            }
            if self.overlay_count > 0 {
                pass.set_pipeline(&pipeline.pipeline);
                pass.set_bind_group(0, &pipeline.bind_group, &[]);
                pass.set_vertex_buffer(0, pipeline.vertex_buf.slice(..));
                pass.draw(
                    self.overlay_start..self.overlay_start + self.overlay_count,
                    0..1,
                );
            }
        } else if pipeline.solid_count > 0 {
            pass.set_pipeline(&pipeline.pipeline);
            pass.set_bind_group(0, &pipeline.bind_group, &[]);
            pass.set_vertex_buffer(0, pipeline.vertex_buf.slice(..));
            if self.bed_verts > 0 {
                pass.draw(0..self.bed_verts, 0..1);
            }
            let aspect = (vp_w / vp_h.max(1.0)).max(0.1);
            let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, aspect, 1.0, 4000.0);
            let planes = crate::meshlet::frustum_planes(proj * self.camera.view_matrix());
            for meshlet in self.meshlets.iter() {
                if meshlet.count == 0 || !crate::meshlet::aabb_in_frustum(meshlet.aabb, &planes) {
                    continue;
                }
                pass.draw(meshlet.start..meshlet.start + meshlet.count, 0..1);
            }
            if self.overlay_count > 0 {
                pass.draw(
                    self.overlay_start..self.overlay_start + self.overlay_count,
                    0..1,
                );
            }
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
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
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
            buffers: &[Some(vertex_layout)],
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
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: msaa_state(),
        multiview_mask: None,
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
    mesh_vertices_range(mesh, 0, mesh.indices.len() as u32, color)
}

fn mesh_vertices_range(
    mesh: &TriangleMesh,
    first_tri: u32,
    tri_count: u32,
    color: [f32; 3],
) -> Vec<Vertex> {
    let center = mesh_center(mesh);
    let start = first_tri as usize;
    let end = (start + tri_count as usize).min(mesh.indices.len());
    let mut out = Vec::with_capacity(end.saturating_sub(start) * 3);
    for idx in &mesh.indices[start..end] {
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

    #[test]
    fn gizmo_pick_hits_x_shaft() {
        let g = AxisGizmo {
            origin: Vec3::ZERO,
            half: Vec3::splat(10.0),
        };
        let origin = Vec3::new(axis_len(10.0) * 0.5, 40.0, 0.0);
        let dir = Vec3::new(0.0, -1.0, 0.0);
        assert_eq!(g.pick_axis(origin, dir), Some(GizmoAxis::X));
        let miss = Vec3::new(80.0, 40.0, 80.0);
        assert_eq!(g.pick_axis(miss, dir), None);
    }

    #[test]
    fn tessellate_emits_meshlets_for_cube() {
        let scene = ViewportScene::with_cube("test".into());
        let mesh = scene.cached_gpu_mesh();
        assert!(!mesh.meshlets.is_empty());
        let covered: u32 = mesh.meshlets.iter().map(|m| m.count).sum();
        assert!(covered > 0);
        assert!(!mesh.rt_instances.is_empty());
        assert!(mesh.bed_verts > 0);
    }
}
