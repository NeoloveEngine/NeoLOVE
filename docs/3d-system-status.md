# Native 3D system implementation status

This document is the implementation checklist for NeoLOVE's native 3D work. It
is intentionally evidence-based: a feature is marked complete only when the
runtime implementation, public Luau surface, editor authoring path, docs, and
focused validation needed by that feature are present. Passing a narrow unit
test is not treated as proof of an entire subsystem.

Status meanings:

- **Implemented**: usable end-to-end in the current engine, with focused tests.
- **Partial**: real functionality exists, but one or more required production
  paths or authoring/validation surfaces remain.
- **Missing**: no production implementation exists yet.

## Compatibility and architecture

| Requirement | Status | Current evidence / remaining work |
| --- | --- | --- |
| Existing 2D projects run unchanged | Implemented | Legacy projects default to `kind = "2d"`; 2D transforms, rendering, editor documents, and tests remain intact. |
| Component-based scene graph and transform hierarchy | Implemented | Entities retain parent/child component tables; 3D world TRS is composed down the hierarchy and covered by runtime/editor tests. |
| Backend-neutral 3D preparation | Partial | `render3d` feeds software, Vulkan, and Web paths. Default Vulkan meshes retain indexed geometry, perform transforms/PBR lighting in shaders, and skin supported armatures from per-draw joint palettes. Custom mesh shaders, software/Web presentation, particles, and the cross-backend skinned snapshot still use CPU preparation. |
| Cross-platform 3D behavior | Partial | Software fallback is deterministic and Vulkan/Web paths exist; ordinary Web meshes still use software compositing and separate WebGL mesh commands do not share one depth surface. |
| Major subsystem documentation | Partial | Runtime foundations are documented in `README.md` and `docs.md`; the remaining systems below require architecture and user documentation as they land. |

## Rendering

