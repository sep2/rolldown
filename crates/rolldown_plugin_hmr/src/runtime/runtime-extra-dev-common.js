// @ts-check

class Module {
  /**
   * @type {{ exports: any }}
   */
  exportsHolder = { exports: null };
  /**
   * @type {string}
   */
  id;
  /** Factory completion gates full binding refreshes: other getters may still be in TDZ.
   * @type {boolean}
   */
  ready = false;

  /**
   * @param {string} id
   */
  constructor(id) {
    this.id = id;
  }

  get exports() {
    return this.exportsHolder.exports;
  }
}

/**
 * Compiler-emitted module-graph delta — pure topology (static + dynamic edges).
 * `ids[0, localCount)` are the modules this payload carries; `ids[localCount, …)` are foreign edge targets.
 * `edges[i]` / `dynamicEdges[i]` are the static / dynamic-`import()` out-edges of `ids[i]`.
 * @typedef {{ ids: string[], localCount: number, edges: number[][], dynamicEdges?: number[][] }} ModuleGraphDelta
 * @typedef {{ createModuleHotContext(moduleId: string): any, onModuleCacheRemoval(moduleId: string): void }} DevRuntimeHooks
 */

export class MissingFactoryError extends Error {
  /**
   * @param {string} id
   */
  constructor(id) {
    super(`No factory registered for module ${id}`);
    this.id = id;
  }
}

export class DevRuntime {
  /**
   * Client ID generated at runtime initialization, used for lazy compilation requests.
   * @type {string}
   */
  clientId;

  /**
   * @param {string} clientId
   */
  constructor(clientId) {
    this.clientId = clientId;
  }

  /**
   * Static import edges from `registerGraph` — entries persist across `removeModuleCache`
   * and change only by replacement from a newer payload (last write wins).
   * @type {Map<string, { edges: string[] }>}
   */
  staticImports = new Map();
  /**
   * Reverse index over the static imports.
   * @type {Map<string, Set<string>>}
   */
  importers = new Map();
  /**
   * Dynamic `import()` edges from `registerGraph`, keyed by importer — mirror of
   * `staticImports` for the dynamic reverse index.
   * @type {Map<string, { edges: string[] }>}
   */
  dynamicImports = new Map();
  /**
   * Reverse index over the dynamic imports.
   * @type {Map<string, Set<string>>}
   */
  dynamicImporters = new Map();
  /**
   * The module cache. Membership means "this module's side effects ran in this tab" —
   * registration is emitted ahead of every module body, and nothing un-registers on
   * unwind, so a factory that throws mid-body stays registered. A `Map` rather than a
   * plain object: HMR eviction deletes entries, and a `delete` on an object drops V8
   * into dictionary mode, taxing every later lookup on the hottest read path.
   * @type {Map<string, Module>}
   */
  moduleCache = new Map();
  /**
   * Re-runnable factories from HMR patches and lazy chunks. Opt-in factory-backed
   * initial chunks contribute lower-priority fallbacks.
   * @type {Map<string, (id: string) => void>}
   */
  factories = new Map();
  /**
   * One compiler-owned binding callback per source module. It reconnects replacement
   * namespaces to the ordinary chunk's live export variables; HMR eviction keeps it.
   * @type {Map<string, (exports: any, names: ReadonlySet<string> | null) => void>}
   */
  #exportBindings = new Map();
  /**
   * Direct compiler-owned chunk loaders, indexed by source id. There is no graph walk:
   * importing one physical chunk registers all its factories through normal evaluation.
   * @type {Map<string, () => Promise<unknown>>}
   */
  #moduleLoaders = new Map();
  /** Shared in-flight load per callback (one callback per physical chunk).
   * Cleared on settlement so a rejected load can be retried.
   * @type {Map<() => Promise<unknown>, Promise<unknown>>}
   */
  #loading = new Map();
  /**
   * Installed by the dev client at boot. The runtime is a store + executor and makes
   * no HMR decisions; accepting, disposing, and reloading live behind these hooks.
   * @type {DevRuntimeHooks | null}
   */
  hooks = null;

