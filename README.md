# bevy_dlss_bench

Screenshot harness for the Bevy rendering features that break when DLSS renders the main
pass at a lower resolution (`MainPassResolutionOverride`) while other passes still use the
full-size `view.viewport`.

Each feature gets one small, deterministic scene (fixed camera, fixed seeds, no animation
except the TAA scene's frame-driven fence). That scene is captured as **native**, **DLSS
Quality**, **DLSS Performance** and, for the deferred scenes, **DLSS + forward**. The TAA
scene runs with TAA on and adds DLSS without TAA as its control. Every
image has a Bevy UI label, and the variants are then composed into comparison sheets.
This repo is meant as the evidence for a Bevy issue or PR.

Pinned versions: Bevy **0.19.1** (feature `dlss`), dlss_wgpu **4.0.0**, NVIDIA DLSS SDK
**v310.5.3**, wgpu 29. Developed for NixOS + RTX 4070 Super + Vulkan.

## Running

```sh
nix-shell          # libclang, vulkan-headers, /run/opengl-driver/lib, fetches vendor/DLSS
just shots         # all scenes x all variants -> out/
just shots --scene ssao,skybox --mode quality,ultra-performance --res 2560x1440
just list          # scenes, features, variants
just view          # interactive viewer (keys below)
just sheets        # re-compose sheets from existing captures
just shots-native  # any GPU/OS, built without the dlss feature (DLSS tiles say "unsupported")
```

### Interactive viewer

To step through a bug by hand, or check a fix against a patched Bevy, open the same scenes in a
window:

```sh
just view                                              # first scene, native
just view --scene ssao --variants dlss-performance     # start on a given scene / variant
```

| key | action |
|---|---|
| `1`-`7`, `[` `]` | jump to / previous / next scene |
| `Z` | cycle DLSS: off -> Quality -> Balanced -> Performance -> Ultra Performance -> DLAA (scene 7 always has TAA on, so this shows TAA + DLSS) |
| `R` | reset DLSS / TAA history |
| `C` | snap the camera back to the scene's fixed pose |
| right-drag + `WASD` / `QE` (`Shift` = fast) | fly (Bevy `FreeCamera`) |
| `H` | hide / show the label |
| `P` | screenshot to `out/view/<scene>/<variant>-NNN.png` (label included unless hidden) |
| `Esc` | quit |

Each switch rebuilds the scene, because the deferred/forward choice is baked into the
materials. The camera stays where you put it, so you can flip between `native` and
`dlss-*` from exactly the same view. Switching scene resets the camera. The label is the same
as in the captures and shows the live main-pass resolution read from the render world. `--res`
sets the window's physical size (scale factor forced to 1).

CLI (`bevy_dlss_bench --help`):

| flag | default | meaning |
|---|---|---|
| `--scene <name\|all>` | `all` | comma-separated scene names (see below) |
| `--variants <list\|all>` | `all` | variant names (`dlss-quality-forward`) or kinds: `native`, `taa`, `dlss`, `forward`, `dlss-taa` |
| `--mode <list>` | `quality,performance` | DLSS modes: `dlaa`, `quality`, `balanced`, `performance`, `ultra-performance`. The first mode is also used for the `-forward` variant and the TAA scene's no-TAA control |
| `--res WxH` | `1920x1080` | output resolution |
| `--settle N` | `90` | frames rendered after every pipeline is compiled, before capture (DLSS/TAA history) |
| `--out DIR` | `out` | output directory |
| `--tile-width N` | `960` | max tile width in the per-scene sheets |
| `--view` | | interactive viewer (see above) |
| `--no-sheets` / `--sheets-only` / `--list` | | |

Output:

```
out/
  index.png                     every scene x every variant (unlabelled thumbnails)
  SUMMARY.md                    status, render -> output resolution, pipeline, expected artefact
  <scene>/sheet.png             labelled variants side by side + header
  <scene>/<variant>.png         labelled capture (Bevy UI overlay)
  <scene>/raw/<variant>.png     the same frame without the label (for diffing)
  <scene>/<variant>.meta        what the capture actually did (status, real render resolution, adapter, features)
```

### How a capture works

* Each (scene, variant) runs as a separate headless Bevy app: the orchestrator re-executes
  itself with `--capture scene:variant`. This is needed because the render device and the DLSS
  context cannot be rebuilt inside one process. A crash or panic therefore only loses one tile.
* The 3D camera renders into an offscreen `Image`. **All variants**, native included, use
  `Hdr`, `Msaa::Off`, `DepthPrepass` and `MotionVectorPrepass`, which DLSS requires. That way
  DLSS is the only difference between `native` and `dlss-*`.
* A second, UI-only camera (order 1) draws that image full-size plus the label into a
  second offscreen image. Both images are captured **in the same frame**, so `raw/` and the
  labelled PNG are pixel-identical apart from the label.
* Frames are counted only once the render world's `PipelineCache` has no pending
  pipelines. The capture then waits `--settle` more frames.
* The label shows the **actual** main-pass resolution. It is read back from the render
  world (`MainPassResolutionOverride` when present, else `view.viewport`), not computed from
  the nominal mode scale. A DLSS variant without an override on its camera is reported as
  `dlss-inactive`.
* No DLSS (other vendors, macOS/Metal, missing NGX runtime, or a build with
  `--no-default-features`): the first DLSS capture prints the reason and exits with code 3.
  The remaining DLSS variants are skipped, and their sheet tiles show the reason. Native
  shots still work.

### DLSS SDK / Nix details

* `scripts/fetch-dlss-sdk.sh` makes a sparse clone of tag `v310.5.3` of
  [NVIDIA/DLSS](https://github.com/NVIDIA/DLSS/tree/v310.5.3) into `vendor/DLSS`. It takes the
  headers, `libnvsdk_ngx.a`, `libnvidia-ngx-dlss.so.310.5.3`, the license and the programming
  guide (about 90 MB). It is idempotent and runs from the `shellHook` and from `just shots`.
  Fetching it means accepting the
  [DLSS SDK license](https://github.com/NVIDIA/DLSS/blob/v310.5.3/LICENSE.txt).
* `.cargo/config.toml` sets `DLSS_SDK = vendor/DLSS` (relative to the project) through `[env]`.
* `shell.nix` sets `VULKAN_SDK=${vulkan-headers}` (dlss_wgpu reads `$VULKAN_SDK/include`) and
  `LIBCLANG_PATH` (bindgen). It also prepends `/run/opengl-driver/lib`, which holds the NVIDIA
  Vulkan ICD and the NGX core, plus the vulkan-loader to `LD_LIBRARY_PATH`.
* `just shots` copies `libnvidia-ngx-dlss.so.310.5.3` next to the executable, where NGX looks
  for it.

## Scenes and the bugs they isolate

All links point to tag `v0.19.1`.

### Background: what `MainPassResolutionOverride` changes

DLSS's [`prepare_dlss`](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_anti_alias/src/dlss/prepare.rs#L70-L111)
uses `view.viewport.zw()` as the upscaled (output) size and inserts
`MainPassResolutionOverride(render_resolution)` on the render-world camera. Then:

* The prepass, deferred-gbuffer and main opaque passes set a smaller GPU viewport via
  [`Viewport::from_viewport_and_override`](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_camera/src/camera.rs#L118-L129)
  ([opaque](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_core_pipeline/src/core_3d/main_opaque_pass_3d_node.rs#L77-L80),
  [prepass](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_core_pipeline/src/prepass/node.rs#L201),
  [deferred](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_core_pipeline/src/deferred/node.rs#L220)).
  So the scene is drawn into the **top-left `render_resolution` rectangle** of full-size textures.
* The view uniform gets
  [`main_pass_viewport`](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_render/src/view/mod.rs#L1035-L1045)
  alongside the unchanged `viewport`. The
  [doc comment](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_camera/src/camera.rs#L132-L144)
  says: *"In shaders, use `View::main_pass_viewport` instead of `View::viewport`"*.
  Outside Solari and the atmosphere, almost nothing does yet.

Everything that maps a pixel of the main pass back to UV, NDC or world space through
`view.viewport`, or that sizes its work from `camera.physical_viewport_size`, is therefore off
by the factor `render / output`: 0.667 for Quality, 0.5 for Performance.

The core helper used by most of the bugs below is
[`frag_coord_to_uv` / `frag_coord_to_ndc` / `ndc_to_frag_coord`](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/render/view_transformations.wgsl#L225-L238),
which divides by `view.viewport.zw`.

### 1. `deferred_lighting`: deferred lighting pass

* **Scene:** checker floor, boxes, a sphere, a thin pole and a back wall. One directional
  light with shadows (1 cascade, 4096²) and low ambient light.
* **Bug:** `pbr_input_from_deferred_gbuffer`
  [rebuilds the world position](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/deferred/pbr_deferred_functions.wgsl#L123)
  with `position_ndc_to_world(frag_coord_to_ndc(frag_coord))`, which uses the full viewport.
  On top of that, the [deferred lighting draw](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/deferred/mod.rs#L121-L181)
  sets no viewport at all.
* **Expected under DLSS:** every pixel is lit as if it were at a world position scaled toward
  the top-left of the screen. Shadows slide and stretch relative to their casters, and N·L and
  the specular term do not match the geometry.
* **`dlss-quality-forward`:** forward shading uses the interpolated `world_position`, so
  this bug should disappear.

### 2. `clustered_lights`: clustered point lights (forward)

* **Scene:** a 9×6 grid of point lights (no shadows, colours from a fixed seed) above a
  plain plane. Each light has an unlit marker sphere. Rendered forward so that deferred
  reconstruction does not mix in.
* **Bug:** clustering on the CPU uses the full viewport.
  [`assign_objects_to_clusters`](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_light/src/cluster/assign.rs#L304-L376)
  takes `camera.physical_viewport_size()`, and
  [`cluster_factors.xy`](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/render/light.rs#L1758-L1759)
  divides by `extracted_view.viewport.zw`. In the shader,
  [`view_fragment_cluster_index`](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/render/clustered_forward.wgsl#L56-L57)
  turns a main-pass `frag_coord` into a tile with those factors. A pixel therefore reads the
  light list of a tile closer to the top-left than the one it really sits in.
* **Expected under DLSS:** light pools are cut along tile edges, are missing, or are offset
  from their marker spheres. This is the same in forward and deferred.

### 3. `ssr`: screen-space reflections

* **Scene:** a glossy floor (`perceptual_roughness` 0.15, inside SSR's default 0.12..0.55 active range) with five coloured boxes and
  spheres. One unshadowed directional light. Deferred, because Bevy's SSR requires it.
* **Bug:** the SSR fragment shader
  [rebuilds the ray origin](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/ssr/ssr.wgsl#L164)
  with `frag_coord_to_ndc`. The raymarch converts between NDC and depth-texture UVs over the
  full texture, and the [SSR pass](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/ssr/mod.rs#L245-L317)
  sets no viewport.
* **Expected under DLSS:** reflections are offset or scaled relative to the objects, smeared,
  or missing. The deferred-lighting artefacts from scene 1 are also present. There is no
  forward variant because SSR does not exist in forward.

### 4. `ssao`: screen-space ambient occlusion (forward)

* **Scene:** a room corner (floor and two walls) with boxes pushed into the corner and
  against the walls, a stack, and a sphere on the floor. Strong ambient light so that only AO
  shapes the image.
* **Bug:** the [SSAO node](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/ssao/mod.rs#L166-L250)
  dispatches all three passes over `camera.physical_viewport_size`, and its
  [textures](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/ssao/mod.rs#L523-L526)
  are sized from it. The shaders compute UVs as `pixel / view.viewport.zw`
  ([ssao.wgsl#L50](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/ssao/ssao.wgsl#L50),
  [#L144](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/ssao/ssao.wgsl#L144),
  [preprocess_depth.wgsl#L60](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/ssao/preprocess_depth.wgsl#L60),
  [spatial_denoise.wgsl#L29](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/ssao/spatial_denoise.wgsl#L29)).
  The result is that SSAO samples the whole depth texture, where only the top-left rectangle
  is valid, and the main pass then reads the AO texture at its own (smaller) pixel coordinates.
* **Expected under DLSS:** contact occlusion is magnified or offset and no longer hugs the
  creases, with dark smears from depth outside the rendered rectangle.
* **Related:** [#25319](https://github.com/bevyengine/bevy/pull/25319) fixes the same class
  of problem for cameras with `Some(Viewport)`, using the viewport offset. Also
  [#25338](https://github.com/bevyengine/bevy/pull/25338).

### 5. `fog`: distance fog in the deferred path

* **Scene:** a long corridor of 2×16 pillars with linear `DistanceFog` from 2 to 28 m.
  Unshadowed directional light.
* **Bug:** in deferred,
  [`main_pass_post_lighting_processing` → `apply_fog`](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/render/pbr_functions.wgsl#L995-L1004)
  runs in the [deferred lighting shader](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/deferred/deferred_lighting.wgsl#L84)
  with `pbr_input.world_position`, which is the wrongly reconstructed position from scene 1.
* **Expected under DLSS:** the fog stops following depth. Near pillars get fogged, far ones
  look too clear, and the gradient bends toward the top-left.
* **`dlss-quality-forward`:** the fog should match native.
* **Note:** the atmosphere had the same problem and was fixed by
  [#22013](https://github.com/bevyengine/bevy/pull/22013) (issue
  [#21973](https://github.com/bevyengine/bevy/issues/21973), regression
  [#23252](https://github.com/bevyengine/bevy/issues/23252)). That fix is the model for the
  others.

### 6. `skybox`: control, already fixed upstream

* **Scene:** a procedural cubemap with one colour per face, an 8×8 grid, a centre cross and
  white face borders. The camera looks at the −X/−Z cube edge, where a red post on a ring of
  white posts marks the seam.
* **Bug:** [`skybox_fragment`](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_core_pipeline/src/skybox/skybox.wgsl#L76)
  builds its ray from `view.viewport`.
* **Expected under DLSS:** the sky is zoomed and offset against the geometry, so the seam
  leaves the red post.
* **Fix:** [#25604 "Build the skybox ray from the main pass viewport"](https://github.com/bevyengine/bevy/pull/25604),
  merged for 0.20. The scene shows that the harness catches a known bug, and it can be re-run
  on 0.20 as a passing control.

### 7. `taa`: TAA + DLSS together (double jitter)

* **Scene:** a fence of 6 mm wires in front of a high-frequency checker wall and floor
  (nearest sampling, no mips). The fence slides sideways with
  `x = 0.35·sin(0.045·frame)`, driven by the frame counter so it is deterministic.
  TAA is on by default in this scene (and only here). Variants: `native-taa` (TAA-only
  reference), `dlss-quality-taa`, `dlss-performance-taa`, and `dlss-quality` without TAA as
  the control.
* **Bug:** both [`prepare_taa_jitter`](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_anti_alias/src/taa/mod.rs#L353-L382)
  (8-tap Halton) and `prepare_dlss`
  ([#L86](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_anti_alias/src/dlss/prepare.rs#L86),
  [#L101](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_anti_alias/src/dlss/prepare.rs#L101))
  write `TemporalJitter.offset` in `RenderSystems::PrepareViews`, with no ordering between
  them. Both the
  [TAA node](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_anti_alias/src/taa/mod.rs#L72)
  and the [DLSS node](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_anti_alias/src/dlss/mod.rs#L193-L196)
  are in `Core3dSystems::EarlyPostProcess`, again unordered. So TAA either resolves the
  low-resolution rectangle as if it were the full frame, or re-accumulates the already
  upscaled image with low-resolution motion vectors. Nothing rejects the combination.
* **Expected:** wires shimmer, ghost or blur in `dlss-*-taa` compared with the `dlss-quality`
  control. `native-taa` is the TAA reference.

## Suggested upstream fix (basis for a PR)

1. Use `main_pass_viewport` in
   `bevy_pbr::view_transformations::{frag_coord_to_uv, frag_coord_to_ndc, ndc_to_frag_coord}`.
   These run on main-pass fragments; any pass that runs at output resolution should use
   `bevy_render::view` with an explicit viewport. This fixes deferred lighting, fog-in-deferred,
   SSR and volumetric fog in one go. The deferred lighting and SSR passes also need
   `set_camera_viewport(Viewport::from_viewport_and_override(..))` so they only shade the
   rendered rectangle.
2. Clustering: size the tiles from the main-pass size, using
   `MainPassResolutionOverride` / `main_pass_viewport` in `assign_objects_to_clusters` and
   `cluster_factors`.
3. SSAO: dispatch over the main-pass size and allocate the AO textures at that size, then
   divide by `main_pass_viewport.zw` in the three shaders. Builds on #25319.
4. Skybox: done in #25604.
5. TAA + DLSS: make them mutually exclusive (warn and remove `TemporalAntiAliasing` when `Dlss`
   is present), or order `prepare_dlss` after `prepare_taa_jitter` and skip the TAA node for
   DLSS cameras.
6. Others found while auditing, not covered by a scene:
   [transmission](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/transmission/transmission.wgsl#L37-L38)
   and [OIT resolve](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_core_pipeline/src/oit/resolve/oit_resolve.wgsl#L32)
   use `view.viewport`. [volumetric fog](https://github.com/bevyengine/bevy/blob/v0.19.1/crates/bevy_pbr/src/volumetric_fog/volumetric_fog.wgsl#L139-L200)
   uses `frag_coord_to_ndc`.

Related upstream: [#19817](https://github.com/bevyengine/bevy/pull/19817) (resolution
override), [#20403](https://github.com/bevyengine/bevy/pull/20403) (`main_pass_viewport`),
[#19864](https://github.com/bevyengine/bevy/pull/19864) (initial DLSS),
[#22240](https://github.com/bevyengine/bevy/pull/22240) (Solari viewport under DLSS),
[#21530](https://github.com/bevyengine/bevy/issues/21530) (render scaling).

## Caveats

* The "expected" captions come from reading the source; the screenshots are the evidence.
  Check every claim against `out/` before filing.
* DLSS output is not bit-exact across drivers. Compare against `native` from the same
  machine and run.
* NVIDIA documents [Vulkan validation errors](https://forums.developer.nvidia.com/t/validation-errors-using-dlss-vulkan-sdk-due-to-vkcmdclearcolorimage/326493)
  from DLSS itself. They are harmless.
