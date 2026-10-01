use json_escape_simd::escape;
use rolldown_common::RUNTIME_MODULE_KEY;

use crate::types::generator::GenerateContext;

/// One normal `import()` callback per physical chunk, indexed by the source ids it owns.
/// Both initial linking and async transport remain compiler-owned. No additional outputs
/// or runtime dependency traversal are needed.
/// See internal-docs/dev-module-factories/implementation.md.
pub(super) fn render_dev_module_loaders(ctx: &GenerateContext<'_>) -> String {
  let mut code = String::new();
  for chunk_idx in &ctx.chunk_graph.sorted_chunk_idx_vec {
    if *chunk_idx == ctx.chunk_idx {
      continue;
    }
    let chunk = &ctx.chunk_graph.chunk_table[*chunk_idx];
    let ids = chunk
      .modules
      .iter()
      .filter_map(|idx| {
        let module = ctx.link_output.module_table[*idx].as_normal()?;
        (module.id.as_str() != RUNTIME_MODULE_KEY).then_some(module.stable_id.as_str())
      })
      .collect::<Vec<_>>();
    if ids.is_empty() {
      continue;
    }
    code.push_str("__rolldown_runtime__.registerChunkLoader([");
    for (index, id) in ids.iter().enumerate() {
      if index > 0 {
        code.push(',');
      }
      code.push_str(&escape(id));
    }
    code.push_str("],()=>import(");
    code.push_str(&escape(&ctx.chunk.import_path_for(chunk)));
    code.push_str("));\n");
  }
  code
}
