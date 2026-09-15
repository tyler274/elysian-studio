---
name: bugfix-regression-tests
description: Add unit, integration, and regression tests whenever fixing a bug
---

# Bugfix regression tests

A fix is incomplete until a test would fail on the old code and pass on the new code.

- Put the test next to the behavior: `bambu-gpu` for viewport/cull/pack, `bambu-model` for plates/volumes, `bambu-protocol` for MQTT/HTTP, GUI goldens only for visible chrome.
- Name tests after the symptom (`worker_pack_without_rt_still_rasters_meshlets`, not `it_works`).
- Cover the integration seam, not only a helper: if load + GPU pack + draw disagreed, assert that pack, not just a bool.
- Do not git-add huge fixtures (`tests/belle/*.3mf`). Reproduce with a tiny mesh, empty plate indices, or an RT instance count.
- Do not edit cube G-code / `default_slice_settings` to make a test pass.

```rust
// BAD: "we looked at the screenshot"
// GOOD:
assert!(
    !blit_raytraced_solids(true, true, packed.rt_instances.len()),
    "off-thread packs omit the model from the TLAS; blit would show an empty bed"
);
```
