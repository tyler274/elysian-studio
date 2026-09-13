struct RtUniforms {
    view_inv: mat4x4<f32>,
    proj_inv: mat4x4<f32>,
    light_dir: vec4<f32>,
    eye: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> uniforms: RtUniforms;
@group(0) @binding(1)
var output: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2)
var acc_struct: acceleration_structure;
@group(0) @binding(3)
var<storage, read> positions: array<vec4<f32>>;
@group(0) @binding(4)
var<storage, read> indices: array<u32>;

const SKY: vec3<f32> = vec3<f32>(0.07, 0.075, 0.09);
const BED: vec3<f32> = vec3<f32>(0.22, 0.23, 0.26);
const PLASTIC: vec3<f32> = vec3<f32>(0.93, 0.42, 0.18);
const SUN: vec3<f32> = vec3<f32>(1.6, 1.45, 1.25);
const SPP: u32 = 4u;
const BOUNCES: u32 = 3u;

fn pcg(state: ptr<function, u32>) -> f32 {
    *state = *state * 747796405u + 2891336453u;
    var x = *state;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    x = (x >> 22u) ^ x;
    return f32(x) * (1.0 / 4294967296.0);
}

fn orthonormal(n: vec3<f32>) -> mat3x3<f32> {
    let helper = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.z) > 0.9);
    let t = normalize(cross(helper, n));
    let b = cross(n, t);
    return mat3x3<f32>(t, b, n);
}

fn cosine_hemisphere(n: vec3<f32>, u: f32, v: f32) -> vec3<f32> {
    let r = sqrt(u);
    let phi = 6.2831853 * v;
    let local = vec3<f32>(r * cos(phi), r * sin(phi), sqrt(max(1.0 - u, 0.0)));
    return orthonormal(n) * local;
}

fn albedo(instance_index: u32) -> vec3<f32> {
    if instance_index == 0u {
        return BED;
    }
    return PLASTIC;
}

fn intersect(origin: vec3<f32>, direction: vec3<f32>, tmin: f32) -> RayIntersection {
    var rq: ray_query;
    rayQueryInitialize(&rq, acc_struct, RayDesc(0u, 0xffu, tmin, 4000.0, origin, direction));
    rayQueryProceed(&rq);
    return rayQueryGetCommittedIntersection(&rq);
}

fn hit_point(origin: vec3<f32>, direction: vec3<f32>, hit: RayIntersection) -> vec3<f32> {
    let base = hit.instance_custom_data + hit.primitive_index * 3u;
    let a = positions[indices[base]].xyz;
    let b = positions[indices[base + 1u]].xyz;
    let c = positions[indices[base + 2u]].xyz;
    var n = normalize(cross(b - a, c - a));
    if dot(n, -direction) < 0.0 {
        n = -n;
    }
    return origin + direction * hit.t + n * 0.08;
}

fn hit_normal(direction: vec3<f32>, hit: RayIntersection) -> vec3<f32> {
    let base = hit.instance_custom_data + hit.primitive_index * 3u;
    let a = positions[indices[base]].xyz;
    let b = positions[indices[base + 1u]].xyz;
    let c = positions[indices[base + 2u]].xyz;
    var n = normalize(cross(b - a, c - a));
    if dot(n, -direction) < 0.0 {
        n = -n;
    }
    return n;
}

fn camera_ray(uv: vec2<f32>) -> vec3<f32> {
    // Match `OrbitCamera::ray_from_ndc`: NDC y = +1 at the top of the pane.
    let d = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let near = uniforms.proj_inv * vec4<f32>(d.x, d.y, 0.0, 1.0);
    let far = uniforms.proj_inv * vec4<f32>(d.x, d.y, 1.0, 1.0);
    let n = uniforms.view_inv * vec4<f32>(near.xyz / max(near.w, 1e-8), 1.0);
    let f = uniforms.view_inv * vec4<f32>(far.xyz / max(far.w, 1e-8), 1.0);
    return normalize(f.xyz - n.xyz);
}

fn trace_path(origin0: vec3<f32>, dir0: vec3<f32>, rng: ptr<function, u32>) -> vec3<f32> {
    var origin = origin0;
    var direction = dir0;
    var throughput = vec3<f32>(1.0);
    var color = vec3<f32>(0.0);
    let sun_dir = normalize(uniforms.light_dir.xyz);

    for (var bounce = 0u; bounce < BOUNCES; bounce++) {
        let hit = intersect(origin, direction, select(0.1, 0.05, bounce == 0u));
        if hit.kind == RAY_QUERY_INTERSECTION_NONE {
            color += throughput * SKY;
            break;
        }
        let n = hit_normal(direction, hit);
        let p = origin + direction * hit.t + n * 0.08;
        let mat = albedo(hit.instance_index);

        let shadow = intersect(p, sun_dir, 0.2);
        if shadow.kind == RAY_QUERY_INTERSECTION_NONE {
            let ndl = max(dot(n, sun_dir), 0.0);
            color += throughput * mat * SUN * ndl;
        }

        throughput *= mat;
        let u1 = pcg(rng);
        let u2 = pcg(rng);
        direction = cosine_hemisphere(n, u1, u2);
        origin = p;
        if bounce >= 1u {
            let p_surv = max(max(throughput.x, throughput.y), throughput.z);
            if pcg(rng) > p_surv {
                break;
            }
            throughput /= max(p_surv, 0.05);
        }
    }
    return color;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(output);
    if (gid.x >= size.x || gid.y >= size.y) {
        return;
    }
    var rng = gid.x * 1973u + gid.y * 9277u + 1u;
    var acc = vec3<f32>(0.0);
    for (var s = 0u; s < SPP; s++) {
        let jx = pcg(&rng) - 0.5;
        let jy = pcg(&rng) - 0.5;
        let pixel = vec2<f32>(gid.xy) + vec2<f32>(0.5 + jx, 0.5 + jy);
        let uv = pixel / vec2<f32>(size.xy);
        let origin = uniforms.eye.xyz;
        let direction = camera_ray(uv);
        acc += trace_path(origin, direction, &rng);
        rng += 17u + s * 1013u;
    }
    var color = acc / f32(SPP);
    color = color / (color + vec3<f32>(1.0));
    color = pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    textureStore(output, gid.xy, vec4<f32>(color, 1.0));
}
