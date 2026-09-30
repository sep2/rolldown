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
grouping nor entry exports, execution order, runtime implementation, or existing
HMR/lazy-compilation policy. A consumer cannot transparently replace a normal
output chunk with factory code: it must arrange dependencies, runtime
initialization, and entry/export behavior itself.

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
re-transformed, and no additional persistent cache or per-client state is added.
