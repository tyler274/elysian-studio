struct OccParams {
    tri_count: u32,
    grid_w: u32,
    grid_h: u32,
    cell: f32,
    origin_x: f32,
    origin_y: f32,
    z: f32,
    z_below: f32,
};

struct Vertex {
    p: vec4<f32>,
};

struct Triangle {
    i0: u32,
    i1: u32,
    i2: u32,
    _pad: u32,
};

@group(0) @binding(0) var<uniform> params: OccParams;
@group(0) @binding(1) var<storage, read> vertices: array<Vertex>;
@group(0) @binding(2) var<storage, read> triangles: array<Triangle>;
@group(0) @binding(3) var<storage, read_write> cells: array<atomic<u32>>;

const EPS: f32 = 1e-6;

fn hit_xy(a: vec3<f32>, b: vec3<f32>, z: f32) -> vec2<f32> {
    let t = (a.z - z) / (a.z - b.z);
    let p = mix(a, b, t);
    return p.xy;
}

fn stamp(p: vec2<f32>, bit: u32) {
    let gx = i32(floor((p.x - params.origin_x) / params.cell));
    let gy = i32(floor((p.y - params.origin_y) / params.cell));
    if gx < 0 || gy < 0 || u32(gx) >= params.grid_w || u32(gy) >= params.grid_h {
        return;
    }
    let idx = u32(gy) * params.grid_w + u32(gx);
    atomicOr(&cells[idx], bit);
}

fn stamp_edge(a: vec3<f32>, b: vec3<f32>, z: f32, bit: u32) {
    let da = a.z - z;
    let db = b.z - z;
    if abs(da) <= EPS {
        stamp(a.xy, bit);
    }
    if abs(db) <= EPS {
        stamp(b.xy, bit);
    }
    if da * db < 0.0 {
        stamp(hit_xy(a, b, z), bit);
    }
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let t = gid.x;
    if t >= params.tri_count {
        return;
    }
    let tri = triangles[t];
    let a = vertices[tri.i0].p.xyz;
    let b = vertices[tri.i1].p.xyz;
    let c = vertices[tri.i2].p.xyz;
    stamp_edge(a, b, params.z, 1u);
    stamp_edge(b, c, params.z, 1u);
    stamp_edge(c, a, params.z, 1u);
    stamp_edge(a, b, params.z_below, 2u);
    stamp_edge(b, c, params.z_below, 2u);
    stamp_edge(c, a, params.z_below, 2u);
}