| Requirement | Status | Current evidence / remaining work |
| --- | --- | --- |
| Perspective and orthographic cameras | Implemented | `Camera3D`, projection matrices, activation pre-pass, editor schema, and projection tests. |
| Static meshes and primitives | Implemented | Revisioned `MeshHandle`, OBJ/glTF/GLB/FBX import, six cached primitives, software/Vulkan/Web presentation. |
| Custom 3D shaders | Partial | `MeshRenderer3D.shader`, explicit 3D fragment constructors/capability queries, portable Vulkan/WebGL source normalization, uniform limits, lazy pipeline caches, and web projected-depth testing work. The stage is fragment-only, native software cannot interpret GLSL, extra named web textures and cross-command WebGL depth remain. |
| Configurable 3D antialiasing | Implemented | Global off/standard/high modes select lazy depth/luminance software smoothing before 2D overlays, Vulkan 1×/2×/4× MSAA by device support, and WebGL off/browser-MSAA/bounded 2× supersampling. Focused software/Vulkan tests and an Emscripten compile check cover the paths. |
| Skinned meshes | Partial | glTF and ASCII-FBX armatures have independent component poses. Default Vulkan meshes with at most 256 joints deform bind vertices from GPU joint palettes while sharing persistent bind/index buffers; software/Web, custom shaders, larger armatures, and manually edited skinned geometry use the CPU-deformed snapshot. Multi-skin assets, morph targets, and full FBX coverage remain. |
| Directional, point, and spot lights | Partial | `Light3D` feeds direct Cook-Torrance PBR and editor proxies. 3D scene export now explicitly resets the persistent legacy 2D light-map compositor, Scene View never applies its 2D preview to 3D geometry, and the Scene inspector explains the component-driven path with quick add/select controls for `Light3D` and `Environment3D`. The first shadow-enabled directional light (preferred) or spot light drives the native shadow map; point-light cubemaps, multiple simultaneous shadow lights, and dedicated light variants remain. |
| PBR metallic/roughness and normal mapping | Partial | Revisioned reusable `Material3DHandle` assets expose factors/maps/alpha state and live per-slot renderer overrides. Default Vulkan and software/ordinary Web meshes evaluate tangent-space normal maps, G/B roughness/metallic maps and factors, emissive maps/factors, alpha masking, two-sided normals, a Cook-Torrance direct-light BRDF, and bounded panorama/cubemap diffuse/specular IBL. Authored local reflection probes priority-select and edge-blend over the global environment. Native Vulkan retains unclamped PBR radiance in its linear RGBA16F target. UV1, convolved/prefiltered IBL, clearcoat/transmission extensions, and custom-shader conveniences remain. |
| Automatic imported materials | Partial | glTF/GLB external, data-URI, and buffer-view images, OBJ/MTL libraries/maps, and ASCII/binary FBX common factors/external maps are retained as per-material bindings. ByPolygon/AllSame submeshes render in software/ordinary Web and Vulkan unless explicitly overridden. Versioned `.neomaterial` files cache shared identities, resolve relative maps, round-trip through public APIs, and can be selected in the scene editor. Embedded FBX media, broader FBX mapping modes, and custom-WebGL per-submesh binding remain. |
| Shadow mapping | Partial | Default Vulkan meshes render a persistent 2048×2048 depth pass for the first shadow-enabled directional or spot light and use bounded 3×3 PCF in PBR shading. `casts_shadows`, `receives_shadows`, and per-light `shadow_bias` are live; GPU-skinned/off-camera casters are included. Four directional cascades, point-light cubemaps, multiple shadow lights, alpha-mask silhouettes, configurable resolution/filtering, and software/Web parity remain. |
| Skyboxes | Partial | Solid, gradient, equirectangular, and six-face cubemap environments work. Path- or live-image cubemaps validate equal square faces, propagate face revisions, render as backgrounds, and drive built-in Vulkan/software/ordinary-Web PBR with matching intensity/rotation. Float-HDR uploads, irradiance convolution, prefiltered mip chains, and BRDF LUTs remain. |
| HDR rendering and tonemapping | Partial | Native Vulkan renders and MSAA-resolves the complete scene into a persistent linear RGBA16F target, then presents through a GPU exposure/None/Reinhard/ACES/gamma pass. Clear colors, ordinary images/UI, panoramas, PBR output, light-map multiplication, and portable custom fragments follow explicit linear/display conversions. Software/Web retain their RGBA8 reference path; general native ping-pong effects beyond bloom remain. |
| Bloom | Partial | Deterministic software bloom and native Vulkan HDR bloom are live. Vulkan threshold/downsamples into reusable half-resolution RGBA16F targets, performs bounded separable blur, and adds the result before tone mapping; disabled/zero passes skip all bloom draws. Exact multi-bloom ordering and a Web GPU implementation remain. |
| Ambient occlusion and fog | Partial | `Environment3D` authors linear, exponential, and exponential-squared camera-distance fog plus bounded world-space 3D contact/crease AO. AO transforms mesh bounds, selects the nearest 32 occluders per receiver, obeys `casts_shadows`/`receives_shadows`, and runs per pixel in software/ordinary Web, per fragment in native Vulkan PBR, on projected custom meshes, and in Scene View. Shared sanitization, script/Inspector controls, persistence, focused tests, a sample, and cross-backend captures are present. Mesh-exact/SSAO AO, height fog, and volumetric fog remain. |
| GPU instancing | Partial | Vulkan automatically merges compatible opaque default-shader mesh commands into indexed instance draws with per-instance model, normal, and tint data. Transparent/custom-shader meshes and a packed public instance-batch API remain. The current 10,000-ECS-entity diagnostic reaches 17.77 FPS average/16.07 FPS 1% low, so the 100,000-instance target is not yet met. |
| Static and dynamic batching | Partial | Compatible 2D vertices are coalesced, compatible default Vulkan meshes are automatically instanced, and particles batch per emitter. A backend-neutral batch policy, indirect draws, and a packed instance submission API remain. |
| Frustum culling | Implemented | Conservative mesh-bounds rejection precedes vertex preparation; triangles are clipped against the full homogeneous frustum. |
| Distance and hardware occlusion culling | Partial | `LODGroup3D` provides component-authored active-camera distance culling through the shared Scene/runtime selector. There is no global renderer distance policy, hardware query pool, or hierarchical depth culling yet. |
| Automatic LOD | Implemented | `LODGroup3D` selects three mesh paths and distance-culls from the active camera in the real runtime. Scene View uses the same sanitized thresholds and populated-slot fallback, the Inspector authors every field, and the LOD diagnostic draws current state/ranges without scene mutation. Focused tests cover boundaries, invalid ordering, mesh fallback, runtime draw replacement/culling, static-path bypass, Scene View culling, persistence, and 2D picker isolation. |
| Texture and mesh streaming | Missing | Asset loads are synchronous and whole-resource; no background residency/budget manager exists. |
| GPU resource lifetime management | Partial | Revisioned image caches and persistent device-local indexed Vulkan mesh buffers use bounded idle eviction (512 meshes, 512 MiB, or 600 idle frames). Detached animation poses share a stable bind-geometry cache identity, so palette-only animation does not re-upload vertex/index data. Explicit residency controls, streaming, staging pools, and equivalent resource ownership on other backends remain. |

