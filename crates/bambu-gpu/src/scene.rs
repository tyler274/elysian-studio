//! iced `shader::Program`: Z-up bed, mesh, and orbit camera on Vulkan.

pub use crate::camera::{CameraView, OrbitCamera};

use bambu_config::{BedRect, BedShape};
use bambu_geom::{Aabb3, Bvh, TriangleMesh};
use bambu_preview::{ExtrusionRole, ToolpathBuffer};
use glam::{Mat4, Vec3};
use iced::mouse;
use iced::wgpu;
use iced::widget::shader::{self, Viewport};
use iced::{Event, Rectangle};
use rayon::prelude::*;
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
/// `Vertex` is three `vec3`s (36 bytes). One wgpu buffer cannot exceed
/// `max_buffer_size` (256 MiB on many adapters); huge meshes are split into
/// several buffers / draw calls instead of dropping triangles.
const VERTEX_STRIDE: u64 = 36;
const DEFAULT_MAX_BUFFER_BYTES: u64 = 256 * 1024 * 1024;
const CHUNK_VERTS: usize = ((DEFAULT_MAX_BUFFER_BYTES / VERTEX_STRIDE / 3) * 3) as usize;
const RT_TRI_LIMIT: usize = 200_000;

/// Off-thread packs set `realistic = false`, and huge meshes omit solids from
/// the TLAS (`RT_TRI_LIMIT`). Blit RT only when the acceleration structure
/// actually contains the model; otherwise Fast-raster the meshlets.
fn blit_raytraced_solids(realistic: bool, rt_ready: bool, rt_instance_count: usize) -> bool {
    realistic && rt_ready && rt_instance_count > 1
}

fn gizmo_primitive_state() -> wgpu::PrimitiveState {
    wgpu::PrimitiveState {
        topology: wgpu::PrimitiveTopology::TriangleList,
        cull_mode: Some(wgpu::Face::Back),
        front_face: wgpu::FrontFace::Ccw,
        ..Default::default()
    }
}

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

    pub fn needs_gizmo(self) -> bool {
        matches!(
            self,
            Self::Move | Self::Rotate | Self::Scale | Self::LayOnFace
        )
    }
}

/// Selection AABB used as the XYZ origin. Shaft length is screen-constant.
#[derive(Debug, Clone, Copy)]
pub struct AxisGizmo {
    pub origin: Vec3,
    pub half: Vec3,
    pub axis_len: f32,
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
        for (axis, unit) in [
            (GizmoAxis::X, Vec3::X),
            (GizmoAxis::Y, Vec3::Y),
            (GizmoAxis::Z, Vec3::Z),
        ] {
            let len = self.axis_len;
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
    gpu_chunks: Option<Arc<[(Arc<[Vertex]>, Aabb3)]>>,
}

impl SceneSolid {
    pub fn from_mesh(mesh: TriangleMesh) -> Self {
        Self {
            mesh,
            meshlets: Vec::new(),
            gpu_chunks: None,
        }
    }

    pub fn clusterize(&mut self) {
        self.meshlets = crate::meshlet::clusterize(&self.mesh);
    }
}

/// GPU-ready solids built off the UI thread (meshlets + lit vertices + pick BVH).
#[derive(Debug, Clone)]
pub struct ShadedSolids {
    solids: Vec<SceneSolid>,
    mesh: TriangleMesh,
    pick_bvh: Option<Bvh>,
    packed: CachedGpuMesh,
    /// Object-space AABBs (meshlet unions) for UI gizmos — no vertex walks.
    pub local_aabbs: Vec<Aabb3>,
}

impl ShadedSolids {
    pub fn triangle_count(&self) -> usize {
        self.mesh.indices.len()
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
    pub viewport_height: f32,
    pick_bvh: Option<Bvh>,
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
    solid: Arc<[Arc<[Vertex]>]>,
    lines: Arc<[Vertex]>,
    #[allow(dead_code)]
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
        let mut scene = Self {
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
            solids: {
                let mut solid = SceneSolid::from_mesh(TriangleMesh::cube(20.0));
                solid.clusterize();
                vec![solid]
            },
            realistic: true,
            viewport_height: 800.0,
            pick_bvh: None,
            gpu: Mutex::new(None),
        };
        scene.pack_gpu();
        scene
    }

