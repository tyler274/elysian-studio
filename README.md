# Bambu Studio (Rust)

AGPL-3.0-or-later rewrite of [Bambu Studio](https://github.com/bambulab/BambuStudio)
in safe Rust. This workspace does **not** dlopen proprietary `libbambu_networking`. Printer I/O is a
safe-Rust port of [open-bamboo-networking](https://github.com/ClusterM/open-bamboo-networking)
and [OpenBambuAPI](https://github.com/Doridian/OpenBambuAPI): LAN SSDP, MQTT topics, and
Option B command signing when **you** supply `slicer_*.pem`. Those PEMs are never shipped.

## Workspace

| Crate | Role |
|-------|------|
| `bambu-geom` | Scaled integer geometry, clipper, meshes |
| `bambu-config` | Slice / print settings |
| `bambu-model` | Objects, instances, plates |
| `bambu-io` | STL and 3MF mesh import |
| `bambu-slicer` | Layer slice → walls → top/bottom shells → infill → ironing → skirt/brim → classic supports |
| `bambu-gcode` | G-code writer |
| `bambu-preview` | CPU toolpath buffers for the GPU |
| `bambu-gpu` | wgpu Vulkan viewport + compute contours |
| `bambu-device` | Printer / AMS / camera traits (no I/O) |
| `bambu-protocol` | LAN SSDP, MQTT payloads, Option B RSA signing, credential extract |
| `bambu-cli` | Headless slice |
| `bambu-ui` | iced application |

First-party crates set `unsafe_code = "forbid"`. GPU work uses wgpu with the
Vulkan backend on Linux: the plater viewport, G-code preview overlay, and the
triangle–plane contour pass. Clipper union, walls, infill, top/bottom shells, ironing, skirt, brim, and
classic supports stay on the CPU for integer determinism. `bambu-cli slice` and the UI **Slice** button use
Vulkan compute when an adapter is present and fall back to CPU otherwise
(`--cpu` / `--gpu` to force).

## Build

Requires current **stable** Rust (`rust-toolchain.toml` tracks `stable`).

```bash
cargo test --workspace
cargo run -p bambu-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode
cargo run -p bambu-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode --gpu
cargo run -p bambu-cli -- slice model.3mf -o /tmp/model.gcode
cargo run -p bambu-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode --brim 5 --skirt 2 --top 4 --bottom 3
cargo run -p bambu-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode --ironing top
# table-like overhangs:
# cargo run -p bambu-cli -- slice overhang.stl -o /tmp/overhang.gcode --support
cargo run -p bambu-ui
```

The UI re-execs with `WGPU_BACKEND=vulkan` on Linux. Drag to orbit, scroll to zoom, **Open model** (STL/3MF), Slice.

Load the same Bambu process JSON the C++ app uses (`inherits` is followed in-directory):

```bash
cargo run -p bambu-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode --bbl-0-20
cargo run -p bambu-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode \
  --settings /home/luluco/code/BambuStudio/resources/profiles/BBL/process/0.20mm\ Standard\ @BBL\ X1C.json
```

`cargo test -p bambu-cli --test golden_cube` slices the 20 mm cube with that profile in Rust **and** with the upstream `bambu-studio --slice=0` CLI, then compares `CHANGE_LAYER` count, `FEATURE` roles, and the C++ `; CONFIG_BLOCK` values. The C++ binary is taken from `BAMBU_STUDIO` or `PATH`. Profiles come from `BAMBU_STUDIO_RESOURCES` or `../BambuStudio/resources`. Set `BAMBU_STUDIO_REQUIRE_ORACLE=1` to fail if the C++ CLI is missing.

## Printer network (open-bamboo-networking)

[Option A](https://github.com/ClusterM/open-bamboo-networking#option-a-developer-mode) is Developer Mode LAN (no signing keys). [Option B](https://github.com/ClusterM/open-bamboo-networking#option-b-cloud-mode-without-developer-mode) needs `slicer_cert.pem`, `slicer_key.pem`, and `slicer_crl.pem` extracted from the stock plugin **you already have**. Put them in `$XDG_CONFIG_HOME/bambu-studio-rs/` (never commit them).

```bash
cargo run -p bambu-cli -- keys extract --plugin /path/to/libbambu_networking.so
cargo run -p bambu-cli -- keys status
cargo run -p bambu-cli -- device discover --timeout 3
cargo run -p bambu-cli -- device status --host 192.168.1.42 --code 12345678
cargo run -p bambu-cli -- device send cube.gcode --host 192.168.1.42 --code 12345678
cargo run -p bambu-cli -- device gcode --host 192.168.1.42 --code 12345678 --line G28
```

`device status` / `send` use MQTT `:8883` (user `bblp`, password = access code, self-signed TLS) and `send` uploads a `.gcode.3mf` over implicit FTPS `:990` then publishes `project_file`. Serial can be omitted: the MQTT certificate CN is used. `push_status.fun` bit 29 selects Developer Mode vs secured: secured printers get `url_enc`/`param_enc` (device-cert RSA) and optional `app_cert_install` when `slicer_cert.pem` + `slicer_crl.pem` are present.

```bash
cargo run -p bambu-cli -- device camera --host 192.168.1.42 --code 12345678 --output /tmp/chamber.jpg
cargo run -p bambu-cli -- device install-cert --host 192.168.1.42 --code 12345678
```

P1/A1 chamber JPEG is TLS `:6000`. X1/H2 use RTSPS `:322` (not this snapshot path). The UI **Chamber snapshot** button reports frame size after the same JPEG grab.

The UI **Extract keys** / **Discover printers** / **Send last slice** buttons run the same paths. C++ CLI leftovers such as `result.json` are gitignored.

Nix:

```bash
nix build .#bambu-cli
nix build .#bambu-ui
```
