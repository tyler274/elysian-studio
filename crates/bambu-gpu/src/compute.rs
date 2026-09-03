//! Headless Vulkan compute: triangle–plane intersection per layer.
//!
//! Mesh vertices/indices are uploaded once and reused for every Z. Clipper
//! union, walls, and infill stay on the CPU (integer, deterministic).

use bambu_geom::{union_polygons, Polygon, TriangleMesh};
use bambu_slicer::{loops_from_segments, point_from_xy_mm};
use wgpu::util::DeviceExt;

use crate::GpuError;

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
    z: f32,
    tri_count: u32,
    _pad0: u32,
    _pad1: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuSegment {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

struct UploadedMesh {
    vertex_buf: wgpu::Buffer,
    tri_buf: wgpu::Buffer,
    tri_count: u32,
}

struct Scratch {
    param_buf: wgpu::Buffer,
    count_buf: wgpu::Buffer,
    seg_buf: wgpu::Buffer,
    count_read: wgpu::Buffer,
    seg_read: wgpu::Buffer,
    cap: u64,
}

/// Persistent Vulkan compute device used by CLI and UI slice runs.
pub struct VulkanSliceAccel {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
}

impl VulkanSliceAccel {
    pub fn new() -> Result<Self, GpuError> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
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
                buffer_entry(3, wgpu::BufferBindingType::Storage { read_only: false }),
                buffer_entry(4, wgpu::BufferBindingType::Storage { read_only: false }),
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

        Ok(Self {
            device,
            queue,
            pipeline,
            bgl,
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
        if upload.tri_count == 0 {
            return Ok(zs.iter().map(|&z| (z, Vec::new())).collect());
        }
        let scratch = self.make_scratch(upload.tri_count)?;
        let mut out = Vec::with_capacity(zs.len());
        for &z in zs {
            out.push((z, self.dispatch_layer(&upload, &scratch, z as f32)?));
        }
        Ok(out)
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
        let vertex_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("slice-verts"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let tri_bytes = if tris.is_empty() {
            &[0u8; 16][..]
        } else {
            bytemuck::cast_slice(&tris)
        };
        let tri_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("slice-tris"),
            contents: tri_bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        Ok(UploadedMesh {
            vertex_buf,
            tri_buf,
            tri_count: tris.len() as u32,
        })
    }

    fn make_scratch(&self, tri_count: u32) -> Result<Scratch, GpuError> {
        let cap = tri_count.max(1) as u64;
        let seg_size = (cap * std::mem::size_of::<GpuSegment>() as u64).max(16);
        Ok(Scratch {
            param_buf: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("slice-params"),
                size: std::mem::size_of::<GpuParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            count_buf: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("slice-count"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            seg_buf: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("slice-segs"),
                size: seg_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            count_read: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("slice-count-read"),
                size: 16,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            seg_read: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("slice-seg-read"),
                size: seg_size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            cap,
        })
    }

    fn dispatch_layer(
        &self,
        mesh: &UploadedMesh,
        scratch: &Scratch,
        z: f32,
    ) -> Result<Vec<Polygon>, GpuError> {
        let params = GpuParams {
            z,
            tri_count: mesh.tri_count,
            _pad0: 0,
            _pad1: 0,
        };
        self.queue
            .write_buffer(&scratch.param_buf, 0, bytemuck::bytes_of(&params));
        self.queue
            .write_buffer(&scratch.count_buf, 0, bytemuck::bytes_of(&[0u32; 4]));

        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("slice-bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: scratch.param_buf.as_entire_binding(),
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
                    resource: scratch.count_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: scratch.seg_buf.as_entire_binding(),
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
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            let groups = mesh.tri_count.div_ceil(64);
            pass.dispatch_workgroups(groups.max(1), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&scratch.count_buf, 0, &scratch.count_read, 0, 16);
        let seg_bytes = (scratch.cap * std::mem::size_of::<GpuSegment>() as u64).max(16);
        encoder.copy_buffer_to_buffer(&scratch.seg_buf, 0, &scratch.seg_read, 0, seg_bytes);
        self.queue.submit(Some(encoder.finish()));

        let n = map_u32(&self.device, &scratch.count_read)?;
        let n = (n as usize).min(scratch.cap as usize);
        let segs = map_segments(&self.device, &scratch.seg_read, n)?;

        let segments: Vec<_> = segs
            .into_iter()
            .map(|s| {
                (
                    point_from_xy_mm(s.x0 as f64, s.y0 as f64),
                    point_from_xy_mm(s.x1 as f64, s.y1 as f64),
                )
            })
            .filter(|(a, b)| a != b)
            .collect();
        Ok(union_polygons(&loops_from_segments(&segments)))
    }
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

fn map_segments(
    device: &wgpu::Device,
    buf: &wgpu::Buffer,
    n: usize,
) -> Result<Vec<GpuSegment>, GpuError> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let bytes = n * std::mem::size_of::<GpuSegment>();
    let slice = buf.slice(0..bytes as u64);
    wait_mapped(device, slice)?;
    let data = slice.get_mapped_range();
    let segs: Vec<GpuSegment> = bytemuck::cast_slice(&data[..bytes]).to_vec();
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
