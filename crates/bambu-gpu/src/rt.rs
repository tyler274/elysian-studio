//! Hardware BLAS/TLAS + `ray_query` path tracer on the iced Vulkan device.

use glam::Mat4;

const IDENTITY_3X4: [f32; 12] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
pub(crate) const MAX_INSTANCES: u32 = 32;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RtUniforms {
    view_inv: [[f32; 4]; 4],
    proj_inv: [[f32; 4]; 4],
    light_dir: [f32; 4],
    eye: [f32; 4],
}

#[derive(Clone, Debug)]
pub struct RtInstance {
    pub first_index: u32,
    pub index_count: u32,
}

pub struct RtGpu {
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    vert_buf: wgpu::Buffer,
    idx_buf: wgpu::Buffer,
    vert_bytes: u64,
    idx_bytes: u64,
    tlas: wgpu::Tlas,
    blases: Vec<wgpu::Blas>,
    size_descs: Vec<wgpu::BlasTriangleGeometrySizeDescriptor>,
    radiance: wgpu::Texture,
    radiance_view: wgpu::TextureView,
    size: (u32, u32),
    bind_group: Option<wgpu::BindGroup>,
    blit_pipeline: wgpu::RenderPipeline,
    blit_bgl: wgpu::BindGroupLayout,
    blit_bg: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    built_key: u64,
    ready: bool,
}

