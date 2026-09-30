# Rendering source module factories

> [Implementation map](./implementation.md)

## Purpose

The compiler already renders source-level factories for HMR patches, lazy
compilation, and unchanged-output comparisons. That rendering is useful
independently of those orchestration paths: tooling can request a known set of
modules for a custom development payload, inspect the generated factories, or
evaluate selected modules with Rolldown's development runtime without making a
new build.

Expose that capability as `DevEngine.renderModuleFactories(moduleIds)` and
`Bundler::render_module_factories`. Reuse the retained, resolved, transformed scan
graph and the same finalizer and source-map chain as HMR. Do not introduce another
compiler pipeline or a second factory ABI.

## Boundary: rendering, not orchestration

The request selects exactly the supplied resolved module IDs, deduplicated in
first-request order. Dependencies remain references; there is no implicit graph
closure. The returned identities connect compiler IDs to development-runtime
stable IDs. Graph rows include dependency IDs, but those do not imply that their
factories were rendered.

Rendering does not load sources, run hooks, emit assets, select HMR boundaries,
initialize modules, acknowledge delivery, or update client state. The result has
no filename or deployment layout. Consumers own packaging and execution. Loading
the code registers internal factories without running their bodies, but external
ESM imports can evaluate external modules.

Normal bundle generation remains scope-hoisted. This API changes neither chunk
grouping nor entry exports or normal-bundle execution order. The shared runtime
adds an opt-in factory loader; HMR selection and delivery policy remain unchanged. A consumer cannot transparently replace a normal
output chunk with factory code: it must arrange dependencies, runtime
initialization, and entry/export behavior itself.

## Asynchronous execution is a separate runtime capability

Source factories cannot use a physical chunk import to load their dependencies.
The shared `DevRuntime` therefore exposes `importModule(stableId, commonJsInterop?)`
and an optional `loadModuleFactories(stableId)` host callback. Source-factory
`import()` codegen calls this primitive; normal scope-hoisted output and the
existing lazy-compilation proxy transport keep their own loading paths.

The runtime prepares the requested module's static factory closure, then enters
the existing synchronous `initModule` gate. Dynamic graph edges are not traversed.
Static imports and `require()` stay synchronous inside factories: loading completes
before any body starts. The callback supplies registration-only code and must
install its graph rows and factories before settling. It may return synchronously
or asynchronously and may install several modules from one payload.

Each import owns its cycle-safe graph walk. Only individual in-flight loads are
shared; caching entire dependency-closure promises would deadlock concurrent
imports at different roots of a cycle. Failed loads are removed for retry.
Without a callback, missing factories still reject with `MissingFactoryError`.
An already-executed module needs no loading or traversal.

The compiler passes CJS interop mode when needed, so namespace conversion happens
before the import promise resolves, rather than accidentally resolving raw
thenable CJS exports first. Evaluation is asynchronous even on the resident-factory
path; initialization errors reject the import promise.

No transport is built into Rolldown: the host maps stable IDs to resources and
installs their code. The callback does not acknowledge delivery, choose HMR
boundaries, or evict module caches. A host must still coordinate payload versions
and must not install stale baseline code over newer HMR registrations.

## Snapshot, not a freshness guarantee

The engine locks the bundler for one coherent render. It does not schedule or
wait for queued builds. Consumers coordinate build completion before requesting
code; a retained snapshot after an error can be older than current source.
There is no new version or delivery protocol. Pairing factories with other build
outputs, or preventing a later stale payload from overwriting a newer factory,
remains the consumer's responsibility.

Do not await the API reentrantly from a plugin hook that holds the bundler lock.
Output callbacks also run while that lock is held; start follow-up work after the
callback returns.

## Current capability limits

The new request requires dev mode and a retained scan snapshot. Unknown IDs,
external modules, the internal runtime, non-JavaScript loaders, top-level await,
and generated asset-URL references are rejected by this API. The existing source
factory finalizer does not perform their necessary lowering. These checks apply
only to requested factories, never to normal builds or the existing HMR path.

The code uses the existing experimental development-runtime ABI and may contain
top-level ESM imports for external dependencies, regardless of the normal output
format. There is no independent format conversion, tree shaking, minification,
banner/footer application, asset emission, or source-map URL processing.

## Cost

Selection is expected O(N) in requested IDs. Graph rendering visits the selected
modules' import records. AST cloning, semantic analysis, printing, and source-map
composition are proportional to the selected source and map sizes; independent
module rendering uses the existing parallel worker pool. No unrelated source is
re-transformed, and the renderer adds no persistent cache or per-client state.
Runtime import preparation is O(V + E) in the unexecuted static closure, excluding
host I/O. Its shared load map retains only currently in-flight requests.
