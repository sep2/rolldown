# Factory-backed development output — review prototype

## Problem

A dev host can receive a newer source factory before it evaluates an ordinary initial
chunk containing that source. Scope-hoisted initial code currently executes directly:
registering its exports cannot prevent the old body from running.

The generic requirement is **select the resident factory before evaluation**, without
turning normal output into a host-specific registration protocol.

## Chosen seam

Opt in with `experimental.devMode.moduleFactories: true`.

- Keep the normal link, code-splitting, naming, sourcemap, renderChunk, and entry pipeline.
- Wrap application modules with the linker's existing per-module wrappers.
- Lower those wrappers to registry-backed initializers instead of a separate one-shot cache.
- Initial factories and graph rows are fallbacks. A previously delivered patch wins.
- Entries still execute automatically and expose ordinary exports. There is no entry-id
  export, manual startup requirement, extra build, or new output directory.
- Updated factories load cold dynamic targets through normal compiler-emitted `import()`
  callbacks. One callback represents one existing physical chunk. No runtime graph walk
  or host-provided source-factory loading API is introduced.

This replaces the detached cached-factory-rendering design. Initial code is lowered only
once, by the normal finalizer; the existing source-patch renderer remains the HMR path.

## Why the export bridge remains

Linked initial factories close over normal canonical chunk variables. Patch factories
have their own source-level lexical scopes. Selecting a patch while leaving the chunk's
variables untouched would serve stale named exports.

One compiler-owned callback reconnects source-owned exports to those canonical variables.
Replacement declarations, hoisted function/var exports, assignments, updates, and loop
targets publish initialized export names. Factory completion performs a full refresh.
This updates mutable own exports after successful evaluation; copying once at
startup would not preserve `export let count` followed by `count++`.

HMR acceptance, disposal, update ordering, and reload policy remain client-owned.

## Costs

- Development-only wrappers, registry entries, binding callbacks, and export-write
  notifications. Production and default dev output retain their current lowering.
- Constant inlining is disabled in this mode so linked consumers retain binding reads.
- Loader metadata is O(modules + chunks); an import performs one target lookup and at
  most one physical-chunk load. Concurrent loads share the callback's in-flight promise.
- Binding refresh is O(exports in that source module); changed-name membership uses a Set.
- A patch carries the synchronous dependency closure needed by its factories, stopping
  at current acknowledged factories. The first patch can consequently be larger.
- The ordinary planner still determines partitions: naming/grouping options remain
  applicable, but changing wrapping can change automatic partitioning and hashes.

## Explicit review limits

This is **not upstream-ready**. Only static compilation/lint/type checks have been run;
no tests were added or run.

- ESM output and synchronous JS evaluation only. Top-level await and raw lazy exports
  (built-in JSON/text/data lowering) are rejected in initial and incremental scans.
  The latter need a shared linked/source export ABI, not another ad-hoc data renderer.
- Reexport redirection is unresolved: changing a barrel from one export owner to another
  cannot rewire an earlier canonical chunk binding merely by refreshing source-owned
  variables. This requires a proper per-module facade strategy before upstreaming.
- The bridge is not a language-level native-ESM replacement. Circular TDZ reads through
  retained native closures and partially successful destructuring writes still require
  semantic review. Publication occurs after a declaration/assignment succeeds.
- References retained outside the HMR invalidation set do not magically become newly
  instantiated functions or objects. Existing client cycle/reload policy still applies.
- Native external imports evaluate under the host's ordinary import semantics.
- “Newer wins” means a newer factory already delivered to this runtime, not an update
  the host never fetched. Existing HMR delivery/acknowledgement rules remain necessary.

See [implementation.md](./implementation.md) and the default
[HMR design](../hmr/design.md).