  /**
   * @param {ModuleGraphDelta} delta
   */
  registerGraph(delta) {
    this.#registerGraph(delta, false);
  }

  /** Initial output must not roll back graph rows received in a newer patch.
   * @param {ModuleGraphDelta} delta
   */
  registerInitialGraph(delta) {
    this.#registerGraph(delta, true);
  }

  /** @param {ModuleGraphDelta} delta
   * @param {boolean} initial
   */
  #registerGraph(delta, initial) {
    for (let i = 0; i < delta.localCount; i++) {
      const id = delta.ids[i];
      if (initial && this.staticImports.has(id)) {
        continue;
      }
      const edges = delta.edges[i].map((j) => delta.ids[j]);
      for (const target of this.staticImports.get(id)?.edges ?? []) {
        this.importers.get(target)?.delete(id);
      }
      for (const target of edges) {
        let importerSet = this.importers.get(target);
        if (!importerSet) {
          importerSet = new Set();
          this.importers.set(target, importerSet);
        }
        importerSet.add(id);
      }
      this.staticImports.set(id, { edges });

      // Dynamic `import()` edges are maintained in a parallel reverse index with the same
      // last-write-wins bookkeeping; `getImporters` unions the two.
      const dynamicEdges = (delta.dynamicEdges?.[i] ?? []).map((j) => delta.ids[j]);
      for (const target of this.dynamicImports.get(id)?.edges ?? []) {
        this.dynamicImporters.get(target)?.delete(id);
      }
      for (const target of dynamicEdges) {
        let importerSet = this.dynamicImporters.get(target);
        if (!importerSet) {
          importerSet = new Set();
          this.dynamicImporters.set(target, importerSet);
        }
        importerSet.add(id);
      }
      this.dynamicImports.set(id, { edges: dynamicEdges });
    }
  }

  /**
   * @param {string} id
   * @param {(id: string) => void} fn
   */
  registerFactory(id, fn) {
    this.factories.set(id, fn);
  }

  /**
   * Lower-priority registration for normal output that may be loaded after an update.
   * @param {string} id
   * @param {(id: string) => void} factory
   */
  registerInitialFactory(id, factory) {
    if (!this.factories.has(id)) {
      this.factories.set(id, factory);
    }
  }

  /**
   * Compiler lowering of an ordinary ESM initializer. The returned function uses the
   * same cache/factory gate as a patch, rather than retaining a second one-shot cache.
   * See internal-docs/dev-module-factories/design.md.
   * @param {string} id
   * @param {((exports: any, names: ReadonlySet<string> | null) => void) | null} bindings
   * @param {() => void} factory
   * @returns {() => any}
   * @internal
   */
  createModuleInitializer(id, bindings, factory) {
    this.registerInitialFactory(id, factory);
    if (bindings) {
      this.#exportBindings.set(id, bindings);
      const module = this.moduleCache.get(id);
      if (module?.ready) {
        bindings(module.exports, null);
      }
    }
    return () => this.initModule(id);
  }

  /**
   * CommonJS keeps its usual fresh module/exports pair on each factory execution.
   * @param {string} id
   * @param {(exports: any, module: { exports: any }) => void} factory
   * @returns {() => any}
   * @internal
   */
  createCommonJsInitializer(id, factory) {
    return this.createModuleInitializer(id, null, () => {
      const module = { exports: {} };
      factory(module.exports, module);
    });
  }

  /**
   * Generated around writes to exported bindings in replacement factories. Preserve
   * the expression's value (including postfix updates). Publish only initialized names.
   * @template T
   * @param {string} id
   * @param {T} value
   * @param {string[]} names Only bindings whose initialization/write just succeeded.
   * @returns {T}
   * @internal
   */
  notifyExports(id, value, names) {
    const module = this.moduleCache.get(id);
    if (module) {
      this.#exportBindings.get(id)?.(module.exports, new Set(names));
    }
    return value;
  }

  /**
   * @param {string[]} ids
   * @param {() => Promise<unknown>} loader
   * @internal
   */
  registerChunkLoader(ids, loader) {
    for (const id of ids) {
      if (!this.#moduleLoaders.has(id)) {
        this.#moduleLoaders.set(id, loader);
      }
    }
  }

  /**
   * An updated factory's dynamic import still uses the normal output's physical loader
   * when the target is cold. Fresh factories already delivered by HMR need no transport.
   * @param {string} id
   * @param {boolean} commonjs
   * @param {boolean} nodeMode
   * @returns {Promise<any>}
   * @internal
   */
  async importModule(id, commonjs, nodeMode) {
    if (!this.isExecuted(id) && !this.hasFactory(id)) {
      const loader = this.#moduleLoaders.get(id);
      if (!loader) {
        throw new MissingFactoryError(id);
      }
      let loading = this.#loading.get(loader);
      if (!loading) {
        loading = Promise.resolve()
          .then(loader)
          .finally(() => this.#loading.delete(loader));
        this.#loading.set(loader, loading);
      }
      await loading;
    }
    const exports = this.initModule(id);
    return commonjs ? this.__toESM(exports, nodeMode) : exports;
  }

  /**
   * @param {string} id
   * @param {{ exports: any }} [exportsHolder]
   */
  registerModule(id, exportsHolder = { exports: {} }) {
    const module = new Module(id);
    module.exportsHolder = exportsHolder;
    this.moduleCache.set(id, module);
  }

  /**
   * @param {string} id
   * @returns {string[]}
   */
  getImporters(id) {
    // Static ∪ dynamic importers — the boundary walk treats both kinds the same (parity
    // with Vite `node.importers` / webpack `module.parents`). Deduped so a module that
    // imports `id` both statically and via `import()` appears once.
    const dynamic = this.dynamicImporters.get(id);
    if (!dynamic || dynamic.size === 0) {
      return [...(this.importers.get(id) ?? [])];
    }
    return [...new Set([...(this.importers.get(id) ?? []), ...dynamic])];
  }

  /**
   * @param {string} id
   */
  isExecuted(id) {
    return this.moduleCache.has(id);
  }

  /**
   * @param {string} id
   */
  hasFactory(id) {
    return this.factories.has(id);
  }

  /**
   * Module-cache delete only — static imports and factories persist. Removal is what
   * re-arms a cache-gated factory for `initModule`.
   * @param {string} id
   */
  removeModuleCache(id) {
    this.moduleCache.delete(id);
    this.hooks?.onModuleCacheRemoval(id);
  }

  /**
   * The one re-execution gate: registered → return the live exports; otherwise run the
   * mapped factory (which registers itself first, then runs the body).
   * @param {string} id
   */
  initModule(id) {
    if (this.moduleCache.has(id)) {
      return this.loadExports(id);
    }
    const factory = this.factories.get(id);
    if (!factory) {
      throw new MissingFactoryError(id);
    }
    factory(id);
    const module = this.moduleCache.get(id);
    if (module) {
      module.ready = true;
      this.#exportBindings.get(id)?.(module.exports, null);
    }
    return this.loadExports(id);
  }

  /**
   * @param {string} id
   */
  loadExports(id) {
    const module = this.moduleCache.get(id);
    if (module) {
      return module.exportsHolder.exports;
    } else {
      console.warn(`Module ${id} not found`);
      return {};
    }
  }

  /**
   * @param {string} moduleId
   */
  createModuleHotContext(moduleId) {
    if (this.hooks) {
      return this.hooks.createModuleHotContext(moduleId);
    }
    throw new Error('createModuleHotContext requires installed hooks or an override');
  }

  /** @internal */
  // @ts-expect-error The variable will be injected at build time.
  __toESM = __toESM;
  /** @internal */
  // @ts-expect-error The variable will be injected at build time.
  __toCommonJS = __toCommonJS;
  /** @internal */
  // @ts-expect-error The variable will be injected at build time.
  __exportAll = __exportAll;
  /**
   * @param {boolean} [isNodeMode]
   * @returns {(mod: any) => any}
   * @internal
   */
  // @ts-expect-error The variable will be injected at build time.
  __toDynamicImportESM = (isNodeMode) => (mod) => __toESM(mod.default, isNodeMode);
  /** @internal */
  // @ts-expect-error The variable will be injected at build time.
  __reExport = __reExport;
}