    fn packing_scene(bed: BedShape) -> Self {
        let bed_mm = bed.orbit_mm();
        Self {
            adapter_label: String::new(),
            camera: OrbitCamera::looking_at_center(
                Vec3::new(bed.center().0, bed.center().1, 0.0),
                bed_mm,
            ),
            mesh: TriangleMesh::default(),
            toolpaths: ToolpathBuffer::default(),
            preview_layer: 0,
            preview_vertices: 0,
            hide_infill: false,
            hide_support: false,
            bed_mm,
            bed,
            keep_solid: true,
            paint_overlay: Vec::new(),
            tool: PlaterTool::Orbit,
            gizmo: None,
            solids: Vec::new(),
            realistic: false,
            viewport_height: 800.0,
            pick_bvh: None,
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
        self.invalidate_gpu();
    }

    /// Replace the solid mesh in world space. Does not recenter or move the camera.
    pub fn set_mesh(&mut self, mesh: TriangleMesh) {
        self.solids = vec![SceneSolid::from_mesh(mesh.clone())];
        self.mesh = mesh;
        self.toolpaths = ToolpathBuffer::default();
        self.preview_layer = 0;
        self.preview_vertices = 0;
        self.paint_overlay.clear();
        self.pick_bvh = None;
        self.invalidate_gpu();
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
        self.pick_bvh = None;
        self.invalidate_gpu();
    }

    /// Orbit so the loaded solids fill the view (Prepare after open).
    pub fn frame_contents(&mut self) {
        let aabb = self
            .solids
            .iter()
            .filter_map(|s| s.mesh.aabb())
            .chain(self.mesh.aabb())
            .reduce(|a, b| a.union(b));
        let Some(aabb) = aabb else {
            return;
        };
        if !aabb.min.is_finite() || !aabb.max.is_finite() {
            return;
        }
        let span = aabb.size().max_element().max(self.bed_mm * 0.35);
        let target = (aabb.min + aabb.max) * 0.5;
        self.camera = OrbitCamera::looking_at_center(target, span);
    }

    pub fn apply_meshlets(&mut self, meshlets: Vec<Vec<crate::meshlet::Meshlet>>) {
        for (solid, lets) in self.solids.iter_mut().zip(meshlets) {
            solid.meshlets = lets;
        }
        self.invalidate_gpu();
    }

    /// Cluster, shade, pack GPU buffers, and build a pick tree. Worker thread only.
    pub fn shade_meshes(
        meshes: Vec<TriangleMesh>,
        bed: BedShape,
        keep_solid: bool,
        mut progress: impl FnMut(&str, f32),
    ) -> ShadedSolids {
        let nsol = meshes.len().max(1);
        let mut solids: Vec<SceneSolid> = Vec::with_capacity(meshes.len());
        for (i, mesh) in meshes.into_iter().enumerate() {
            progress(
                "Clustering meshlets…",
                0.60 + 0.10 * (i as f32 / nsol as f32),
            );
            let mut solid = SceneSolid::from_mesh(mesh);
            solid.clusterize();
            solids.push(solid);
        }
        for (i, solid) in solids.iter_mut().enumerate() {
            shade_solid_chunks(solid, |done, total| {
                let local = if total == 0 {
                    1.0
                } else {
                    done as f32 / total as f32
                };
                let base = 0.72 + 0.20 * (i as f32 / nsol as f32);
                let span = 0.20 / nsol as f32;
                progress("Preparing display mesh…", base + span * local);
            });
        }
        let local_aabbs: Vec<Aabb3> = solids.iter().filter_map(solid_aabb).collect();
        let mut mesh = TriangleMesh::default();
        for solid in &solids {
            mesh.append(&solid.mesh);
        }
        progress("Building pick tree…", 0.95);
        let pick_bvh = if mesh.indices.len() > 8 {
            Some(Bvh::build(&mesh))
        } else {
            None
        };
        progress("Packing display buffers…", 0.97);
        let mut tmp = ViewportScene::packing_scene(bed);
        tmp.keep_solid = keep_solid;
        tmp.solids = solids;
        tmp.mesh = mesh;
        tmp.pick_bvh = pick_bvh;
        let packed = tmp.tessellate(tmp.geom_key());
        progress("Ready", 1.0);
        ShadedSolids {
            solids: tmp.solids,
            mesh: tmp.mesh,
            pick_bvh: tmp.pick_bvh,
            packed,
            local_aabbs,
        }
    }

    pub fn apply_shaded(&mut self, shaded: ShadedSolids) {
        self.solids = shaded.solids;
        self.mesh = shaded.mesh;
        self.pick_bvh = shaded.pick_bvh;
        self.toolpaths = ToolpathBuffer::default();
        self.preview_layer = 0;
        self.preview_vertices = 0;
        self.paint_overlay.clear();
        self.install_gpu(shaded.packed);
    }

    /// Precompute GPU vertices. Must not run on the iced UI thread.
    pub fn pack_gpu(&mut self) {
        let packed = self.tessellate(self.geom_key());
        self.install_gpu(packed);
    }

    fn install_gpu(&self, packed: CachedGpuMesh) {
        *self.gpu.lock().unwrap_or_else(|err| err.into_inner()) = Some(packed);
    }

    fn invalidate_gpu(&self) {
        *self.gpu.lock().unwrap_or_else(|err| err.into_inner()) = None;
    }

    pub fn pick_triangle(&mut self, origin: Vec3, dir: Vec3) -> Option<usize> {
        if self.mesh.indices.len() <= 8 {
            return self.mesh.pick_triangle_linear(origin, dir);
        }
        self.pick_bvh
            .as_ref()?
            .pick_triangle(&self.mesh, origin, dir)
    }

    pub fn triangles_near(&mut self, point: Vec3, radius: f32) -> Vec<usize> {
        if self.mesh.indices.len() <= 8 {
            return self.mesh.triangles_near_linear(point, radius);
        }
        self.pick_bvh
            .as_ref()
            .map(|bvh| bvh.triangles_near(&self.mesh, point, radius))
            .unwrap_or_default()
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
        self.invalidate_gpu();
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
            solid
                .gpu_chunks
                .as_ref()
                .map(|c| c.len())
                .unwrap_or(0)
                .hash(&mut hasher);
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
        let (x0, y0, x1, y1) = self.bed.printable_aabb();
        x0.to_bits().hash(&mut hasher);
        y0.to_bits().hash(&mut hasher);
        x1.to_bits().hash(&mut hasher);
        y1.to_bits().hash(&mut hasher);
        hasher.finish()
    }

    fn cached_gpu_mesh(&self) -> CachedGpuMesh {
        let cache = self.gpu.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(cached) = cache.as_ref() {
            return cached.clone();
        }
        drop(cache);
        self.tessellate_chrome()
    }

    /// Bed / grid / labels only. Never shades model triangles or toolpaths.
    fn tessellate_chrome(&self) -> CachedGpuMesh {
        let lines = grid_vertices(&self.bed);
        let mut solid = VertexPack::new(CHUNK_VERTS);
        solid.extend(&bed_solids(&self.bed));
        let bed_verts = solid.len() as u32;
        let (rt_positions, rt_indices, rt_instances) = rt_bed_only(&self.bed);
        CachedGpuMesh {
            key: 0,
            solid: solid.freeze(),
            lines: Arc::from(lines),
            gizmos: Arc::from(Vec::new()),
            labels: Arc::from(crate::label::plate_labels(&self.bed, LABEL)),
            meshlets: Arc::from(Vec::new()),
            bed_verts,
            overlay_start: bed_verts,
            overlay_count: 0,
            rt_positions: Arc::from(rt_positions),
            rt_indices: Arc::from(rt_indices),
            rt_instances: Arc::from(rt_instances),
        }
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
        let mut solid = VertexPack::new(CHUNK_VERTS);
        solid.extend(&bed_solids(&self.bed));
        let bed_verts = solid.len() as u32;
        let mut meshlets = Vec::new();
        let show_solids = self.toolpaths.is_empty() || self.keep_solid;
        if show_solids {
            for s in &self.solids {
                if let Some(chunks) = &s.gpu_chunks {
                    for (verts, aabb) in chunks.iter() {
                        solid.push_arc(verts.clone(), *aabb, &mut meshlets);
                    }
                    continue;
                }
                let mut jobs: Vec<(u32, u32, Aabb3)> = Vec::new();
                if s.meshlets.is_empty() {
                    let aabb = s.mesh.aabb().unwrap_or(Aabb3::empty());
                    let n = s.mesh.indices.len() as u32;
                    let mut tri = 0u32;
                    while tri < n {
                        let batch = ((CHUNK_VERTS / 3) as u32).min(n - tri);
                        jobs.push((tri, batch, aabb));
                        tri += batch;
                    }
                } else {
                    for m in &s.meshlets {
                        jobs.push((m.first_tri, m.tri_count, m.aabb));
                    }
                }
                let batches: Vec<(Vec<Vertex>, Aabb3)> = if jobs.len() > 1 {
                    jobs.par_iter()
                        .map(|(first, count, aabb)| {
                            let verts =
                                mesh_vertices_range(&s.mesh, *first, *count, PLASTIC, usize::MAX);
                            (verts, *aabb)
                        })
                        .collect()
                } else {
                    jobs.iter()
                        .map(|(first, count, aabb)| {
                            let verts =
                                mesh_vertices_range(&s.mesh, *first, *count, PLASTIC, usize::MAX);
                            (verts, *aabb)
                        })
                        .collect()
                };
                for (verts, aabb) in batches {
                    solid.push_span(verts, aabb, &mut meshlets);
                }
            }
        }
        let overlay_start = solid.len() as u32;
        if show_solids {
            solid.extend(&overlay_vertices(&self.mesh, &self.paint_overlay));
        }
        let overlay_count = solid.len() as u32 - overlay_start;
        let gizmos = self
            .gizmo
            .map(|g| gizmo_arrows(g.origin, g.axis_len))
            .unwrap_or_default();
        let (rt_positions, rt_indices, rt_instances) = self.rt_geometry();
        CachedGpuMesh {
            key,
            solid: solid.freeze(),
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
        let solid_tris: usize = self.solids.iter().map(|s| s.mesh.indices.len()).sum();
        if (self.toolpaths.is_empty() || self.keep_solid)
            && self.realistic
            && solid_tris <= RT_TRI_LIMIT
        {
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

fn rt_bed_only(bed: &BedShape) -> (Vec<[f32; 4]>, Vec<u32>, Vec<crate::rt::RtInstance>) {
    let (x0, y0, x1, y1) = bed.printable_aabb();
    let positions = vec![
        [x0, y0, 0.0, 1.0],
        [x1, y0, 0.0, 1.0],
        [x1, y1, 0.0, 1.0],
        [x0, y1, 0.0, 1.0],
    ];
    let indices = vec![0, 1, 2, 0, 2, 3];
    let instances = vec![crate::rt::RtInstance {
        first_index: 0,
        index_count: 6,
    }];
    (positions, indices, instances)
}

fn solid_aabb(solid: &SceneSolid) -> Option<Aabb3> {
    solid
        .meshlets
        .iter()
        .map(|m| m.aabb)
        .reduce(|a, b| a.union(b))
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
    CursorMoved {
        ndc_x: f32,
        ndc_y: f32,
        aspect: f32,
        viewport_h: f32,
    },
    CursorLeft,
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
                None => {
                    if let Some(pos) = cursor.position_over(bounds) {
                        let (ndc_x, ndc_y, aspect) = cursor_ndc(bounds, pos);
                        Some(shader::Action::publish(
                            ViewportEvent::CursorMoved {
                                ndc_x,
                                ndc_y,
                                aspect,
                                viewport_h: bounds.height,
                            }
                            .into(),
                        ))
                    } else {
                        Some(shader::Action::publish(ViewportEvent::CursorLeft.into()))
                    }
                }
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
        let gizmos = self
            .gizmo
            .map(|g| gizmo_arrows(g.origin, g.axis_len))
            .unwrap_or_default();
        ScenePrimitive {
            camera: self.camera,
            geom_key: mesh.key,
            solid: mesh.solid,
            lines: mesh.lines,
            gizmos: Arc::from(gizmos),
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
    solid: Arc<[Arc<[Vertex]>]>,
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

const _: () = assert!(std::mem::size_of::<Vertex>() == VERTEX_STRIDE as usize);

struct GpuVertBuf {
    buffer: wgpu::Buffer,
    capacity: u64,
    count: u32,
}

/// CPU-side packing so each chunk fits in one `max_buffer_size` allocation.
struct VertexPack {
    chunks: Vec<Vec<Vertex>>,
    chunk_limit: usize,
}

impl VertexPack {
    fn new(chunk_limit: usize) -> Self {
        Self {
            chunks: Vec::new(),
            chunk_limit: (chunk_limit / 3 * 3).max(3),
        }
    }

    fn len(&self) -> usize {
        self.chunks.iter().map(Vec::len).sum()
    }

    fn extend(&mut self, verts: &[Vertex]) {
        let mut offset = 0;
        while offset < verts.len() {
            if self
                .chunks
                .last()
                .is_none_or(|c| self.chunk_limit.saturating_sub(c.len()) < 3)
            {
                self.chunks.push(Vec::with_capacity(
                    (verts.len() - offset).min(self.chunk_limit),
                ));
            }
            let room = (self.chunk_limit - self.chunks.last().map_or(0, Vec::len)) / 3 * 3;
            let take = (verts.len() - offset).min(room) / 3 * 3;
            if take == 0 {
                break;
            }
            self.chunks
                .last_mut()
                .expect("chunk just created")
                .extend_from_slice(&verts[offset..offset + take]);
            offset += take;
        }
    }

    fn push_span(&mut self, verts: Vec<Vertex>, aabb: Aabb3, meshlets: &mut Vec<MeshletDraw>) {
        if verts.is_empty() {
            return;
        }
        let start = self.len() as u32;
        self.extend(&verts);
        let count = self.len() as u32 - start;
        if count == 0 {
            return;
        }
        meshlets.push(MeshletDraw { start, count, aabb });
    }

    fn push_arc(&mut self, verts: Arc<[Vertex]>, aabb: Aabb3, meshlets: &mut Vec<MeshletDraw>) {
        if verts.is_empty() {
            return;
        }
        let start = self.len() as u32;
        self.extend(&verts);
        let count = self.len() as u32 - start;
        if count == 0 {
            return;
        }
        meshlets.push(MeshletDraw { start, count, aabb });
    }

    fn freeze(self) -> Arc<[Arc<[Vertex]>]> {
        self.chunks
            .into_iter()
            .filter(|c| !c.is_empty())
            .map(Arc::<[Vertex]>::from)
            .collect()
    }
}

pub struct ScenePipeline {
    pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    gizmo_pipeline: wgpu::RenderPipeline,
    label_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    label_bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
    solid_bufs: Vec<GpuVertBuf>,
    line_bufs: Vec<GpuVertBuf>,
    gizmo_buf: wgpu::Buffer,
    gizmo_capacity: u64,
    label_buf: wgpu::Buffer,
    label_capacity: u64,
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
            primitive: gizmo_primitive_state(),
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

        let solid_bufs = vec![GpuVertBuf {
            buffer: empty_vertex_buffer(device, 4096, "bambu-gpu-solid-verts"),
            capacity: 4096,
            count: 0,
        }];
        let line_bufs = vec![GpuVertBuf {
            buffer: empty_vertex_buffer(device, 4096, "bambu-gpu-line-verts"),
            capacity: 4096,
            count: 0,
        }];
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
            solid_bufs,
            line_bufs,
            gizmo_buf,
            gizmo_capacity: 1024,
            label_buf,
            label_capacity: 256,
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
        let proj = self.camera.perspective(aspect);
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
            upload_vert_chunks(
                device,
                queue,
                &mut pipeline.solid_bufs,
                self.solid.iter().map(|c| c.as_ref()),
                3,
                "bambu-gpu-solid-verts",
            );
            upload_vert_chunks(
                device,
                queue,
                &mut pipeline.line_bufs,
                std::iter::once(self.lines.as_ref()),
                2,
                "bambu-gpu-line-verts",
            );
            pipeline.label_count = upload_labels(
                device,
                queue,
                &mut pipeline.label_buf,
                &mut pipeline.label_capacity,
                &self.labels,
            );
            pipeline.uploaded_key = self.geom_key;
        }
        pipeline.gizmo_count = upload_vertices(
            device,
            queue,
            &mut pipeline.gizmo_buf,
            &mut pipeline.gizmo_capacity,
            &self.gizmos,
            "bambu-gpu-gizmo-verts",
        );

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
        let do_rt = blit_raytraced_solids(
            self.realistic,
            rt.as_ref().is_some_and(|gpu| gpu.is_ready()),
            self.rt_instances.len(),
        );

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
                draw_vert_span(
                    &mut pass,
                    &pipeline.solid_bufs,
                    self.overlay_start,
                    self.overlay_count,
                );
            }
        } else if pipeline.solid_bufs.iter().any(|b| b.count > 0) {
            pass.set_pipeline(&pipeline.pipeline);
            pass.set_bind_group(0, &pipeline.bind_group, &[]);
            draw_vert_span(&mut pass, &pipeline.solid_bufs, 0, self.bed_verts);
            let aspect = (vp_w / vp_h.max(1.0)).max(0.1);
            let proj = self.camera.perspective(aspect);
            let planes = crate::meshlet::frustum_planes(proj * self.camera.view_matrix());
            for meshlet in self.meshlets.iter() {
                if meshlet.count == 0 || !crate::meshlet::aabb_in_frustum(meshlet.aabb, &planes) {
                    continue;
                }
                draw_vert_span(
                    &mut pass,
                    &pipeline.solid_bufs,
                    meshlet.start,
                    meshlet.count,
                );
            }
            draw_vert_span(
                &mut pass,
                &pipeline.solid_bufs,
                self.overlay_start,
                self.overlay_count,
            );
        }
        if pipeline.line_bufs.iter().any(|b| b.count > 0) {
            pass.set_pipeline(&pipeline.line_pipeline);
            pass.set_bind_group(0, &pipeline.bind_group, &[]);
            for buf in &pipeline.line_bufs {
                if buf.count == 0 {
                    continue;
                }
                pass.set_vertex_buffer(0, buf.buffer.slice(..));
                pass.draw(0..buf.count, 0..1);
            }
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

fn aligned_chunk_verts(max_buffer_size: u64, stride: u64, align: u64) -> usize {
    let max_count = max_buffer_size / stride.max(1);
    let n = (max_count / align.max(1)) * align.max(1);
    n.max(align.max(1)) as usize
}

/// Map a concatenated vertex span onto per-buffer ranges (may cross chunks).
fn for_chunk_span(
    start: u32,
    count: u32,
    chunk_counts: &[u32],
    mut emit: impl FnMut(usize, u32, u32),
) {
    if count == 0 {
        return;
    }
    let end = start.saturating_add(count);
    let mut acc = 0u32;
    for (i, &n) in chunk_counts.iter().enumerate() {
        if n == 0 {
            continue;
        }
        let chunk_end = acc.saturating_add(n);
        if end <= acc {
            break;
        }
        if start < chunk_end {
            let local_start = start.saturating_sub(acc).min(n);
            let local_end = end.saturating_sub(acc).min(n);
            if local_end > local_start {
                emit(i, local_start, local_end - local_start);
            }
        }
        acc = chunk_end;
    }
}

fn draw_vert_span(pass: &mut wgpu::RenderPass<'_>, bufs: &[GpuVertBuf], start: u32, count: u32) {
    let counts: Vec<u32> = bufs.iter().map(|b| b.count).collect();
    for_chunk_span(start, count, &counts, |i, local_start, n| {
        if let Some(buf) = bufs.get(i) {
            pass.set_vertex_buffer(0, buf.buffer.slice(..));
            pass.draw(local_start..local_start + n, 0..1);
        }
    });
}

fn upload_vert_chunks<'a>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bufs: &mut Vec<GpuVertBuf>,
    chunks: impl IntoIterator<Item = &'a [Vertex]>,
    align: usize,
    label: &str,
) {
    let max = aligned_chunk_verts(device.limits().max_buffer_size, VERTEX_STRIDE, align as u64)
        .max(align);
    let pieces: Vec<&[Vertex]> = chunks
        .into_iter()
        .flat_map(|c| c.chunks(max))
        .filter(|p| !p.is_empty())
        .collect();
    if pieces.is_empty() {
        for buf in bufs.iter_mut() {
            buf.count = 0;
        }
        return;
    }
    if bufs.len() > pieces.len() {
        bufs.truncate(pieces.len());
    }
    while bufs.len() < pieces.len() {
        bufs.push(GpuVertBuf {
            buffer: empty_vertex_buffer(device, 64, label),
            capacity: 64,
            count: 0,
        });
    }
    for (gpu, piece) in bufs.iter_mut().zip(pieces) {
        gpu.count = upload_vertices(
            device,
            queue,
            &mut gpu.buffer,
            &mut gpu.capacity,
            piece,
            label,
        );
    }
}

#[cfg(test)]
fn vertex_draw_range(start: u32, count: u32, limit: u32) -> Option<std::ops::Range<u32>> {
    if count == 0 || start >= limit {
        return None;
    }
    let end = start.saturating_add(count).min(limit);
    (end > start).then_some(start..end)
}

/// Grow to a power of two, but never past `max_buffer_size / stride`.
fn grow_element_count(needed: u64, stride: u64, max_buffer_size: u64) -> u64 {
    let max_count = (max_buffer_size / stride.max(1)).max(1);
    let upload = needed.min(max_count);
    if upload == 0 {
        return 64.min(max_count).max(1);
    }
    upload
        .next_power_of_two()
        .max(64)
        .min(max_count)
        .max(upload)
}

fn empty_vertex_buffer(device: &wgpu::Device, count: u64, label: &str) -> wgpu::Buffer {
    let max_count = (device.limits().max_buffer_size / VERTEX_STRIDE).max(1);
    let count = count.min(max_count).max(1);
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: count * VERTEX_STRIDE,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn empty_label_buffer(device: &wgpu::Device, count: u64) -> wgpu::Buffer {
    let stride = std::mem::size_of::<crate::label::LabelVertex>() as u64;
    let max_count = (device.limits().max_buffer_size / stride).max(1);
    let count = count.min(max_count).max(1);
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bambu-gpu-label-verts"),
        size: count * stride,
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
) -> u32 {
    let needed = verts.len() as u64;
    if needed == 0 {
        return 0;
    }
    let max_size = device.limits().max_buffer_size;
    let max_count = (max_size / VERTEX_STRIDE).max(1);
    let upload = needed.min(max_count);
    if upload > *capacity {
        *capacity = grow_element_count(upload, VERTEX_STRIDE, max_size);
        *buffer = empty_vertex_buffer(device, *capacity, label);
    }
    if upload < needed {
        tracing::warn!(
            label,
            needed,
            upload,
            "mesh exceeds a single GPU buffer; this piece should have been split"
        );
    }
    queue.write_buffer(buffer, 0, bytemuck::cast_slice(&verts[..upload as usize]));
    upload as u32
}

fn upload_labels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut wgpu::Buffer,
    capacity: &mut u64,
    verts: &[crate::label::LabelVertex],
) -> u32 {
    let needed = verts.len() as u64;
    if needed == 0 {
        return 0;
    }
    let stride = std::mem::size_of::<crate::label::LabelVertex>() as u64;
    let max_size = device.limits().max_buffer_size;
    let max_count = (max_size / stride).max(1);
    let upload = needed.min(max_count);
    if upload > *capacity {
        *capacity = grow_element_count(upload, stride, max_size);
        *buffer = empty_label_buffer(device, *capacity);
    }
    queue.write_buffer(buffer, 0, bytemuck::cast_slice(&verts[..upload as usize]));
    upload as u32
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

fn shade_solid_chunks(solid: &mut SceneSolid, mut progress: impl FnMut(usize, usize)) {
    if solid.meshlets.is_empty() {
        solid.clusterize();
    }
    let total = solid.meshlets.len();
    if total == 0 {
        solid.gpu_chunks = Some(Arc::from([]));
        progress(0, 0);
        return;
    }
    const BATCH: usize = 256;
    let mut out = Vec::with_capacity(total);
    for chunk in solid.meshlets.chunks(BATCH) {
        let part: Vec<(Arc<[Vertex]>, Aabb3)> = chunk
            .par_iter()
            .map(|m| {
                let verts =
                    mesh_vertices_range(&solid.mesh, m.first_tri, m.tri_count, PLASTIC, usize::MAX);
                (Arc::<[Vertex]>::from(verts), m.aabb)
            })
            .collect();
        out.extend(part);
        progress(out.len(), total);
    }
    solid.gpu_chunks = Some(Arc::from(out));
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

#[cfg(test)]
fn mesh_vertices(mesh: &TriangleMesh, color: [f32; 3], max_verts: usize) -> Vec<Vertex> {
    mesh_vertices_range(mesh, 0, mesh.indices.len() as u32, color, max_verts)
}

fn mesh_vertices_range(
    mesh: &TriangleMesh,
    first_tri: u32,
    tri_count: u32,
    color: [f32; 3],
    max_verts: usize,
) -> Vec<Vertex> {
    let center = mesh_center(mesh);
    let start = first_tri as usize;
    let max_tris = max_verts / 3;
    let end = (start + tri_count as usize)
        .min(mesh.indices.len())
        .min(start.saturating_add(max_tris));
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

fn gizmo_arrows(origin: Vec3, len: f32) -> Vec<Vertex> {
    let mut out = Vec::new();
    axis_arrow(&mut out, origin, Vec3::X, len, AXIS_X);
    axis_arrow(&mut out, origin, Vec3::Y, len, AXIS_Y);
    axis_arrow(&mut out, origin, Vec3::Z, len, AXIS_Z);
    out
}

/// World length for ~18 px grabbers. Clamped so a large AABB cannot dwarf the mesh.
pub fn screen_axis_len(distance: f32, viewport_height_px: f32) -> f32 {
    const GRABBER_PX: f32 = 18.0;
    const FOV: f32 = std::f32::consts::FRAC_PI_4;
    let world_h = 2.0 * distance.max(1.0) * (FOV * 0.5).tan();
    let px = viewport_height_px.max(1.0);
    (GRABBER_PX * world_h / px).clamp(12.0, 48.0)
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
    fn grow_element_count_stays_under_256mib_max_buffer() {
        let max = 268_435_456;
        let stride = VERTEX_STRIDE;
        let max_verts = max / stride;
        // This is the panic: 8M+ verts rounded to 2^24 * 36 = 576 MiB.
        let grown = grow_element_count(16_777_216, stride, max);
        assert!(
            grown <= max_verts,
            "grown {grown} verts exceeds {max_verts}"
        );
        assert!(grown * stride <= max);
        assert_eq!(grow_element_count(100, stride, max), 128);
        assert_eq!(grow_element_count(4096, stride, max), 4096);
        assert!(CHUNK_VERTS as u64 * stride <= max);
        assert_eq!(CHUNK_VERTS % 3, 0);
    }

    #[test]
    fn vertex_pack_preserves_every_vertex_across_chunks() {
        let verts: Vec<Vertex> = (0..100)
            .map(|i| Vertex {
                position: [i as f32, 0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                color: PLASTIC,
            })
            .collect();
        let mut pack = VertexPack::new(30);
        pack.extend(&verts);
        assert_eq!(
            pack.len(),
            99,
            "keep complete triangles only (100 % 3 = 1 dropped)"
        );
        assert!(pack
            .chunks
            .iter()
            .all(|c| c.len() <= 30 && c.len() % 3 == 0));
        let mut packed = Vec::new();
        for c in &pack.chunks {
            packed.extend_from_slice(c);
        }
        assert_eq!(
            packed.iter().map(|v| v.position).collect::<Vec<_>>(),
            verts[..99].iter().map(|v| v.position).collect::<Vec<_>>()
        );
    }

    #[test]
    fn chunk_span_covers_ranges_that_cross_buffers() {
        let mut got = Vec::new();
        for_chunk_span(8, 10, &[10, 10, 5], |i, start, n| {
            got.push((i, start, n));
        });
        assert_eq!(got, vec![(0, 8, 2), (1, 0, 8)]);
        got.clear();
        for_chunk_span(0, 25, &[10, 10, 5], |i, start, n| {
            got.push((i, start, n));
        });
        assert_eq!(got, vec![(0, 0, 10), (1, 0, 10), (2, 0, 5)]);
        got.clear();
        for_chunk_span(20, 0, &[10, 10], |i, start, n| got.push((i, start, n)));
        assert!(got.is_empty());
    }

    #[test]
    fn tessellate_covers_all_solid_triangles() {
        let meshes: Vec<_> = (0..8).map(|_| TriangleMesh::cube(10.0)).collect();
        let expected: u32 = meshes.iter().map(|m| m.indices.len() as u32 * 3).sum();
        let mut scene = ViewportScene::with_cube("chunks".into());
        scene.set_solids(meshes);
        scene.pack_gpu();
        let mesh = scene.cached_gpu_mesh();
        let covered: u32 = mesh.meshlets.iter().map(|m| m.count).sum();
        assert_eq!(covered, expected);
        let solid_verts: usize = mesh.solid.iter().map(|c| c.len()).sum();
        assert!(solid_verts >= expected as usize + mesh.bed_verts as usize);
    }

    #[test]
    fn vertex_draw_range_clamps_to_uploaded_count() {
        assert_eq!(vertex_draw_range(0, 10, 10), Some(0..10));
        assert_eq!(vertex_draw_range(8, 8, 10), Some(8..10));
        assert_eq!(vertex_draw_range(10, 4, 10), None);
        assert_eq!(vertex_draw_range(0, 0, 10), None);
    }

    #[test]
    fn mesh_vertices_range_respects_max_verts() {
        let mesh = TriangleMesh::cube(20.0);
        let verts = mesh_vertices_range(&mesh, 0, mesh.indices.len() as u32, PLASTIC, 9);
        assert_eq!(verts.len(), 9);
        assert_eq!(verts.len() % 3, 0);
        let full = mesh_vertices(&mesh, PLASTIC, usize::MAX);
        assert_eq!(full.len(), mesh.indices.len() * 3);
    }

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
        let verts = gizmo_arrows(Vec3::ZERO, 24.0);
        assert!(
            verts.len() > 36,
            "shaft + cone should be tessellated, got {}",
            verts.len()
        );
        assert_eq!(verts.len() % 3, 0);
    }

    #[test]
    fn gizmo_arrows_are_screen_sized() {
        let near = gizmo_arrows(Vec3::ZERO, screen_axis_len(200.0, 800.0));
        let far = gizmo_arrows(Vec3::ZERO, screen_axis_len(800.0, 800.0));
        let tip = |verts: &[Vertex]| verts.iter().map(|v| v.position[0]).fold(f32::MIN, f32::max);
        assert!(
            tip(&far) > tip(&near),
            "zoomed-out camera should lengthen world shafts, near {} far {}",
            tip(&near),
            tip(&far)
        );
        let huge = screen_axis_len(2500.0, 800.0);
        assert!(huge <= 48.0 + 1e-3, "shafts must not dwarf a large mesh");
        assert!((screen_axis_len(200.0, 800.0) - screen_axis_len(200.0, 800.0)).abs() < 1e-5);
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
            axis_len: 24.0,
        };
        let origin = Vec3::new(g.axis_len * 0.5, 40.0, 0.0);
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

    #[test]
    fn shade_meshes_precomputes_display_chunks() {
        let shaded = ViewportScene::shade_meshes(
            vec![TriangleMesh::cube(20.0)],
            BedShape::square(BED_MM),
            true,
            |_, _| {},
        );
        let mut scene = ViewportScene::with_cube("shade".into());
        scene.apply_shaded(shaded);
        let mesh = scene.cached_gpu_mesh();
        let covered: u32 = mesh.meshlets.iter().map(|m| m.count).sum();
        assert_eq!(covered, 12 * 3);
    }

    #[test]
    fn draw_without_pack_stays_chrome_only() {
        let mut mesh = TriangleMesh::cube(20.0);
        let base = mesh.indices.clone();
        while mesh.indices.len() <= 48_000 {
            mesh.indices.extend_from_slice(&base);
        }
        let mut scene = ViewportScene::with_cube("skip".into());
        scene.set_solids(vec![mesh]);
        let gpu = scene.cached_gpu_mesh();
        let covered: u32 = gpu.meshlets.iter().map(|m| m.count).sum();
        assert_eq!(covered, 0, "UI draw must not tessellate unprepared solids");
    }

    #[test]
    fn gizmo_pipeline_culls_back_faces() {
        assert_eq!(
            gizmo_primitive_state().cull_mode,
            Some(wgpu::Face::Back),
            "back-face cull hides cone interiors"
        );
        assert_eq!(gizmo_primitive_state().front_face, wgpu::FrontFace::Ccw);
    }

    #[test]
    fn gizmo_triangles_are_ccw_from_outside() {
        let verts = gizmo_arrows(Vec3::ZERO, 24.0);
        assert!(verts.len() >= 9);
        for tri in verts.chunks_exact(3) {
            let a = Vec3::from(tri[0].position);
            let b = Vec3::from(tri[1].position);
            let c = Vec3::from(tri[2].position);
            let geometric = (b - a).cross(c - a);
            let stored = Vec3::from(tri[0].normal);
            assert!(
                geometric.dot(stored) > 0.0,
                "winding must match the outward normal so Back cull keeps the shell"
            );
        }
    }

    #[test]
    fn worker_pack_without_rt_still_rasters_meshlets() {
        let shaded = ViewportScene::shade_meshes(
            vec![TriangleMesh::cube(20.0)],
            BedShape::square(BED_MM),
            true,
            |_, _| {},
        );
        let mut scene = ViewportScene::with_cube("worker-pack".into());
        scene.realistic = true;
        scene.apply_shaded(shaded);
        let mesh = scene.cached_gpu_mesh();
        let covered: u32 = mesh.meshlets.iter().map(|m| m.count).sum();
        assert!(
            covered >= 12 * 3,
            "Fast raster meshlets must survive the pack"
        );
        assert!(
            !blit_raytraced_solids(true, true, mesh.rt_instances.len()),
            "off-thread packs omit the model from the TLAS; blit would show an empty bed"
        );
    }

    #[test]
    fn over_rt_limit_omits_tlas_solids() {
        assert!(RT_TRI_LIMIT < usize::MAX / 2);
        assert!(!blit_raytraced_solids(true, true, 1));
        assert!(blit_raytraced_solids(true, true, 2));
        assert!(!blit_raytraced_solids(true, false, 2));
        assert!(!blit_raytraced_solids(false, true, 2));
    }

    #[test]
    fn frame_contents_looks_at_solid_center() {
        let mut mesh = TriangleMesh::cube(20.0);
        mesh.translate(Vec3::new(80.0, 40.0, 0.0));
        let mut scene = ViewportScene::with_cube("frame".into());
        scene.set_mesh(mesh);
        scene.frame_contents();
        assert!(
            (scene.camera.target.x - 90.0).abs() < 1.0,
            "{}",
            scene.camera.target
        );
        assert!((scene.camera.target.y - 50.0).abs() < 1.0);
        assert!(scene.camera.distance > 20.0);
    }

    #[test]
    fn framed_meshlets_stay_in_frustum() {
        let shaded = ViewportScene::shade_meshes(
            vec![TriangleMesh::cube(20.0)],
            BedShape::square(BED_MM),
            true,
            |_, _| {},
        );
        let mut scene = ViewportScene::with_cube("frustum".into());
        scene.apply_shaded(shaded);
        scene.frame_contents();
        let mesh = scene.cached_gpu_mesh();
        let proj = scene.camera.perspective(1.0);
        let planes = crate::meshlet::frustum_planes(proj * scene.camera.view_matrix());
        assert!(!mesh.meshlets.is_empty());
        for m in mesh.meshlets.iter() {
            assert!(
                crate::meshlet::aabb_in_frustum(m.aabb, &planes),
                "packed meshlet AABB {:?} culled after frame",
                m.aabb
            );
        }
    }
}
