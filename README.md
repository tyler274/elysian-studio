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
| `bambu-slicer` | Layer slice → walls → infill |
| `bambu-gcode` | G-code writer |
| `bambu-preview` | CPU toolpath buffers for the GPU |
| `bambu-gpu` | wgpu / Vulkan viewport |
| `bambu-device` | Printer / AMS / camera traits (no I/O) |
| `bambu-protocol` | LAN MQTT/FTPS backend (stub) |
| `bambu-cli` | Headless slice |
| `bambu-ui` | iced application |

First-party crates set `unsafe_code = "forbid"`. GPU work uses wgpu with the
Vulkan backend on Linux. Toolpaths are CPU-generated.

## Build

Requires current **stable** Rust (`rust-toolchain.toml` tracks `stable`).

```bash
cargo test --workspace
cargo run -p bambu-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode
cargo run -p bambu-ui
```

The UI re-execs with `WGPU_BACKEND=vulkan` on Linux. Drag to orbit, scroll to zoom, Open STL, Slice.

Nix:

```bash
nix build .#bambu-cli
nix build .#bambu-ui
```
