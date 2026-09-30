use oxc::ast::builder::AstBuilder;
use oxc_traverse::traverse_mut;
use rolldown_common::{Module, ModuleIdx, ModuleTable, NormalizedBundlerOptions};
use rolldown_ecmascript::{EcmaAst, EcmaCompiler, PrintCommentsOptions, PrintOptions};
use rolldown_sourcemap::SourceMap;
use rolldown_utils::indexmap::{FxIndexMap, FxIndexSet};
use rustc_hash::FxHashMap;

use crate::{
  hmr::hmr_ast_finalizer::HmrAstFinalizer, utils::render_ecma_module::collapse_module_sourcemap,
};

// Owned AST clones keep mutation local and can move into parallel render workers.
// See internal-docs/module-factory-rendering/implementation.md.
pub struct ModuleRenderInput {
  pub idx: ModuleIdx,
  pub ecma_ast: EcmaAst,
}

/// Finalize and print one module into its development-runtime form (factory-registration
/// snippet, without the `//#region` framing).
///
/// `unique_index` seeds the payload-position-dependent binding suffixes, so two
/// renders of the same module compare equal only when they pin it to the same
/// value. `with_sourcemap: false` skips sourcemap generation even when the
/// options ask for one.
pub fn render_module_factory(
  module_table: &ModuleTable,
  options: &NormalizedBundlerOptions,
  render_input: ModuleRenderInput,
  unique_index: usize,
  with_sourcemap: bool,
) -> (String, Option<SourceMap>) {
  let ModuleRenderInput { idx: module_idx, ecma_ast: mut ast } = render_input;

  let Module::Normal(module) = &module_table.modules[module_idx] else {
    unreachable!("Factory rendering only supports normal modules");
  };

  let enable_sourcemap = with_sourcemap && options.sourcemap.is_some() && !module.is_virtual();
  let use_pife_for_module_wrappers = options.optimization.is_pife_for_module_wrappers_enabled();
  let modules = &module_table.modules;

  ast.program.with_mut(|fields| {
    // Re-running semantic re-stamps every NodeId. The NodeId-keyed side-table lookups
    // below still hit only because the clone is unmutated at this point: identical tree
    // shape re-derives exactly the scan-time ids (see internal-docs/ast-mutation/implementation.md).
    let scoping = EcmaAst::make_semantic(fields.program).into_scoping();

    let mut finalizer = HmrAstFinalizer {
      modules,
      ast_builder: AstBuilder::new(fields.allocator),
      import_bindings: FxHashMap::default(),
      module,
      exports: oxc::allocator::Vec::new_in(&fields.allocator),
      use_pife_for_module_wrappers,
      dependencies: FxIndexSet::default(),
      generated_load_exports_stmts: FxHashMap::default(),
      generated_static_import_infos: FxHashMap::default(),
      re_export_all_dependencies: FxIndexSet::default(),
      generated_static_import_stmts_from_external: FxIndexMap::default(),
      unique_index,
      named_exports: FxHashMap::default(),
    };

    traverse_mut(&mut finalizer, fields.allocator, fields.program, scoping, ());
  });

  let codegen = EcmaCompiler::print_with(
    &ast,
    PrintOptions {
      sourcemap: enable_sourcemap,
      filename: module.id.to_string(),
      comments: PrintCommentsOptions {
        legal: false, // factory payloads omit legal comments
        annotation: options.comments.annotation,
        jsdoc: options.comments.jsdoc,
      },
      initial_indent: 0,
    },
  );

  // The codegen map is the last element of the module's sourcemap chain, so on its
  // own it maps back to what the plugins produced rather than to the original file.
  // Collapsing the chain is what lets a position in a patch or a lazy chunk resolve
  // to the source the user wrote, the same way `render_ecma_module` does for a
  // module rendered into a chunk.
  //
  // Sourcemap diagnostics remain owned by normal build reporting; factory rendering
  // has no diagnostics emission hook.
  let (code, codegen_map) = match codegen.map {
    Some(map) => (codegen.code, Some(map.into_owned())),
    None => (codegen.code, None),
  };

  // Guarded, because a chain collapses to a map even when the codegen step
  // contributed none — which would hand back a sourcemap for a render that asked
  // not to have one.
  let map = enable_sourcemap
    .then(|| {
      collapse_module_sourcemap(
        &module.sourcemap_chain,
        codegen_map,
        module.id.as_str(),
        &mut Vec::new(),
      )
    })
    .flatten();

  (code, map)
}
