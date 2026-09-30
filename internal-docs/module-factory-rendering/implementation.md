# Module factory rendering — implementation

> [Design and API boundary](./design.md)

## API and result

`DevEngine.renderModuleFactories(moduleIds: string[])` returns:

- `code`: a `registerGraph` prelude followed by `registerFactory` registrations,
  with no initialization tail or delivery acknowledgement.
- `sourcemap`: optional source-map JSON when source maps are enabled. It is
  returned separately for every source-map mode; no map asset or
  `sourceMappingURL` is emitted.
- `modules`: `{ id, stableId }` for each requested module, deduplicated in
  first-request order. Dependency IDs in the graph prelude are not extra
  rendered modules.

Use resolved IDs from the engine's module graph, not display/debug paths or
stable runtime IDs. An empty selection returns empty code and metadata, subject
to the same dev-mode and snapshot prerequisites.

```ts
await engine.ensureCurrentBuildFinish();
const output = await engine.renderModuleFactories([moduleId]);
// The caller decides where output.code/output.sourcemap are placed and when
// output.modules[0].stableId is initialized by the development runtime.
```

## Data flow

1. The TypeScript wrapper delegates to `BindingDevEngine`. NAPI passes the IDs
   to the Rust engine and serializes the returned map and module identities.
2. `DevEngine::render_module_factories` checks the engine lifecycle and locks
   the bundler. It sends no coordinator message and touches no client, stamp,
   lazy-compilation, or pending-payload state.
3. `Bundler::render_module_factories`
   (`crates/rolldown/src/bundler/impl_bundler_module_factories.rs`) reads
   `ScanStageCache::snapshot`, looks up exact IDs, validates the requested
   source forms, and constructs the ordered selection.
4. Clone the selected ASTs before entering rayon. Mutation and re-running
   semantic analysis are confined to those owned clones; the cached trees and
   scan-time side tables remain unchanged.
5. `hmr/module_factory_renderer.rs::render_module_factory` runs the existing
   `HmrAstFinalizer`, prints the result, and collapses plugin source-map chains.
   HMR patch rendering, lazy compilation, and unchanged-output suppression
   delegate to this same function with their existing inputs.
6. `render_register_graph_source` emits the existing topology prelude.
   `SourceJoiner` combines it with the module registrations and offsets their
   source maps. No file emitter, output hook, source-map path rewriting, or
   runtime mutation is involved.

All request validation finishes before source rendering starts. A failed request
does not change the compiler graph. Successful rendering also leaves it intact.

## Runtime loading and execution

`crates/rolldown_plugin_hmr/src/runtime/runtime-extra-dev-common.js` implements the
generic source-factory loading contract:

1. `importModule(id, commonJsInterop?)` walks compiler-emitted static graph rows,
   skipping executed modules and using one request-local visited set for cycles.
2. For each missing factory, `#loadFactory` calls the optional host
   `loadModuleFactories(id)` callback. A per-ID promise map deduplicates concurrent
   loads. The callback must finish registration before resolving.
3. Verify registration (or that the module became executed), then remove the
   settled transport promise. Rejection also removes it so a subsequent import
   can retry. Requests never share graph-walk promises.
4. After the closure is ready, yield before calling synchronous `initModule`.
   Existing cache gating still controls evaluation; preloading executes no body.
5. Return the module exports, applying requested CJS namespace conversion before
   resolving the promise.

`HmrAstFinalizer::try_rewrite_dynamic_import` emits:

```js
// ESM target
__rolldown_runtime__.importModule("src/feature.js")
// CJS target: 0 for Babel interop, 1 for Node interop
__rolldown_runtime__.importModule("src/feature.cjs", 1)
```

External imports and the existing lazy-compilation proxy branch are unchanged.
The loader callback owns resource lookup, registration, and physical-load
coalescing when several IDs map to the same payload. It must not recursively await
an import of the ID whose loading it is fulfilling.

A host installs transport independently of the compiler:

```ts
runtime.loadModuleFactories = async (stableId) => {
  const payload = await loadResource(moduleResources.get(stableId));
  payload.install(runtime); // Registers graph rows and factories, not module bodies.
};
const namespace = await runtime.importModule(entryStableId);
```

`loadResource` and the payload envelope are host-defined, not Rolldown APIs.
Synchronous native entry points can install their required static closure first
and continue using `initModule`; this API does not make synchronous consumers
implicitly asynchronous.

## Invariants

- Rebuild semantic information before mutating cloned ASTs, so scan-time
  `NodeId` side tables still match. See
  [AST mutation](../ast-mutation/implementation.md).
- Keep payload-position indices deterministic: imported external bindings use
  those indices to avoid collisions in the combined ESM payload.
- Do not call build/update orchestration from the renderer. Snapshot presence
  does not establish freshness; see
  [data lifecycle](../bundler-data-lifecycle/implementation.md).
- Do not change existing HMR/lazy output or delivery behavior when sharing the
  renderer. Their framing, initialization tail, assets, and stamps remain in
  `HmrStage`, not this API.
- The registration code expects an installed development runtime. It neither
  embeds the runtime nor makes HMR acceptance/disposal decisions.

## Related

- [HMR design](../hmr/design.md)
- [Dev engine](../dev-engine/implementation.md)
- [Lazy compilation](../lazy-compilation/implementation.md)
