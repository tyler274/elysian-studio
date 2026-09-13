struct GyroParams {
    origin_x: f32,
    origin_y: f32,
    cell: f32,
    z: f32,
    period: f32,
    grid_w: u32,
    grid_h: u32,
    _pad: u32,
};

@group(0) @binding(0) var<uniform> params: GyroParams;
@group(0) @binding(1) var<storage, read_write> field: array<f32>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.grid_w || gid.y >= params.grid_h {
        return;
    }
    let x = (params.origin_x + f32(gid.x) * params.cell) / params.period;
    let y = (params.origin_y + f32(gid.y) * params.cell) / params.period;
    let z = params.z / params.period;
    let v = select(
        cos(x) * sin(y) + cos(y) * sin(z) + cos(z) * sin(x),
        sin(x) + sin(y) + sin(z),
        params._pad != 0u,
    );
    let idx = gid.y * params.grid_w + gid.x;
    field[idx] = v;
}
