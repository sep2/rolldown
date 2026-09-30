use rolldown_common::{
  EcmaModuleAstUsage, Module, ModuleFactoriesOutput, ModuleFactoryInfo, ModuleType,
  RUNTIME_MODULE_KEY,
};
use rolldown_error::BuildResult;
use rolldown_sourcemap::{Source, SourceJoiner, SourceMapSource};
#[cfg(not(target_family = "wasm"))]
use rolldown_utils::rayon::IndexedParallelIterator;
use rolldown_utils::{
  indexmap::FxIndexSet,
  rayon::{IntoParallelIterator, ParallelIterator},
};

use super::Bundler;
use crate::hmr::{
  module_factory_renderer::{ModuleRenderInput, render_module_factory},
  module_graph_delta::render_register_graph_source,
};

impl Bundler {
  /// Render exactly the requested modules from the retained scan snapshot.
  ///
  /// Does not scan, resolve, run plugin hooks, emit assets, or update HMR delivery state.
  /// Call after a successful dev-mode build; this does not ensure snapshot freshness.
  /// See internal-docs/module-factory-rendering/design.md.
  pub fn render_module_factories(
    &self,
    module_ids: &[String],
  ) -> BuildResult<ModuleFactoriesOutput> {
    self.create_error_if_closed()?;
    let options = self.options();
    if !options.is_dev_mode_enabled() {
      return Err(anyhow::anyhow!("Factory rendering requires dev mode").into());
    }
    let snapshot = self
      .cache
      .snapshot()
      .ok_or_else(|| anyhow::anyhow!("Factory rendering requires a retained scan snapshot"))?;

    // The ordered set preserves first-request order while deduplicating in O(n) expected time.
    let selected = module_ids
      .iter()
      .map(|id| {
        let idx = self
          .cache
          .module_id_to_idx
          .get(id.as_str())
          .ok_or_else(|| anyhow::anyhow!("Unknown module: {id}"))?
          .idx();
        let Module::Normal(module) = &snapshot.module_table.modules[idx] else {
          anyhow::bail!("Cannot render an external module as a factory: {id}");
        };
        anyhow::ensure!(id != RUNTIME_MODULE_KEY, "The internal runtime has no source factory");
        // These forms need link/generate-stage lowering that the source factory finalizer
        // does not perform. Reject this request, not the application's normal build.
        anyhow::ensure!(
          matches!(
            module.module_type,
            ModuleType::Js | ModuleType::Jsx | ModuleType::Ts | ModuleType::Tsx | ModuleType::Empty
          ),
          "Factory rendering does not support module type {}: {id}",
          module.module_type
        );
        anyhow::ensure!(
          !module.ast_usage.contains(EcmaModuleAstUsage::TopLevelAwait),
          "Factory rendering does not support top-level await: {id}"
        );
        anyhow::ensure!(
          module.new_url_references.is_empty() && module.rolldown_file_url_references.is_empty(),
          "Factory rendering does not support generated asset URLs: {id}"
        );
        Ok(idx)
      })
      .collect::<anyhow::Result<FxIndexSet<_>>>()?;

    let modules = selected
      .iter()
      .map(|idx| {
        let module = &snapshot.module_table.modules[*idx];
        ModuleFactoryInfo { id: module.id().clone(), stable_id: module.stable_id().clone() }
      })
      .collect();
    // Clone before entering rayon: the cached arena-backed ASTs are not Sync.
    let render_inputs = selected
      .iter()
      .map(|idx| ModuleRenderInput {
        idx: *idx,
        ecma_ast: snapshot.index_ecma_ast[*idx]
          .as_ref()
          .expect("Scanned JavaScript modules have an AST")
          .clone_with_another_arena(),
      })
      .collect::<Vec<_>>();
    let rendered = render_inputs
      .into_par_iter()
      .enumerate()
      .map(|(index, input)| {
        let (code, map) =
          render_module_factory(&snapshot.module_table, options, input, index, true);
        let source: Box<dyn Source + Send> = if let Some(map) = map {
          Box::new(SourceMapSource::new(code, map))
        } else {
          Box::new(code)
        };
        source
      })
      .collect::<Vec<_>>();

    // Joining only mutates the local output accumulator; the compiler snapshot is unchanged.
    let mut joiner = SourceJoiner::default();
    if let Some(graph) =
      render_register_graph_source(&snapshot.module_table, selected.iter().copied())
    {
      joiner.append_source(graph);
    }
    for source in rendered {
      joiner.append_source_dyn(source);
    }
    let (code, sourcemap) = joiner.join();
    Ok(ModuleFactoriesOutput { code, sourcemap, modules })
  }
}