## Components

| Requirement group | Status | Current evidence / remaining work |
| --- | --- | --- |
| Transform, mesh renderer, camera, lights, sky/environment, particles, LOD | Partial | Generic production components plus dedicated `LODGroup3D`, `Visibility3D`, `RenderLayer3D`, and `ReflectionProbe3D` exist. Camera masks and hierarchy visibility share one Scene/runtime policy. Dedicated `SkinnedMeshRenderer` and light variants remain. |
| Rigidbody and primitive/mesh colliders | Partial | `Rigidbody3D`, shape-selectable `Collider3D`, exact capsule-box contacts, editable collision layer/mask fields, dynamic-body translational CCD, public continuous capsule sweeps, `CharacterController3D`, authorable runtime-backed `Raycast3D`, and shared revisioned `.neophysicsmaterial` assets exist. Shape-specific collider component ergonomics, angular CCD/general mesh manifolds, and the full multi-body solver remain. |
| Character controller and trigger volume | Partial | `CharacterController3D` authors an upright world-unit capsule, continuous primitive/mesh-BVH sweeps, bounded wall/slope sliding, slope limits, validated step-up/headroom/landing, ground snap, optional gravity, moving-platform translation, collision filters/callbacks, and a runtime-matching collider overlay. Dedicated `Trigger3D` shares native geometry/filtering and exposes deterministic enter/stay/exit plus sorted overlap output without response. Arbitrary controller orientation remains. |
| Animation player/state machine | Partial | Imported clips play on `MeshRenderer3D`; no general 3D animation player/state machine component exists. |
| Audio source/listener | Implemented | `AudioSource3D` owns an independent runtime voice, follows hierarchy-resolved world position, and authors live volume/loop/autoplay plus WebAudio-compatible inverse, linear, or exponential min/max-distance attenuation. `AudioListener3D` follows world position/orientation with deterministic first/explicit active selection. Native Rodio and browser WebAudio paths share the public API; editor sound picking, persistence, runtime export, and focused tests cover the workflow without changing `SpatialSound2D`. |
| Terrain | Missing | No terrain component or runtime terrain renderer exists. |
| Reflection probe | Partial | `ReflectionProbe3D` accepts a live cubemap or six persistent image faces, transforms an authored influence size into world bounds, obeys visibility/render masks, selects overlapping probes by priority/interior weight/distance/stable id, and edge-blends local IBL into the global environment on software, ordinary Web, and Vulkan built-in PBR paths. The editor Inspector, Scene lighting quick action, volume diagnostic, runtime tests, sample, and cross-backend captures are present. Runtime scene capture/baking, filtering/prefiltered mips, parallax box projection, oriented-volume/per-pixel selection, and custom-shader bindings remain. |
| Script, prefab, tag/layer/visibility | Implemented | Scripts and prefabs are implemented. Cross-dimensional `Tag` and logical `Layer` components have enabled-aware entity/ECS queries, while 3D `Visibility3D` inheritance and `RenderLayer3D` camera masks suppress all visual contributors through the shared Scene/runtime policy. Compatibility aliases preserve the brief `*3D` metadata API. |

## Physics, animation, terrain, and audio

