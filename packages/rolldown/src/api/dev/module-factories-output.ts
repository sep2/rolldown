/** A source module and its key in Rolldown's development runtime. */
export interface ModuleFactoryInfo {
  /** Resolved compiler module ID, as returned by the module graph. */
  id: string;
  /** Stable ID used by registerFactory, initModule, and loadExports. */
  stableId: string;
}

/** Source factory code rendered from the engine's existing scan snapshot. */
export interface ModuleFactoriesOutput {
  /**
   * Development-runtime graph and factory registrations. There is no initModule
   * tail, delivery acknowledgement, or runtime bootstrap. External ESM imports
   * may appear at top level.
   */
  code: string;
  /**
   * JSON source map when source maps are enabled. No sourceMappingURL or map
   * asset is emitted; the caller decides where to place the code and map.
   */
  sourcemap?: string | null;
  /** Requested modules, deduplicated in first-request order. */
  modules: ModuleFactoryInfo[];
}
