# Bambu Studio (Rust)

AGPL-3.0-or-later rewrite of [Bambu Studio](https://github.com/bambulab/BambuStudio)
in safe Rust. The C++ tree remains the behavioral oracle; this workspace does
**not** load `libbambu_networking`.

## Workspace

| Crate | Role |
|-------|------|
| `bambu-geom` | Scaled integer geometry, clipper, meshes |
| `bambu-config` | Slice / print settings |
| `bambu-model` | Objects, instances, plates |
| `bambu-io` | STL (3MF later) |
| `bambu-slicer` | Layer slice → walls → top/bottom shells → infill → skirt/brim → classic supports |
| `bambu-gcode` | G-code writer |
| `bambu-preview` | CPU toolpath buffers for the GPU |
| `bambu-gpu` | wgpu Vulkan viewport + compute contours |
| `bambu-device` | Printer / AMS / camera traits (no I/O) |
| `bambu-protocol` | LAN MQTT/FTPS backend (stub) |
| `bambu-cli` | Headless slice |
| `bambu-ui` | iced application |

First-party crates set `unsafe_code = "forbid"`. GPU work uses wgpu with the
Vulkan backend on Linux: the plater viewport, G-code preview overlay, and the
triangle–plane contour pass. Clipper union, walls, infill, top/bottom shells, skirt, brim, and
classic supports stay on the CPU for integer determinism. `bambu-cli slice` and the UI **Slice** button use
Vulkan compute when an adapter is present and fall back to CPU otherwise
(`--cpu` / `--gpu` to force).

## Build

Requires current **stable** Rust (`rust-toolchain.toml` tracks `stable`).

```bash
cargo test --workspace
cargo run -p bambu-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode
cargo run -p bambu-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode --gpu
cargo run -p bambu-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode --brim 5 --skirt 2 --top 4 --bottom 3
# table-like overhangs:
# cargo run -p bambu-cli -- slice overhang.stl -o /tmp/overhang.gcode --support
cargo run -p bambu-ui
```

The UI re-execs with `WGPU_BACKEND=vulkan` on Linux. Drag to orbit, scroll to zoom, Open STL, Slice.

Load the same Bambu process JSON the C++ app uses (`inherits` is followed in-directory):

```bash
cargo run -p bambu-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode --bbl-0-20
cargo run -p bambu-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode \
  --settings /home/luluco/code/BambuStudio/resources/profiles/BBL/process/0.20mm\ Standard\ @BBL\ X1C.json
```

`cargo test -p bambu-cli --test golden_cube` slices the 20 mm cube with that profile in Rust **and** with the upstream `bambu-studio --slice=0` CLI, then compares `CHANGE_LAYER` count, `FEATURE` roles, and the C++ `; CONFIG_BLOCK` values. The C++ binary is taken from `BAMBU_STUDIO` or `PATH`. Profiles come from `BAMBU_STUDIO_RESOURCES` or `../BambuStudio/resources`. Set `BAMBU_STUDIO_REQUIRE_ORACLE=1` to fail if the C++ CLI is missing.

Nix:

```bash
nix build .#bambu-cli
nix build .#bambu-ui
```