| Requirement | Status | Current evidence / remaining work |
| --- | --- | --- |
| Rigid bodies, collision detection, raycasts, filters, triggers, materials | Partial | Exact primitive contacts include capsule-box slopes; mesh BVH raycasts, continuous character capsule sweeps, and Rigidbody3D translational CCD all use the shared registry and deterministic filters. Dynamic sphere/capsule mesh hits are triangle-exact; box/mesh and non-uniform-round CCD is conservative and diagnosed as bounds quality. `Raycast3D`, Trigger3D events, and revisioned cached `.neophysicsmaterial` authoring are runtime-backed. Angular CCD, general mesh manifolds, and mass-coupled multi-body impulses remain. |
| Character controller | Implemented | `CharacterController3D` and public `physics3d.sweepCapsule` share continuous upright-capsule casts across exact primitives and mesh triangles. Runtime movement covers iterative sliding, slope classification, steps with head/landing validation, ground snap, gravity, collision callbacks/filters, and translated moving platforms; focused tests cover thin-wall CCD, mesh floors, rotated capsule/box contact, steps, wall tangents, grounding, callbacks, and registry participation. |
| Skeletal animation | Partial | Pose sampling works for supported imports; default Vulkan rendering applies palettes on the GPU for armatures up to 256 joints without per-frame geometry uploads, while a CPU-deformed snapshot preserves bounds and fallback backends. Parallel pose evaluation, blending, and animation-palette instancing remain. |
| Blending, blend trees, events, root motion | Missing | No mixer graph, event cursor, or root-motion extraction/application exists. |
| Terrain rendering, heightmaps, painting, foliage, materials | Missing | Runtime, importer, editor tools, streaming chunks, and tests all remain. |
| Positional audio, attenuation, listener | Implemented | Native and Web runtimes provide independent 3D voices, oriented listener transforms, three sanitized distance models, live movement/settings updates, and explicit listener selection through editor-authored `AudioSource3D`/`AudioListener3D` components. Existing 2D spatial audio remains separate and unchanged. |

## Importing and assets

| Requirement | Status | Current evidence / remaining work |
| --- | --- | --- |
| OBJ | Partial | Geometry, groups, normals/UVs, `mtllib`/`usemtl`, common MTL/PBR factors, opacity, and external base/alpha/normal/roughness/metallic/emissive maps import. Less common vendor extensions and embedded media remain. |
| glTF/GLB | Partial | Geometry, PBR factors/maps, external/embedded buffers and images, automatic per-material bindings, one flattened skin, and LINEAR/STEP animation import. Sparse/compressed accessors, CUBICSPLINE, morphs, multi-skin scenes, node instancing, and material extensions remain. |
| FBX | Partial | ASCII/binary geometry, common material factors, external texture/video links, and ByPolygon/AllSame slots import; ASCII also supports a practical skin/curve subset. Embedded media, binary skin/animation, and broader mapping modes remain. |
| PNG, JPEG, HDR | Implemented | The image asset loader decodes these formats through the shared image crate configuration. |
| Reusable material assets | Implemented | Versioned `.neomaterial` JSON, relative image dependencies, cached revisioned handles, transactional live setters, per-submesh renderer overrides, editor picker/export, strict Luau declarations, and focused round-trip/backend tests are present. |
| Reusable physics material assets | Implemented | Versioned `.neophysicsmaterial` JSON, cached shared identities, transactional live friction/restitution setters, Collider3D binding with inline fallback, editor creation/editing/picker/export, strict Luau declarations, runtime lifecycle coverage, and JSON/binary scene round trips are present. |
| Asynchronous loading and streaming | Missing | Asset manager caches identities, but decode/import is synchronous and has no residency budgets. |

## Visual editor

