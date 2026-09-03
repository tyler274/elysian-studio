struct Params {
    z: f32,
    tri_count: u32,
    _pad0: u32,
    _pad1: u32,
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

struct Segment {
    a: vec2<f32>,
    b: vec2<f32>,
};

struct Counter {
    n: atomic<u32>,
    _p0: u32,
    _p1: u32,
    _p2: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> vertices: array<Vertex>;
@group(0) @binding(2) var<storage, read> triangles: array<Triangle>;
@group(0) @binding(3) var<storage, read_write> counter: Counter;
@group(0) @binding(4) var<storage, read_write> segments: array<Segment>;

const EPS: f32 = 1e-6;

fn hit(a: vec3<f32>, b: vec3<f32>, z: f32) -> vec3<f32> {
    let t = (a.z - z) / (a.z - b.z);
    return mix(a, b, t);
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
    let z = params.z;

    var p0 = vec3<f32>(0.0);
    var p1 = vec3<f32>(0.0);
    var n: u32 = 0u;

    n = accum_edge(a, b, z, &p0, &p1, n);
    n = accum_edge(b, c, z, &p0, &p1, n);
    n = accum_edge(c, a, z, &p0, &p1, n);

    if n < 2u {
        return;
    }
    let slot = atomicAdd(&counter.n, 1u);
    if slot < arrayLength(&segments) {
        segments[slot] = Segment(p0.xy, p1.xy);
    }
}

fn accum_edge(
    a: vec3<f32>,
    b: vec3<f32>,
    z: f32,
    p0: ptr<function, vec3<f32>>,
    p1: ptr<function, vec3<f32>>,
    n: u32,
) -> u32 {
    if n >= 2u {
        return n;
    }
    let da = a.z - z;
    let db = b.z - z;
    var out_n = n;
    if abs(da) <= EPS && abs(db) <= EPS {
        out_n = store_hit(p0, p1, out_n, a);
        if out_n < 2u {
            out_n = store_hit(p0, p1, out_n, b);
        }
        return out_n;
    }
    if abs(da) <= EPS {
        return store_hit(p0, p1, out_n, a);
    }
    if abs(db) <= EPS {
        return store_hit(p0, p1, out_n, b);
    }
    if da * db < 0.0 {
        return store_hit(p0, p1, out_n, hit(a, b, z));
    }
    return out_n;
}

fn store_hit(
    p0: ptr<function, vec3<f32>>,
    p1: ptr<function, vec3<f32>>,
    n: u32,
    p: vec3<f32>,
) -> u32 {
    if n == 0u {
        *p0 = p;
        return 1u;
    }
    if n == 1u {
        *p1 = p;
        return 2u;
    }
    return n;
}
