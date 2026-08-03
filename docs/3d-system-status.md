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
| Backend-neutral 3D preparation | Partial | `render3d` feeds software, Vulkan, and Web paths, but native Vulkan still receives CPU-projected non-indexed triangles. |
| Cross-platform 3D behavior | Partial | Software fallback is deterministic and Vulkan/Web paths exist; ordinary Web meshes still use software compositing and separate WebGL mesh commands do not share one depth surface. |
| Major subsystem documentation | Partial | Runtime foundations are documented in `README.md` and `docs.md`; the remaining systems below require architecture and user documentation as they land. |

## Rendering

| Requirement | Status | Current evidence / remaining work |
| --- | --- | --- |
| Perspective and orthographic cameras | Implemented | `Camera3D`, projection matrices, activation pre-pass, editor schema, and projection tests. |
| Static meshes and primitives | Implemented | Revisioned `MeshHandle`, OBJ/glTF/GLB/FBX import, six cached primitives, software/Vulkan/Web presentation. |
| Custom 3D shaders | Partial | `MeshRenderer3D.shader`, explicit 3D fragment constructors/capability queries, portable Vulkan/WebGL source normalization, uniform limits, lazy pipeline caches, and web projected-depth testing work. The stage is fragment-only, native software cannot interpret GLSL, extra named web textures and cross-command WebGL depth remain. |
| Configurable 3D antialiasing | Implemented | Global off/standard/high modes select lazy depth/luminance software smoothing before 2D overlays, Vulkan 1×/2×/4× MSAA by device support, and WebGL off/browser-MSAA/bounded 2× supersampling. Focused software/Vulkan tests and an Emscripten compile check cover the paths. |
| Skinned meshes | Partial | glTF and ASCII-FBX armatures deform on CPU with independent component pose copies; GPU skinning, multi-skin assets, morph targets, and full FBX coverage remain. |
| Directional, point, and spot lights | Partial | Diffuse lighting and editor proxies exist through `Light3D`; dedicated component ergonomics, physically based evaluation, and shadow integration remain. |
| PBR metallic/roughness and normal mapping | Partial | Importers preserve glTF factors and texture metadata, but the current renderer evaluates only diffuse vertex lighting and a single explicit base texture. |
| Automatic imported materials | Partial | Material factors/submeshes are generated; referenced images are not yet resolved into per-material runtime bindings. |
| Shadow mapping | Missing | `casts_shadows`, `receives_shadows`, and bias are authoring metadata only for 3D. |
| Skyboxes | Partial | Solid, gradient, and equirectangular environments work; cubemap import/rendering and image-based lighting remain. |
| HDR rendering and tonemapping | Partial | Exposure/Reinhard/ACES effects operate on the byte framebuffer; a linear floating-point HDR target and native GPU post-process path remain. |
| Bloom | Partial | Deterministic software post-process bloom exists; native GPU bloom remains. |
| Ambient occlusion and fog | Missing | Existing AO is the 2D light-map effect, not 3D SSAO; no 3D fog volume/environment evaluation exists. |
| GPU instancing | Missing | Vulkan batches compatible projected vertices but issues non-instanced draws. |
| Static and dynamic batching | Partial | Compatible 2D/3D vertices are coalesced by texture/shader on Vulkan and particles batch per emitter; persistent static batches and a backend-neutral dynamic batcher remain. |
| Frustum culling | Implemented | Conservative mesh-bounds rejection precedes vertex preparation; triangles are clipped against the full homogeneous frustum. |
| Distance and hardware occlusion culling | Missing | No renderer-level distance policy, query pool, or hierarchical depth culling exists. |
| Automatic LOD | Missing | No runtime `LODGroup` selection or editor authoring workflow exists. |
| Texture and mesh streaming | Missing | Asset loads are synchronous and whole-resource; no background residency/budget manager exists. |
| GPU resource lifetime management | Partial | Revisioned image caches and bounded idle eviction exist; meshes are uploaded as transient expanded vertex buffers every frame. |

## Components

| Requirement group | Status | Current evidence / remaining work |
| --- | --- | --- |
| Transform, mesh renderer, camera, lights, sky/environment, particles | Partial | Generic production components exist; dedicated `SkinnedMeshRenderer`, light variants, visibility/layer/tag, probes, and LOD components remain. |
| Rigidbody and primitive/mesh colliders | Partial | `Rigidbody3D` and shape-selectable `Collider3D` exist; dedicated collider components and shared `PhysicsMaterial` assets remain. |
| Character controller and trigger volume | Partial | Trigger behavior exists via `Collider3D.is_trigger`; no swept character controller or dedicated trigger authoring component exists. |
| Animation player/state machine | Partial | Imported clips play on `MeshRenderer3D`; no general 3D animation player/state machine component exists. |
| Audio source/listener | Missing | `SpatialSound2D` is real 2D positional audio; 3D listener/source transforms and attenuation are not implemented. |
| Terrain | Missing | No terrain component or runtime terrain renderer exists. |
| Reflection probe | Missing | No capture, filtering, or probe selection exists. |
| Script, prefab, tag/layer/visibility | Partial | Scripts and prefabs are implemented; collision layer fields exist on colliders, but reusable tag/layer/visibility components do not. |

## Physics, animation, terrain, and audio

