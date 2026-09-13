struct RtUniforms {
    view_inv: mat4x4<f32>,
    proj_inv: mat4x4<f32>,
    light_dir: vec4<f32>,
    eye: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> uniforms: RtUniforms;
@group(0) @binding(1)
var output: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2)
var acc_struct: acceleration_structure;
@group(0) @binding(3)
var<storage, read> positions: array<vec4<f32>>;
@group(0) @binding(4)
var<storage, read> indices: array<u32>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(output);
    if (gid.x >= size.x || gid.y >= size.y) {
        return;
    }
    let pixel = vec2<f32>(gid.xy) + vec2<f32>(0.5);
    let uv = pixel / vec2<f32>(size.xy);
    let d = uv * 2.0 - 1.0;
    let origin = (uniforms.view_inv * vec4<f32>(0.0, 0.0, 0.0, 1.0)).xyz;
    let tmp = uniforms.proj_inv * vec4<f32>(d.x, d.y, 1.0, 1.0);
    let direction = (uniforms.view_inv * vec4<f32>(normalize(tmp.xyz), 0.0)).xyz;

    var rq: ray_query;
    rayQueryInitialize(&rq, acc_struct, RayDesc(0u, 0xffu, 0.1, 4000.0, origin, direction));
    rayQueryProceed(&rq);

    var color = vec3<f32>(0.07, 0.075, 0.09);
    let hit = rayQueryGetCommittedIntersection(&rq);
    if hit.kind != RAY_QUERY_INTERSECTION_NONE {
        let base = hit.instance_custom_data + hit.primitive_index * 3u;
        let i0 = indices[base];
        let i1 = indices[base + 1u];
        let i2 = indices[base + 2u];
        let a = positions[i0].xyz;
        let b = positions[i1].xyz;
        let c = positions[i2].xyz;
        let n = normalize(cross(b - a, c - a));
        let light = normalize(uniforms.light_dir.xyz);
        let ndl = max(dot(n, light), 0.18);
        color = vec3<f32>(0.93, 0.42, 0.18) * ndl;
        let shadow_origin = origin + direction * hit.t + n * 0.15;
        var rq2: ray_query;
        rayQueryInitialize(&rq2, acc_struct, RayDesc(0u, 0xffu, 0.2, 4000.0, shadow_origin, light));
        rayQueryProceed(&rq2);
        let sh = rayQueryGetCommittedIntersection(&rq2);
        if sh.kind != RAY_QUERY_INTERSECTION_NONE {
            color = color * 0.45;
        }
    }
    textureStore(output, gid.xy, vec4<f32>(color, 1.0));
}