| Requirement | Status | Current evidence / remaining work |
| --- | --- | --- |
| Scene view, hierarchy, inspector, asset browser | Implemented | Dockable/detachable core editor panels, 2D/3D viewports, component inspectors, hierarchy editing, and project browser exist. |
| Game view, console, animation editor, material inspector, profiler, project settings | Partial | 3D Run presents a streamed real-runtime framebuffer inside Scene/Game tabs. Vulkan builds automatically use the native HDR/tonemap presenter with a dedicated RGBA8 GPU readback pass and resize-safe host buffer, falling back to the real software renderer when Vulkan is unavailable; validation can force either backend. Its integrated live pane lists runtime entities through stable authored source ids, shows recent console lines, resolves structured entity/component/script diagnostics, and exposes state and visual parity reports. The detached logger, `.neoanim` editor, dedicated runtime-validated `.neomaterial` PBR editor, dedicated `.neophysicsmaterial` response editor, scene lighting/post effects, and project settings dialogs also exist. A complete performance profiler remains. |
| Layouts, multi-select, drag/drop, prefabs, undo/redo, search | Partial | 3D click modifiers and empty-space marquee selection now cover projected meshes and component proxies with additive selection, locked/hidden filtering, and per-entity deduplication; group movement and duplicate/delete/parent workflows remain one-command operations. Dirty 3D tabs now receive 30-second project-local recovery snapshots through a synced temp-file/backup rotation; startup/open prompts can recover or explicitly discard them, content fingerprints reject stale snapshots, ordinary saves/reloads/renames/close-and-discard clean them up, and restored work remains dirty and undoable. The recovery path is gated away from 2D documents. Named saved layouts, command palette, asset search, prefab-instance operations, and complete shared/relative multi-object property editing remain. |
| 3D manipulation and view modes | Partial | Existing X/Y/Z move/scale/rotation handles now include explicit Local/World move orientation, XY/XZ/YZ plane handles, camera-facing free movement, independent editable move and rotation snapping, perspective-correct mesh surface/pivot and vertex snapping, box/sphere/capsule/mesh-collider placement within the bounded viewport budget, optional surface-normal alignment, stable multi-parent conversion, locked snap targets, one-command duplicate-and-drag, mesh picking, locking, Alt+LMB orbit, independently invertible horizontal/vertical mouse look, optional hovered WASD/QE fly movement without holding RMB, focus/frame-all, perspective/orthographic switching, Top/Front/Right views, four persisted camera bookmarks, orthographic wheel scaling, a clickable orientation widget, and independently persisted wireframe/normal/tangent/UV-seam/bounds/pivot/axes/collider/rigid-body/trigger/raycast/particle/camera/light/spot/shadow/LOD/render-layer/entity-visibility/stat overlays. Scene View triangles now share a per-pixel depth buffer instead of whole-triangle painter sorting. Local/world scale/rotation semantics, explicit pivot/center group operations, and configurable shortcut bindings remain. |
| Play, pause, and single-frame step | Partial | For 3D scenes, Run serializes the current unsaved document into an isolated project-local cache, launches a hidden real runtime, and losslessly streams software or native-Vulkan framebuffer output into Game View. Native capture repeats only the final tone-map pass into an RGBA8 transfer target, preserving the shipped HDR renderer while avoiding swapchain-format assumptions. Focused input, pause/resume, fixed 1/60-second step, restart, stop, resize, and play-from-selected-Camera3D use the same bidirectional IPC; stop removes staged files and never copies runtime mutations into authored state. Runtime errors carry structured entity/component/script context, and authored links no longer assume allocation ids match. Complete profiler telemetry remains. The established 2D Run path is unchanged. |
| Rebindable shortcut set | Missing | Several fixed shortcuts exist, but there is no command registry, conflict validation, persistence, or rebinding UI. |

## Debugging, samples, and validation

