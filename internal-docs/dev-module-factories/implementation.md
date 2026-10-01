# Factory-backed development output — implementation

See [design.md](./design.md) for scope, costs, and unresolved semantic boundaries.

## Compiler

1. `DevModeOptions.module_factories` is exposed as `devMode.moduleFactories` in the
   existing TypeScript/NAPI option schema. No DevEngine method or detached output type.
2. Option normalization requires ESM output and disables constant inlining.
   `hmr/validate_dev_factory_modules.rs` rejects TLA and raw lazy exports both during
   initial generation and after incremental/lazy scans.
3. `LinkStage::wrap_modules` uses existing ESM/CJS wrappers for every application module.
   The internal runtime remains unwrapped. Normal inclusion, symbols, chunk planning,
   cross-chunk imports, export names, and entry initialization remain compiler-owned.
4. `module_finalizers/dev_module_factories.rs` consumes the already-linked wrapper body.
   ESM variables/functions retain the finalizer's existing hoisting; CommonJS retains a
   fresh module/exports holder per execution. The initializer enters the runtime gate.
5. ESM binding callbacks assign only canonical exports owned by the source. Existing
   reexport routes remain connected through normal linked owners; redirecting a route
   to a different owner is a documented unresolved boundary. Synthetic deconflicted wrapper/hot
   names serve as callback parameters without scanning the chunk for fresh identifiers.
6. `HmrAstFinalizer` uses scan-time SymbolId/reference flags to recognize exported
   writes. It publishes only initialized names before full completion, including hoisted
   functions/vars before static dependency evaluation. Generated notifications preserve
   expression values and loop control flow. They are inserted after semantic analysis;
   source NodeId lookups retain the existing clone/semantic contract.
7. The ordinary graph prelude uses `registerInitialGraph` in this mode. The runtime
   chunk additionally emits one `registerChunkLoader(ids, () => import(path))` per
   other live source-owning chunk. Its dynamic-import metadata records these same edges,
   so naming, hashes, output metadata, and renderChunk see ordinary chunk imports.
8. Source-patch dynamic imports use `importModule`; normal initial dynamic imports
   retain normal chunk-path lowering. Patches extend their carried set with missing
   synchronous dependencies using the existing client dependency collector.

## Runtime

`createModuleInitializer(id, bindings, body)` registers a low-priority fallback, attaches
the compiler-owned binding callback, and returns an initializer calling `initModule(id)`.
A previously executed replacement can refresh a late chunk immediately.

`createCommonJsInitializer` adapts its callback to the existing module/exports holder ABI.
It does not introduce a second execution cache.

`registerFactory` and `registerGraph` remain last-write-wins for patches.
`registerInitialFactory` and `registerInitialGraph` never overwrite resident versions.
Factory completion refreshes all native facade bindings; `notifyExports` refreshes only
the explicitly initialized/written names while other getters may remain in TDZ.

`importModule` checks the target cache/factory before looking up its physical loader.
The callback loads one normal output chunk, which registers its factories. In-flight
loads are deduplicated by callback and deleted on settlement, allowing retries. No
dependency graph is interpreted by this loading path.

## Expected generated shape (schematic)

```js
import { init_dep, dep } from './common.js';

var value;
var source_exports = __exportAll({ value: () => value });
var init_source = __rolldown_runtime__.createModuleInitializer(
  './source.js',
  (namespace, names) => {
    if (!names || names.has('value')) value = namespace.value;
    if (!names) source_exports = namespace;
  },
  () => {
    // Ordinary linked body, including registration and dependency init calls.
    init_dep();
    value = dep + 1;
  }
);

init_source(); // ordinary entry execution
export { value };
```

The actual wrapper includes the existing registerModule/hot-context header. Loading an
initial chunk registers code; only its ordinary entry/init calls evaluate application
bodies. If a patch factory is resident, the linked initial body is not selected.

## Verification performed

- Rust check for core, binding, and shared-runtime plugin.
- Rust Clippy with warnings denied; rustfmt and diff whitespace checks.
- TypeScript 7 package typecheck and strict changed-runtime JS check.
- Native/WASI option declarations regenerated from NAPI macro output with the official
  NAPI generator, not manually edited.
- No tests added/run; no claim of runtime-semantic or cross-platform verification.