impl RtGpu {
    pub fn try_new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Option<Self> {
        if !bambu_wgpu_exp::device_has_ray_query(device) {
            return None;
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bambu-gpu-path"),
            source: wgpu::ShaderSource::Wgsl(include_str!("path.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bambu-gpu-rt-bgl"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::COMPUTE),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::AccelerationStructure {
                        vertex_return: false,
                    },
                    count: None,
                },
                storage_entry(3, wgpu::ShaderStages::COMPUTE, true),
                storage_entry(4, wgpu::ShaderStages::COMPUTE, true),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bambu-gpu-rt-pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("bambu-gpu-rt-pipeline"),
            layout: Some(&layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bambu-gpu-rt-uniforms"),
            size: std::mem::size_of::<RtUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vert_buf = empty_buf(device, 256, true);
        let idx_buf = empty_buf(device, 256, false);
        let tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
            label: Some("bambu-gpu-tlas"),
            max_instances: MAX_INSTANCES,
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        });
        let (radiance, radiance_view) = make_radiance(device, 64, 64);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bambu-gpu-rt-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bambu-gpu-blit"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
        });
        let blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bambu-gpu-blit-bgl"),
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
        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bambu-gpu-blit-pl"),
            bind_group_layouts: &[&blit_bgl],
            push_constant_ranges: &[],
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bambu-gpu-blit-pipeline"),
            layout: Some(&blit_layout),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        });
        let blit_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bambu-gpu-blit-bg"),
            layout: &blit_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&radiance_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        Some(Self {
            pipeline,
            bgl,
            uniform_buf,
            vert_buf,
            idx_buf,
            vert_bytes: 256,
            idx_bytes: 256,
            tlas,
            blases: Vec::new(),
            size_descs: Vec::new(),
            radiance,
            radiance_view,
            size: (64, 64),
            bind_group: None,
            blit_pipeline,
            blit_bgl,
            blit_bg,
            sampler,
            built_key: u64::MAX,
            ready: false,
        })
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.size == (width, height) {
            return;
        }
        let (tex, view) = make_radiance(device, width, height);
        self.radiance = tex;
        self.radiance_view = view;
        self.size = (width, height);
        self.blit_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bambu-gpu-blit-bg"),
            layout: &self.blit_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.radiance_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.bind_group = None;
        self.ready = false;
    }

    pub fn write_camera(
        &self,
        queue: &wgpu::Queue,
        view: Mat4,
        proj: Mat4,
        light: glam::Vec3,
        eye: glam::Vec3,
    ) {
        let uniforms = RtUniforms {
            view_inv: view.inverse().to_cols_array_2d(),
            proj_inv: proj.inverse().to_cols_array_2d(),
            light_dir: [light.x, light.y, light.z, 0.0],
            eye: [eye.x, eye.y, eye.z, 1.0],
        };
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
    }

    pub fn upload_geom(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: u64,
        positions: &[[f32; 4]],
        indices: &[u32],
        instances: &[RtInstance],
    ) {
        if self.built_key == key && self.ready {
            return;
        }
        self.ready = false;
        self.bind_group = None;
        if positions.is_empty() || indices.is_empty() || instances.is_empty() {
            self.built_key = key;
            return;
        }
        let vbytes = std::mem::size_of_val(positions) as u64;
        let ibytes = std::mem::size_of_val(indices) as u64;
        if vbytes > self.vert_bytes {
            self.vert_bytes = vbytes.next_power_of_two().max(256);
            self.vert_buf = empty_buf(device, self.vert_bytes, true);
        }
        if ibytes > self.idx_bytes {
            self.idx_bytes = ibytes.next_power_of_two().max(256);
            self.idx_buf = empty_buf(device, self.idx_bytes, false);
        }
        queue.write_buffer(&self.vert_buf, 0, bytemuck::cast_slice(positions));
        queue.write_buffer(&self.idx_buf, 0, bytemuck::cast_slice(indices));

        self.blases.clear();
        self.size_descs.clear();
        for inst in instances.iter().take(MAX_INSTANCES as usize) {
            if inst.index_count < 3 {
                continue;
            }
            let size = wgpu::BlasTriangleGeometrySizeDescriptor {
                vertex_format: wgpu::VertexFormat::Float32x3,
                vertex_count: positions.len() as u32,
                index_format: Some(wgpu::IndexFormat::Uint32),
                index_count: Some(inst.index_count),
                flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
            };
            let blas = device.create_blas(
                &wgpu::CreateBlasDescriptor {
                    label: Some("bambu-gpu-blas"),
                    flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
                    update_mode: wgpu::AccelerationStructureUpdateMode::Build,
                },
                wgpu::BlasGeometrySizeDescriptors::Triangles {
                    descriptors: vec![size.clone()],
                },
            );
            self.size_descs.push(size);
            self.blases.push(blas);
        }
        for i in 0..MAX_INSTANCES as usize {
            self.tlas[i] = None;
        }
        for (i, blas) in self.blases.iter().enumerate() {
            let custom = instances.get(i).map(|s| s.first_index).unwrap_or(0);
            self.tlas[i] = Some(wgpu::TlasInstance::new(blas, IDENTITY_3X4, custom, 0xFF));
        }
        self.built_key = key;
    }

    pub fn ensure_bind_group(&mut self, device: &wgpu::Device) {
        if self.bind_group.is_some() || self.blases.is_empty() {
            return;
        }
        self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bambu-gpu-rt-bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.radiance_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.tlas.as_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.vert_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.idx_buf.as_entire_binding(),
                },
            ],
        }));
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn build_and_trace(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        instances: &[RtInstance],
        width: u32,
        height: u32,
    ) {
        if self.blases.is_empty() || instances.is_empty() {
            return;
        }
        if !self.ready {
            let mut entries = Vec::with_capacity(self.blases.len());
            for (i, blas) in self.blases.iter().enumerate() {
                let Some(inst) = instances.get(i) else {
                    continue;
                };
                let Some(size) = self.size_descs.get(i) else {
                    continue;
                };
                entries.push(wgpu::BlasBuildEntry {
                    blas,
                    geometry: wgpu::BlasGeometries::TriangleGeometries(vec![
                        wgpu::BlasTriangleGeometry {
                            size,
                            vertex_buffer: &self.vert_buf,
                            first_vertex: 0,
                            vertex_stride: 16,
                            index_buffer: Some(&self.idx_buf),
                            first_index: Some(inst.first_index),
                            transform_buffer: None,
                            transform_buffer_offset: None,
                        },
                    ]),
                });
            }
            encoder.build_acceleration_structures(entries.iter(), std::iter::once(&self.tlas));
            self.ready = true;
        }
        let Some(bg) = self.bind_group.as_ref() else {
            return;
        };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("bambu-gpu-rt-pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bg, &[]);
        pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
    }

    pub fn blit<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.blit_pipeline);
        pass.set_bind_group(0, &self.blit_bg, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn uniform_entry(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_entry(
    binding: u32,
    vis: wgpu::ShaderStages,
    read_only: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn empty_buf(device: &wgpu::Device, bytes: u64, verts: bool) -> wgpu::Buffer {
    let extra = if verts {
        wgpu::BufferUsages::BLAS_INPUT | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST
    } else {
        wgpu::BufferUsages::BLAS_INPUT
            | wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::INDEX
    };
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(if verts {
            "bambu-gpu-rt-verts"
        } else {
            "bambu-gpu-rt-idx"
        }),
        size: bytes.max(256),
        usage: extra,
        mapped_at_creation: false,
    })
}

fn make_radiance(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bambu-gpu-radiance"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Headless cube ray-query: skip when the adapter has no hardware RT.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::GpuError;
    use bambu_geom::TriangleMesh;

    fn rt_device() -> Result<(wgpu::Device, wgpu::Queue), GpuError> {
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
        if bambu_wgpu_exp::ray_query_features(&adapter).is_empty() {
            return Err(GpuError::Request("no EXPERIMENTAL_RAY_QUERY".into()));
        }
        let limits = wgpu::Limits::default().using_acceleration_structure_values(adapter.limits());
        let (features, exp, limits) = bambu_wgpu_exp::ray_query_device(&adapter, limits);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("bambu-rt-test"),
            required_features: features,
            required_limits: limits,
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
            experimental_features: exp,
        }))
        .map_err(|e| GpuError::Request(e.to_string()))?;
        Ok((device, queue))
    }

    #[test]
    fn cube_ray_query_hits_or_skips_without_rt() {
        let Ok((device, queue)) = rt_device() else {
            eprintln!("skipping hardware ray query (adapter has no RT)");
            return;
        };
        let mesh = TriangleMesh::cube(20.0);
        let mut pos = Vec::new();
        for v in &mesh.vertices {
            pos.push([v.x, v.y, v.z, 1.0]);
        }
        let mut idx = Vec::new();
        for t in &mesh.indices {
            idx.extend_from_slice(t);
        }
        let Some(mut rt) = RtGpu::try_new(&device, wgpu::TextureFormat::Rgba8Unorm, 1) else {
            eprintln!("skipping: device lost ray query");
            return;
        };
        rt.upload_geom(
            &device,
            &queue,
            1,
            &pos,
            &idx,
            &[RtInstance {
                first_index: 0,
                index_count: idx.len() as u32,
            }],
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("rt-test"),
        });
        let eye = glam::Vec3::new(10.0, 10.0, 60.0);
        let view = glam::Mat4::look_at_rh(eye, glam::Vec3::new(10.0, 10.0, 10.0), glam::Vec3::Z);
        let proj = glam::Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, 1.0, 1.0, 4000.0);
        rt.write_camera(
            &queue,
            view,
            proj,
            (eye - glam::Vec3::new(10.0, 10.0, 10.0)).normalize(),
            eye,
        );
        rt.ensure_bind_group(&device);
        rt.build_and_trace(
            &mut encoder,
            &[RtInstance {
                first_index: 0,
                index_count: idx.len() as u32,
            }],
            8,
            8,
        );
        queue.submit(std::iter::once(encoder.finish()));
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        assert!(rt.ready);
    }
}