| Requirement | Status | Current evidence / remaining work |
| --- | --- | --- |
| Render/CPU/GPU/memory statistics and graphs | Partial | The Scene View has an opt-in bounded strip for its own CPU time, mesh draws, projected triangles, active lights, and prepared snap surfaces. Embedded Game View reports child-runtime FPS, update/software-render time, submitted draw-command count, and known mesh/2D triangle count per streamed frame. GPU timing, texture/mesh memory, shaders, culling, streaming, graphs, and a durable profiler stream remain. |
| Physics/bounds/collision/light/frustum debug drawing | Partial | Independently persisted editor-only mesh bounds, pivots, world axes, correctly transformed box/sphere/capsule Collider3D and dedicated orange Trigger3D shapes, rigid-body state markers, authored `Raycast3D` lines, conservative particle bounds, camera frustums, point/spot light ranges, circular spot cones, shared-runtime-projection directional/spot shadow frustums, transformed reflection-probe influence volumes, shared-selector LOD ranges/state, render-mask pass/block, hierarchy visibility reasons, wireframe, sampled surface normals and tangents, and discontinuous shared-edge UV seams exist. Navigation/occlusion overlays and runtime debug toggles remain. |
| Focused sample projects under `~/Documents/apps/samples` | Partial | `dodge-3d`, `3d-shaders-aa`, `3d-pbr-materials`, `3d-native-stress`, `3d-gpu-skinning`, `3d-shadows`, `3d-hdr-tonemap`, `3d-ibl`, `3d-cubemap`, `3d-reflection-probes`, `3d-fog`, and `3d-ambient-occlusion` run; they cover gameplay, portable shader/AA behavior, reusable PBR materials, native stress, independent GPU-skinned poses, live directional shadows, linear HDR/ACES presentation, zero-direct-light panorama/cubemap IBL, local probe priority/edge blending, cross-backend distance fog, and AO contact/control parity. The rest of the required focused suite is not present. |
| Automated editor/project/render/physics/animation validation | Partial | Game View retains the real runtime's immutable post-load/pre-update entity snapshot and compares it with the current authored 3D scene. The structured report classifies serialization, hierarchy, transform, component/property, mesh/material/texture/shader, lighting/shadow/environment/camera, physics, animation, particle, and script mismatches and links rows back to the authored Inspector. Game View saves canonical PNGs plus backend sidecars and compares software or native-Vulkan real-runtime frames with a strict same-backend profile or explicit AA-aware cross-backend profile; it writes JSON metrics, a mismatch rectangle, and a highlighted diff PNG on failure. The headless `validate-3d` CLI runs the same isolated runtime/capture/comparator and exits nonzero on runtime or visual failure; its parser and passing/failing artifact paths are tested. Linux CI runs a repository-owned deterministic PBR fixture against software and Mesa/Lavapipe Vulkan. Forced native Vulkan smoke validation proves initial capture and a 320×180→256×144 resize/step cycle. The gate exposed and drove repair of native front-face winding; the corrected PBR comparison has 0.991 mean RGB error. Scene View capture, broader representative fixtures, and the complete end-to-end workflow matrix remain. |
| Organized build/runtime/render/performance/import/physics/animation/error logs | Partial | Focused shader/AA, imported binding, native mesh, GPU skinning, shadows, HDR/tonemapping, bloom, panorama/cubemap IBL, reflection-probe, and PBR material reports under `test-artifacts/logs` record commands, counts, runtime checks, known gaps, and release diagnostics; comprehensive subsystem reports remain. |
| Automated screenshots and visual regression | Partial | Software and Vulkan captures, backend-tagged baselines, strict pixel/mean-error tolerances, structured reports, highlighted diff artifacts, and a nonzero-exit headless CLI gate are implemented. Linux CI enforces the PBR fixture on software and Mesa/Lavapipe Vulkan; a broader scene matrix and Scene View capture remain. |

## Dependency-ordered milestones

1. Complete remaining imported-material edge cases and custom-WebGL
   per-submesh bindings on top of the backend-neutral PBR representation.
2. Move native mesh transforms, indexed geometry, materials, and skin palettes
   to persistent GPU resources; add instancing and resource budgets.
3. Complete convolved/prefiltered IBL, float-HDR uploads, runtime probe capture,
   and parallax correction on top of the shipped panorama/cubemap global IBL,
   local probe selection/blending, linear HDR, GPU PBR/normal mapping,
   shadow-map, bloom, distance fog, and bounded world-space AO foundations;
   then add mesh-exact/SSAO AO and volumetric/height fog.
4. Extend the shipped `LODGroup3D` distance culling with hardware occlusion,
   a broader static/dynamic batching policy, asynchronous loading, and
   texture/mesh streaming.
5. Complete rigid-body response, dynamic-body CCD, general mesh manifolds, and
   mass-coupled constraints on top of the shipped controller, reusable physics
   materials, raycasts, and dedicated trigger authoring.
6. Add animation mixing/state machines/events/root motion and parallel pose
   evaluation.
7. Implement terrain and foliage, and extend the shipped authored reflection
   probes and 3D audio listener/source systems with production capture/
   filtering and world-streaming workflows.
8. Complete editor panels, play-state lifecycle, saved layouts, command palette,
   rebindable shortcuts, profiler, debugging views, and material/terrain tools.
9. Build every focused sample and the automated logging, screenshot, benchmark,
   and visual-regression validation suite.

Every milestone must preserve the 2D regression suite and add focused evidence
for the new runtime, editor, import, backend, and failure paths it introduces.