| Requirement | Status | Current evidence / remaining work |
| --- | --- | --- |
| Rigid bodies, collision detection, raycasts, filters, triggers, materials | Partial | Exact primitive contacts, mesh BVH raycasts, broadphase, callbacks, friction, and restitution work. Mesh contacts, multi-body impulses, CCD, and reusable material assets remain. |
| Character controller | Missing | Requires capsule/shape sweeps, slope/step handling, grounding, and moving-platform behavior. |
| Skeletal animation | Partial | CPU pose sampling and skinning work for supported imports. Parallel evaluation and GPU palette skinning remain. |
| Blending, blend trees, events, root motion | Missing | No mixer graph, event cursor, or root-motion extraction/application exists. |
| Terrain rendering, heightmaps, painting, foliage, materials | Missing | Runtime, importer, editor tools, streaming chunks, and tests all remain. |
| Positional audio, attenuation, listener | Partial | 2D spatial attenuation exists; native 3D transforms and listener selection remain. |

## Importing and assets

| Requirement | Status | Current evidence / remaining work |
| --- | --- | --- |
| OBJ | Partial | Geometry, groups, normals/UVs, and material names import; MTL loading remains. |
| glTF/GLB | Partial | Geometry, PBR metadata, external/embedded buffers, one flattened skin, and LINEAR/STEP animation import. Sparse/compressed accessors, CUBICSPLINE, morphs, multi-skin scenes, node instancing, and automatic image bindings remain. |
| FBX | Partial | ASCII/binary geometry and an ASCII skin/curve subset import. Binary materials/skin/animation and broader mapping modes remain. |
| PNG, JPEG, HDR | Implemented | The image asset loader decodes these formats through the shared image crate configuration. |
| Asynchronous loading and streaming | Missing | Asset manager caches identities, but decode/import is synchronous and has no residency budgets. |

## Visual editor

| Requirement | Status | Current evidence / remaining work |
| --- | --- | --- |
| Scene view, hierarchy, inspector, asset browser | Implemented | Dockable/detachable core editor panels, 2D/3D viewports, component inspectors, hierarchy editing, and project browser exist. |
| Game view, console, animation editor, material inspector, profiler, project settings | Partial | Run preview, logger, `.neoanim` editor, scene lighting/post effects, and project settings dialogs exist, but these are not yet a complete dedicated panel set and there is no PBR material or integrated performance profiler workflow. |
| Layouts, multi-select, drag/drop, prefabs, undo/redo, search | Partial | Most core workflows exist; named saved layouts, crash recovery, command palette, asset search, and complete multi-object property editing remain. |
| 3D manipulation and view modes | Partial | Local hierarchy transforms, X/Y/Z gizmos, grid snap, mesh picking, and fly camera work. Surface snap, explicit local/world modes, wireframe, bookmarks, locking, and statistics overlay remain. |
| Play, pause, and single-frame step | Partial | Run preview exists; a complete embedded play-state lifecycle with pause and deterministic stepping remains. |
| Rebindable shortcut set | Missing | Several fixed shortcuts exist, but there is no command registry, conflict validation, persistence, or rebinding UI. |

## Debugging, samples, and validation

| Requirement | Status | Current evidence / remaining work |
| --- | --- | --- |
| Render/CPU/GPU/memory statistics and graphs | Missing | Opt-in microbenchmarks exist, but no integrated frame profiler or durable frame statistics stream exists. |
| Physics/bounds/collision/light/frustum debug drawing | Partial | Editor camera/light/collider proxies exist; runtime toggles and complete debug visualization modes remain. |
| Focused sample projects under `~/Documents/apps/samples` | Partial | `dodge-3d` and the focused `3d-shaders-aa` project run; the latter demonstrates capability fallback, portable material uniforms, and live AA switching. The rest of the required focused suite is not present. |
| Automated editor/project/render/physics/animation validation | Missing | Unit tests cover current foundations; no end-to-end validation runner exercises every required workflow. |
| Organized build/runtime/render/performance/import/physics/animation/error logs | Partial | `test-artifacts/logs/3d-shaders-aa-validation.md` records commands, counts, runtime checks, and a benchmark for this milestone; comprehensive subsystem reports remain. |
| Automated screenshots and visual regression | Partial | A Spectacle-captured Vulkan shader/AA frame is stored under `test-artifacts/screenshots`; repeatable multi-backend capture, pixel tolerances, baselines, and CI comparison remain. |

## Dependency-ordered milestones

1. Complete automatic imported material/image binding and a backend-neutral PBR
   material representation.
2. Move native mesh transforms, indexed geometry, materials, and skin palettes
   to persistent GPU resources; add instancing and resource budgets.
3. Add linear HDR targets, GPU PBR/normal mapping, shadow maps, IBL, GPU
   post-processing, fog, and 3D AO.
4. Add distance/occlusion culling, `LODGroup`, static/dynamic batching policy,
   asynchronous loading, and texture/mesh streaming.
5. Complete rigid-body response, character control, physics materials, and
   dedicated collider/trigger components.
6. Add animation mixing/state machines/events/root motion and parallel pose
   evaluation.
7. Implement terrain, foliage, reflection probes, and 3D audio listener/source
   systems.
8. Complete editor panels, play-state lifecycle, saved layouts, command palette,
   rebindable shortcuts, profiler, debugging views, and material/terrain tools.
9. Build every focused sample and the automated logging, screenshot, benchmark,
   and visual-regression validation suite.

Every milestone must preserve the 2D regression suite and add focused evidence
for the new runtime, editor, import, backend, and failure paths it introduces.
