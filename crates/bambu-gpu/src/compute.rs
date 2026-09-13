//! Headless Vulkan compute: triangle–plane, occupancy, and infill fields.
//!
//! Mesh vertices/indices are uploaded once and reused across Z batches. Clipper
//! union, walls, and infill stay on the CPU (integer, deterministic).

use bambu_geom::{union_polygons, Point, Polygon, Polyline, TriangleMesh};
use bambu_slicer::{clip_polylines, loops_from_segments, point_from_xy_mm};
use wgpu::util::DeviceExt;

use crate::GpuError;

const MAX_SEGS: u64 = 2_000_000;
const MAX_GRID: u32 = 256;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuVertex {
    p: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuTri {
    i0: u32,
    i1: u32,
    i2: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuParams {
    tri_count: u32,
    layer_count: u32,
    _pad0: u32,
    _pad1: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuLayerSeg {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    layer: u32,
    _p0: u32,
    _p1: u32,
    _p2: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuOccParams {
    tri_count: u32,
    grid_w: u32,
    grid_h: u32,
    cell: f32,
    origin_x: f32,
    origin_y: f32,
    z: f32,
    z_below: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuGyroParams {
    origin_x: f32,
    origin_y: f32,
    cell: f32,
    z: f32,
    period: f32,
    grid_w: u32,
    grid_h: u32,
    _pad: u32,
}

struct MeshXyAabb {
    min: (f32, f32),
    max: (f32, f32),
}

struct UploadedMesh {
    vertex_buf: wgpu::Buffer,
    tri_buf: wgpu::Buffer,
    tri_count: u32,
    aabb: Option<MeshXyAabb>,
}

/// Persistent Vulkan compute device used by CLI and UI slice runs.
pub struct VulkanSliceAccel {
    device: wgpu::Device,
    queue: wgpu::Queue,
    slice_pipeline: wgpu::ComputePipeline,
    slice_bgl: wgpu::BindGroupLayout,
    occ_pipeline: wgpu::ComputePipeline,
    occ_bgl: wgpu::BindGroupLayout,
    gyro_pipeline: wgpu::ComputePipeline,
    gyro_bgl: wgpu::BindGroupLayout,
}

impl VulkanSliceAccel {
    pub fn new() -> Result<Self, GpuError> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .map_err(|e| GpuError::NoAdapter(e.to_string()))?;

        let desc = wgpu::DeviceDescriptor {
            label: Some("bambu-compute"),
            ..Default::default()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&desc))
            .map_err(|e| GpuError::Request(e.to_string()))?;

        let (slice_pipeline, slice_bgl) = make_slice_pipeline(&device);
        let (occ_pipeline, occ_bgl) = make_occ_pipeline(&device);
        let (gyro_pipeline, gyro_bgl) = make_gyro_pipeline(&device);

        Ok(Self {
            device,
            queue,
            slice_pipeline,
            slice_bgl,
            occ_pipeline,
            occ_bgl,
            gyro_pipeline,
            gyro_bgl,
        })
    }

    /// GPU plane hits → CPU loop stitch → Clipper union, per Z.
    pub fn contours_at(&self, mesh: &TriangleMesh, z: f32) -> Result<Vec<Polygon>, GpuError> {
        self.contours_for_layers(mesh, &[z as f64])
            .map(|mut layers| layers.pop().map(|(_, p)| p).unwrap_or_default())
    }

    pub fn contours_for_layers(
        &self,
        mesh: &TriangleMesh,
        zs: &[f64],
    ) -> Result<Vec<(f64, Vec<Polygon>)>, GpuError> {
        let upload = self.upload_mesh(mesh)?;
        if upload.tri_count == 0 || zs.is_empty() {
            return Ok(zs.iter().map(|&z| (z, Vec::new())).collect());
        }
        let mut buckets: Vec<Vec<(Point, Point)>> = vec![Vec::new(); zs.len()];
        let mut offset = 0usize;
        while offset < zs.len() {
            let tri = upload.tri_count.max(1) as u64;
            let batch = ((MAX_SEGS / tri).max(1) as usize).min(zs.len() - offset);
            self.dispatch_batch(&upload, &zs[offset..offset + batch], offset, &mut buckets)?;
            offset += batch;
        }
        Ok(zs
            .iter()
            .zip(buckets)
            .map(|(&z, segs)| {
                let segs: Vec<_> = segs.into_iter().filter(|(a, b)| a != b).collect();
                (z, union_polygons(&loops_from_segments(&segs)))
            })
            .collect())
    }

    /// Cells occupied at `z` but not `z_below` — extra tree-contact candidates.
    pub fn occupancy_contacts(
        &self,
        mesh: &TriangleMesh,
        zs: &[f64],
    ) -> Result<Vec<(f64, Vec<Point>)>, GpuError> {
        let upload = self.upload_mesh(mesh)?;
        if upload.tri_count == 0 || zs.is_empty() {
            return Ok(zs.iter().map(|&z| (z, Vec::new())).collect());
        }
        let Some(aabb) = &upload.aabb else {
            return Ok(zs.iter().map(|&z| (z, Vec::new())).collect());
        };
        let (min_x, min_y) = aabb.min;
        let (max_x, max_y) = aabb.max;
        let span_x = (max_x - min_x).max(1.0);
        let span_y = (max_y - min_y).max(1.0);
        let cell = (span_x.max(span_y) / 64.0).clamp(0.4, 2.0);
        let grid_w = ((span_x / cell).ceil() as u32).clamp(1, MAX_GRID);
        let grid_h = ((span_y / cell).ceil() as u32).clamp(1, MAX_GRID);
        let origin_x = min_x;
        let origin_y = min_y;
        let cell_count = (grid_w * grid_h).max(1) as u64;
        let cell_bytes = (cell_count * 4).max(16);

        let param_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("occ-params"),
            size: std::mem::size_of::<GpuOccParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cell_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("occ-cells"),
            size: cell_bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cell_read = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("occ-cells-read"),
            size: cell_bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut out = Vec::with_capacity(zs.len());
        for (i, &z) in zs.iter().enumerate() {
            let z_below = if i == 0 { z - 0.2 } else { zs[i - 1] };
            self.queue
                .write_buffer(&cell_buf, 0, &vec![0u8; cell_bytes as usize]);
            let params = GpuOccParams {
                tri_count: upload.tri_count,
                grid_w,
                grid_h,
                cell,
                origin_x,
                origin_y,
                z: z as f32,
                z_below: z_below as f32,
            };
            self.queue
                .write_buffer(&param_buf, 0, bytemuck::bytes_of(&params));
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("occ-bg"),
                layout: &self.occ_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: param_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: upload.vertex_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: upload.tri_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: cell_buf.as_entire_binding(),
                    },
                ],
            });
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("occ-enc"),
                });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("occ-pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.occ_pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(upload.tri_count.div_ceil(64).max(1), 1, 1);
            }
            encoder.copy_buffer_to_buffer(&cell_buf, 0, &cell_read, 0, cell_bytes);
            self.queue.submit(Some(encoder.finish()));
            let bits = map_u32s(&self.device, &cell_read, cell_count as usize)?;
            let mut pts = Vec::new();
            for gy in 0..grid_h {
                for gx in 0..grid_w {
                    let v = bits[(gy * grid_w + gx) as usize];
                    if v & 1 == 1 && v & 2 == 0 {
                        pts.push(Point::from_mm(
                            f64::from(origin_x + (gx as f32 + 0.5) * cell),
                            f64::from(origin_y + (gy as f32 + 0.5) * cell),
                        ));
                    }
                }
            }
            out.push((z, pts));
        }
        Ok(out)
    }

    /// Implicit gyroid field, then CPU marching-squares polylines clipped to `region`.
    pub fn gyroid_polylines(
        &self,
        region: &[Polygon],
        spacing_mm: f64,
        density: f64,
        z_mm: f64,
    ) -> Result<Vec<Polyline>, GpuError> {
        let period = spacing_mm / (density * 2.44).max(0.05);
        self.field_polylines(region, period, z_mm, 0)
    }

    /// Adaptive-cubic-style sine field, clipped on the CPU.
    pub fn adaptive_polylines(
        &self,
        region: &[Polygon],
        spacing_mm: f64,
        z_mm: f64,
    ) -> Result<Vec<Polyline>, GpuError> {
        self.field_polylines(region, spacing_mm.max(0.2), z_mm, 1)
    }

    fn field_polylines(
        &self,
        region: &[Polygon],
        period: f64,
        z_mm: f64,
        mode: u32,
    ) -> Result<Vec<Polyline>, GpuError> {
        let Some((min, max)) = poly_bbox(region) else {
            return Ok(Vec::new());
        };
        let min_x = bambu_geom::unscale(min.x) as f32;
        let min_y = bambu_geom::unscale(min.y) as f32;
        let max_x = bambu_geom::unscale(max.x) as f32;
        let max_y = bambu_geom::unscale(max.y) as f32;
        let cell = (period as f32 * 0.25).clamp(0.1, 2.0);
        let grid_w = (((max_x - min_x) / cell).ceil() as u32 + 2).clamp(2, MAX_GRID);
        let grid_h = (((max_y - min_y) / cell).ceil() as u32 + 2).clamp(2, MAX_GRID);
        let n = (grid_w * grid_h).max(1);
        let bytes = (u64::from(n) * 4).max(16);
        let params = GpuGyroParams {
            origin_x: min_x,
            origin_y: min_y,
            cell,
            z: z_mm as f32,
            period: period as f32,
            grid_w,
            grid_h,
            _pad: mode,
        };
        let param_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("gyro-params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let field_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gyro-field"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let field_read = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gyro-field-read"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gyro-bg"),
            layout: &self.gyro_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: param_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: field_buf.as_entire_binding(),
                },
            ],
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gyro-enc"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("gyro-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.gyro_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(grid_w.div_ceil(8).max(1), grid_h.div_ceil(8).max(1), 1);
        }
        encoder.copy_buffer_to_buffer(&field_buf, 0, &field_read, 0, bytes);
        self.queue.submit(Some(encoder.finish()));
        let field = map_f32s(&self.device, &field_read, n as usize)?;
        let raw = marching_squares(&field, grid_w, grid_h, min_x, min_y, cell);
        Ok(clip_polylines(&raw, region))
    }

    fn upload_mesh(&self, mesh: &TriangleMesh) -> Result<UploadedMesh, GpuError> {
        let verts: Vec<GpuVertex> = mesh
            .vertices
            .iter()
            .map(|v| GpuVertex {
                p: [v.x, v.y, v.z, 0.0],
            })
            .collect();
        let tris: Vec<GpuTri> = mesh
            .indices
            .iter()
            .map(|i| GpuTri {
                i0: i[0],
                i1: i[1],
                i2: i[2],
                _pad: 0,
            })
            .collect();
        if verts.is_empty() {
            return Err(GpuError::Request("mesh has no vertices".into()));
        }
        let vertex_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("slice-verts"),
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let tri_bytes = if tris.is_empty() {
            &[0u8; 16][..]
        } else {
            bytemuck::cast_slice(&tris)
        };
        let tri_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("slice-tris"),
                contents: tri_bytes,
                usage: wgpu::BufferUsages::STORAGE,
            });
        let aabb = mesh.aabb().map(|a| MeshXyAabb {
            min: (a.min.x, a.min.y),
            max: (a.max.x, a.max.y),
        });
        Ok(UploadedMesh {
            vertex_buf,
            tri_buf,
            tri_count: tris.len() as u32,
            aabb,
        })
    }

    fn dispatch_batch(
        &self,
        mesh: &UploadedMesh,
        zs: &[f64],
        layer_offset: usize,
        buckets: &mut [Vec<(Point, Point)>],
    ) -> Result<(), GpuError> {
        let layer_count = zs.len() as u32;
        let cap = (u64::from(mesh.tri_count.max(1)) * u64::from(layer_count.max(1))).min(MAX_SEGS);
        let seg_size = (cap * std::mem::size_of::<GpuLayerSeg>() as u64).max(16);
        let mut z_f32: Vec<f32> = zs.iter().map(|&z| z as f32).collect();
        while z_f32.len() < 4 {
            z_f32.push(0.0);
        }
        let param_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("slice-params"),
            size: std::mem::size_of::<GpuParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let zs_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("slice-zs"),
                contents: bytemuck::cast_slice(&z_f32),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let count_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("slice-count"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let seg_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("slice-segs"),
            size: seg_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let count_read = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("slice-count-read"),
            size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let seg_read = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("slice-seg-read"),
            size: seg_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let params = GpuParams {
            tri_count: mesh.tri_count,
            layer_count,
            _pad0: 0,
            _pad1: 0,
        };
        self.queue
            .write_buffer(&param_buf, 0, bytemuck::bytes_of(&params));
        self.queue
            .write_buffer(&count_buf, 0, bytemuck::bytes_of(&[0u32; 4]));

        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("slice-bg"),
            layout: &self.slice_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: param_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: mesh.vertex_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: mesh.tri_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: zs_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: count_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: seg_buf.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("slice-enc"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("slice-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.slice_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            let groups = mesh.tri_count.div_ceil(64).max(1);
            pass.dispatch_workgroups(groups, layer_count.max(1), 1);
        }
        encoder.copy_buffer_to_buffer(&count_buf, 0, &count_read, 0, 16);
        encoder.copy_buffer_to_buffer(&seg_buf, 0, &seg_read, 0, seg_size);
        self.queue.submit(Some(encoder.finish()));

        let n = map_u32(&self.device, &count_read)?;
        let n = (n as usize).min(cap as usize);
        let segs = map_layer_segs(&self.device, &seg_read, n)?;
        for s in segs {
            let layer = layer_offset + s.layer as usize;
            if let Some(bucket) = buckets.get_mut(layer) {
                bucket.push((
                    point_from_xy_mm(s.x0 as f64, s.y0 as f64),
                    point_from_xy_mm(s.x1 as f64, s.y1 as f64),
                ));
            }
        }
        Ok(())
    }
}

fn make_slice_pipeline(device: &wgpu::Device) -> (wgpu::ComputePipeline, wgpu::BindGroupLayout) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("slice-plane"),
        source: wgpu::ShaderSource::Wgsl(include_str!("slice.wgsl").into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("slice-bgl"),
        entries: &[
            buffer_entry(0, wgpu::BufferBindingType::Uniform),
            buffer_entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
            buffer_entry(2, wgpu::BufferBindingType::Storage { read_only: true }),
            buffer_entry(3, wgpu::BufferBindingType::Storage { read_only: true }),
            buffer_entry(4, wgpu::BufferBindingType::Storage { read_only: false }),
            buffer_entry(5, wgpu::BufferBindingType::Storage { read_only: false }),
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("slice-pl"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("slice-plane-pipeline"),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    (pipeline, bgl)
}

fn make_occ_pipeline(device: &wgpu::Device) -> (wgpu::ComputePipeline, wgpu::BindGroupLayout) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("occupancy"),
        source: wgpu::ShaderSource::Wgsl(include_str!("occupancy.wgsl").into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("occ-bgl"),
        entries: &[
            buffer_entry(0, wgpu::BufferBindingType::Uniform),
            buffer_entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
            buffer_entry(2, wgpu::BufferBindingType::Storage { read_only: true }),
            buffer_entry(3, wgpu::BufferBindingType::Storage { read_only: false }),
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("occ-pl"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("occupancy-pipeline"),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    (pipeline, bgl)
}

fn make_gyro_pipeline(device: &wgpu::Device) -> (wgpu::ComputePipeline, wgpu::BindGroupLayout) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("gyroid-field"),
        source: wgpu::ShaderSource::Wgsl(include_str!("gyroid.wgsl").into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("gyro-bgl"),
        entries: &[
            buffer_entry(0, wgpu::BufferBindingType::Uniform),
            buffer_entry(1, wgpu::BufferBindingType::Storage { read_only: false }),
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("gyro-pl"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("gyroid-pipeline"),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    (pipeline, bgl)
}

fn buffer_entry(binding: u32, ty: wgpu::BufferBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn map_u32(device: &wgpu::Device, buf: &wgpu::Buffer) -> Result<u32, GpuError> {
    let slice = buf.slice(0..4);
    wait_mapped(device, slice)?;
    let data = slice.get_mapped_range();
    let bytes: [u8; 4] = data[..4]
        .try_into()
        .map_err(|_| GpuError::Request("count readback truncated".into()))?;
    let n = u32::from_le_bytes(bytes);
    drop(data);
    buf.unmap();
    Ok(n)
}

fn map_u32s(device: &wgpu::Device, buf: &wgpu::Buffer, n: usize) -> Result<Vec<u32>, GpuError> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let bytes = n * 4;
    let slice = buf.slice(0..bytes as u64);
    wait_mapped(device, slice)?;
    let data = slice.get_mapped_range();
    let out: Vec<u32> = bytemuck::cast_slice(&data[..bytes]).to_vec();
    drop(data);
    buf.unmap();
    Ok(out)
}

fn map_f32s(device: &wgpu::Device, buf: &wgpu::Buffer, n: usize) -> Result<Vec<f32>, GpuError> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let bytes = n * 4;
    let slice = buf.slice(0..bytes as u64);
    wait_mapped(device, slice)?;
    let data = slice.get_mapped_range();
    let out: Vec<f32> = bytemuck::cast_slice(&data[..bytes]).to_vec();
    drop(data);
    buf.unmap();
    Ok(out)
}

fn map_layer_segs(
    device: &wgpu::Device,
    buf: &wgpu::Buffer,
    n: usize,
) -> Result<Vec<GpuLayerSeg>, GpuError> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let bytes = n * std::mem::size_of::<GpuLayerSeg>();
    let slice = buf.slice(0..bytes as u64);
    wait_mapped(device, slice)?;
    let data = slice.get_mapped_range();
    let segs: Vec<GpuLayerSeg> = bytemuck::cast_slice(&data[..bytes]).to_vec();
    drop(data);
    buf.unmap();
    Ok(segs)
}

fn wait_mapped(device: &wgpu::Device, slice: wgpu::BufferSlice<'_>) -> Result<(), GpuError> {
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| GpuError::Request(e.to_string()))?;
    rx.recv()
        .map_err(|e| GpuError::Request(e.to_string()))?
        .map_err(|e| GpuError::Request(e.to_string()))
}

fn poly_bbox(region: &[Polygon]) -> Option<(Point, Point)> {
    let mut min_x = i64::MAX;
    let mut min_y = i64::MAX;
    let mut max_x = i64::MIN;
    let mut max_y = i64::MIN;
    for poly in region {
        for p in poly {
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
        }
    }
    if min_x > max_x {
        None
    } else {
        Some((Point::new(min_x, min_y), Point::new(max_x, max_y)))
    }
}

fn marching_squares(
    field: &[f32],
    grid_w: u32,
    grid_h: u32,
    origin_x: f32,
    origin_y: f32,
    cell: f32,
) -> Vec<Polyline> {
    let mut out = Vec::new();
    let w = grid_w as usize;
    let h = grid_h as usize;
    for gy in 0..h.saturating_sub(1) {
        for gx in 0..w.saturating_sub(1) {
            let v00 = field[gy * w + gx];
            let v10 = field[gy * w + gx + 1];
            let v01 = field[(gy + 1) * w + gx];
            let v11 = field[(gy + 1) * w + gx + 1];
            let bits = (u8::from(v00 > 0.0))
                | (u8::from(v10 > 0.0) << 1)
                | (u8::from(v11 > 0.0) << 2)
                | (u8::from(v01 > 0.0) << 3);
            if bits == 0 || bits == 15 {
                continue;
            }
            let x = origin_x + gx as f32 * cell;
            let y = origin_y + gy as f32 * cell;
            let lerp = |a: f32, b: f32| {
                let t = if (b - a).abs() < 1e-8 {
                    0.5
                } else {
                    (-a) / (b - a)
                };
                t.clamp(0.0, 1.0)
            };
            let e = [
                (x + lerp(v00, v10) * cell, y),
                (x + cell, y + lerp(v10, v11) * cell),
                (x + lerp(v01, v11) * cell, y + cell),
                (x, y + lerp(v00, v01) * cell),
            ];
            let pairs: &[[usize; 2]] = match bits {
                1 | 14 => &[[3, 0]],
                2 | 13 => &[[0, 1]],
                3 | 12 => &[[3, 1]],
                4 | 11 => &[[1, 2]],
                6 | 9 => &[[0, 2]],
                7 | 8 => &[[3, 2]],
                5 => &[[3, 0], [1, 2]],
                10 => &[[0, 1], [2, 3]],
                _ => &[],
            };
            for pair in pairs {
                out.push(vec![
                    Point::from_mm(e[pair[0]].0 as f64, e[pair[0]].1 as f64),
                    Point::from_mm(e[pair[1]].0 as f64, e[pair[1]].1 as f64),
                ]);
            }
        }
    }
    out
}
