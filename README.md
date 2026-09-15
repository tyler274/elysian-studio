# Elysian Studio

AGPL-3.0-or-later rewrite of [Bambu Studio](https://github.com/bambulab/BambuStudio)
in safe Rust. This workspace does **not** dlopen proprietary `libbambu_networking`. Printer I/O is a
safe-Rust port of [open-bamboo-networking](https://github.com/ClusterM/open-bamboo-networking)
and [OpenBambuAPI](https://github.com/Doridian/OpenBambuAPI): LAN SSDP, MQTT topics, and
Option B command signing when **you** supply `slicer_*.pem`. Those PEMs are never shipped.

## Workspace

| Crate | Role |
|-------|------|
| `elysian-alloc` | Process `#[global_allocator]`: sibling mimalloc rewrite |
| `elysian-geom` | Scaled integer geometry, clipper, meshes |
| `elysian-config` | Slice / print settings |
| `elysian-model` | Objects, instances, plates |
| `elysian-io` | STL and 3MF mesh import |
| `elysian-slicer` | Layer slice → walls → top/bottom shells → infill → ironing → skirt/brim → classic supports |
| `elysian-gcode` | G-code writer |
| `elysian-preview` | CPU toolpath buffers for the GPU |
| `elysian-gpu` | wgpu Vulkan viewport + compute contours |
| `elysian-device` | Printer / AMS / camera traits (no I/O) |
| `elysian-protocol` | LAN SSDP, MQTT payloads, Option B RSA signing, credential extract |
| `elysian-cli` | Headless slice |
| `elysian-ui` | iced application |

First-party crates set `unsafe_code = "forbid"`. GPU work uses wgpu with the
Vulkan backend on Linux: the plater viewport, G-code preview overlay, and the
triangle–plane contour pass. Clipper union, walls, infill, top/bottom shells, ironing, skirt, brim, and
classic supports stay on the CPU for integer determinism. `elysian-cli slice` and the UI **Slice** button use
Vulkan compute when an adapter is present and fall back to CPU otherwise
(`--cpu` / `--gpu` to force). Layering follows Bambu / PrusaSlicer 3.0
`generate_object_layers`: contours at mid-slab `slice_z`, G-code at `print_z`, first layer height from `initial_layer_print_height`. Bambu `precise_z_height` (`--precise-z`) retunes the last five slabs to the object top. First-layer `elefant_foot_compensation` insets layer 0 (`--elephant-foot`).

## Build

Requires current **stable** Rust (`rust-toolchain.toml` tracks `stable`) and **Git LFS**
(`git lfs install` before clone or pull; test `.3mf` fixtures are LFS objects). Check out the
mimalloc rewrite and Wild linker as siblings (`../mimalloc`, `../wild`) and build Wild
once (`cargo build --release -p wild-linker` in `../wild`). `cargo` links with Wild via
`.cargo/config.toml`; `elysian-cli` / `elysian-ui` allocate with `mimalloc-core`.

```bash
cargo test --workspace
cargo run -p elysian-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode
cargo run -p elysian-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode --gpu
cargo run -p elysian-cli -- slice model.3mf -o /tmp/model.gcode
cargo run -p elysian-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode --brim 5 --skirt 2 --top 4 --bottom 3
cargo run -p elysian-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode --layer-height 0.2 --first-layer 0.28 --precise-z
# table-like overhangs:
# cargo run -p elysian-cli -- slice overhang.stl -o /tmp/overhang.gcode --support
cargo run -p elysian-ui
```

The UI re-execs with `WGPU_BACKEND=vulkan` on Linux. The top bar is Studio-shaped:
**Prepare** / **Preview** / **Device** / **Filament**, plus **Open**, **Slice**, and **Send last slice**.
Prepare is the left plater (printer, process, filament slots, TabFilament param pages, grouping, Sync AMS, plates, paint). Preview is the
G-code overlay after a slice. Device holds LAN/cloud, Import Studio, **Extract keys**, and the
live monitor. **Filament** is a Spoolman-style inventory (`filament_inventory/inventory.json`:
vendor → filament → spool, remaining weight, locations) with **SpoolmanDB** as the default catalog
(`SPOOLMAN_DB` or sibling `../SpoolmanDB`). Optional Bambu cloud GET/POST/PUT/DELETE `/my/filament/v2`
when a Bearer is present. Extract, slice, mesh load, catalog disk I/O, and
cloud spool I/O run off the iced thread so orbit/scroll stay live.

Prepare’s filament library is project slots (add/remove, inventory/catalog picks, colour), not the
inventory tab. User presets are `$XDG_CONFIG_HOME/elysian-studio/filament/`
(`"from": "User"`). Studio’s `~/.config/BambuStudio/user/*/filament/` is read-only. **Bambu system
presets** (`instantiation: true` BBL JSON) are an optional picker source (off by default); AMS
sync still resolves `tray_info_idx` against those profiles. Binding a SpoolmanDB SKU overlays
`Generic {material}` when that BBL file exists, then catalog density/diameter/temps/colour.
**Sync AMS** fills slots from MQTT trays (`tray_info_idx` → `filament_id`, else `Generic {type}`).
Dual-nozzle grouping is Flush / Match / Quality / Manual (`filament_map_mode`); cube stays single-filament.
LAN remains the default send path; this workspace still does **not** dlopen `libbambu_networking`.

Load the same Bambu process JSON the C++ app uses (`inherits` is followed in-directory):

```bash
cargo run -p elysian-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode --bbl-0-20
cargo run -p elysian-cli -- slice tests/golden/cube_20mm.stl -o /tmp/cube.gcode \
  --settings /home/luluco/code/BambuStudio/resources/profiles/BBL/process/0.20mm\ Standard\ @BBL\ H2C.json
```

`cargo test -p elysian-cli --test golden_cube` slices the 20 mm cube with that profile in Rust **and** with the upstream `bambu-studio --slice=0` CLI, then compares `CHANGE_LAYER` count, `FEATURE` roles, and the C++ `; CONFIG_BLOCK` values. `cargo test -p elysian-cli --test golden_tower` does the same for `tests/multicolor/Multifilament+advanced+full+test+tower.3mf` (embedded P1P 0.28 mm settings, no `--load-settings` overlay). `golden_remielle`, `golden_eous`, and `golden_calibration` load `tests/remielle` / `tests/eous` / `tests/calibration_block` (H2C lithophane, figurine plates, and a 13-body torture block). Remielle, the Eous figurine, and the calibration-block C++ compares are `#[ignore]` (mesh size, or C++ CLI wipe-tower path conflicts); chassis plate 2 stays in the default suite. The C++ binary is taken from `BAMBU_STUDIO` or `PATH`. Profiles come from `BAMBU_STUDIO_RESOURCES` or `../BambuStudio/resources`. Set `BAMBU_STUDIO_REQUIRE_ORACLE=1` to fail if the C++ CLI is missing.

## Printer network (open-bamboo-networking)

[Option A](https://github.com/ClusterM/open-bamboo-networking#option-a-developer-mode) is Developer Mode LAN (no signing keys). [Option B](https://github.com/ClusterM/open-bamboo-networking#option-b-cloud-mode-without-developer-mode) needs `slicer_cert.pem`, `slicer_key.pem`, and `slicer_crl.pem` from the stock plugin **you already have**. Put them in `$XDG_CONFIG_HOME/elysian-studio/` (never commit them). `keys extract` first scans the on-disk plugin (including Orca `libbambu_networking_*.so`). If that misses, it maps the packed ELF (PT_LOAD / entropy / `bambu_network_*` exports), then spawns the isolated `elysian-vmp-dump` helper so VMProtect can self-decrypt in a **separate process** — `elysian-protocol` still does **not** `dlopen` the plugin. Optional `--dump-elf PATH` keeps that reconstructed image (gitignored); the default is scan-and-delete. If the dump still has no usable PEM/DER, extract launches official `bambu-studio` in a throwaway HOME under `bwrap`, seeds `BambuNetworkEngine.conf` from rewrite cloud tokens, and harvests decrypted PEMs from the child (including `r-x` plugin mappings). Use `--no-unpack` to skip the helper and `--no-live` to skip Studio. `--timeout` (default 90s) is the seeded wait; the same duration is allowed again for interactive Studio login if PEMs never appear.

```bash
cargo run -p elysian-cli -- keys extract
cargo run -p elysian-cli -- keys extract --no-live --plugin /path/to/libbambu_networking.so
cargo run -p elysian-cli -- keys extract --no-unpack --dump-elf /tmp/plugin.vmp.dump
cargo run -p elysian-cli -- keys status
cargo run -p elysian-cli -- device discover --timeout 3
cargo run -p elysian-cli -- device status --host 192.168.1.42 --code 12345678
cargo run -p elysian-cli -- device send cube.gcode --host 192.168.1.42 --code 12345678
cargo run -p elysian-cli -- device gcode --host 192.168.1.42 --code 12345678 --line G28
```

`device status` / `send` use MQTT `:8883` (user `bblp`, password = access code, self-signed TLS) and `send` uploads a `.gcode.3mf` over implicit FTPS `:990` then publishes `project_file`. Serial can be omitted: the MQTT certificate CN is used. `push_status.fun` bit 29 selects Developer Mode vs secured: secured printers get `url_enc`/`param_enc` (device-cert RSA) and optional `app_cert_install` when `slicer_cert.pem` + `slicer_crl.pem` are present. LAN is the default send path. `device pause|resume|stop` publish the same `print.command` JSON as C++ Studio. `device bed --temp` / `nozzle --temp` / `fan --speed` / `ams-load` / `ams-unload` / `hms-resume --err --job` match C++ `MachineObject` MQTT. Optional `--ams 0,1` on `send` sets `ams_mapping`. `--bed-level` / `--timelapse` (and flow/vibration/layer-inspect) default off so LAN cube sends stay uncalibrated.

HMS text comes from MQTT `print.hms` plus a cached catalog from `https://e.bambulab.com/query.php` (`$XDG_CONFIG_HOME/elysian-studio/hms/`). Offline, the UI/CLI show the raw long error code.

`keys import-studio` reads LAN codes and the numeric `user/<id>/` from an existing `~/.config/BambuStudio` data dir, copies any cloud tokens if a `BambuNetworkEngine.conf` is present, and always extracts `slicer_*.pem` into `$XDG_CONFIG_HOME/elysian-studio/`. Optional **cloud MQTT / HTTPS upload** uses `cloud_user`, `cloud_token`, `cloud_region`, and `cloud_serial` in that rewrite config dir (OpenBambuAPI / Home Assistant style). `device send --cloud file.gcode` packs a `.gcode.3mf`, uploads it, then publishes `project_file` with an `https://` URL. This workspace still does **not** dlopen `libbambu_networking` and does **not** ship PEMs.

```bash
cargo run -p elysian-cli -- keys import-studio
cargo run -p elysian-cli -- device devices
cargo run -p elysian-cli -- device send cube.gcode --cloud
cargo run -p elysian-cli -- device pause --host 192.168.1.42 --code 12345678
cargo run -p elysian-cli -- device bed --host 192.168.1.42 --code 12345678 --temp 65
cargo run -p elysian-cli -- device hms-resume --host 192.168.1.42 --code 12345678 --err 0700010000010001 --job 123456
cargo run -p elysian-cli -- device hms --host 192.168.1.42 --code 12345678 --refresh
cargo run -p elysian-cli -- device cloud-status
```

```bash
cargo run -p elysian-cli -- device camera --host 192.168.1.42 --code 12345678 --output /tmp/chamber.jpg
cargo run -p elysian-cli -- device install-cert --host 192.168.1.42 --code 12345678
```

P1/A1 chamber JPEG is TLS `:6000`. X1/H2 use RTSPS `:322` (not this snapshot path). The UI **Chamber snapshot** button reports frame size after the same JPEG grab.

The UI **Device** tab runs **Import Studio** / **Extract keys** / **Discover printers**; **Send last slice** is on the top bar (same LAN FTPS default, or **Cloud upload** when a Bearer is present). The **Filament** tab is local Spoolman-like inventory (catalog add, remaining/use/measure) plus optional cloud pull/push. Extract and slice no longer freeze the window. C++ CLI leftovers such as `result.json` are gitignored.

Nix:

```bash
nix build .#elysian-cli
nix build .#elysian-studio
```

The flake takes `git+file` inputs for `../mimalloc` and `../wild` so Nix builds
link with that Wild and compile against that `mimalloc-core`.
