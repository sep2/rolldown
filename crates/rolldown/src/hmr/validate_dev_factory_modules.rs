use rolldown_common::{EcmaModuleAstUsage, IndexModules, Module};
use rolldown_error::{BuildDiagnostic, BuildResult};

/// The source-patch renderer currently has a synchronous, JS-module export ABI.
/// Reject unsupported inputs in both the initial build and incremental scans, rather
/// than emitting factories that silently drop an export or complete before an await.
pub fn validate_dev_factory_modules(modules: &IndexModules) -> BuildResult<()> {
  for module in modules.iter().filter_map(Module::as_normal) {
    let message = if module.meta.has_lazy_export() {
      // Link-stage JSON/default/CJS lowering is not represented in the cached source AST.
      Some("Factory-backed dev output does not yet support raw lazy exports (JSON/text/data).")
    } else if module.ast_usage.contains(EcmaModuleAstUsage::TopLevelAwait) {
      Some("Factory-backed dev output currently supports synchronous module evaluation only.")
    } else {
      None
    };
    if let Some(message) = message {
      return Err(
        BuildDiagnostic::unsupported_feature(
          module.id.as_arc_str().clone(),
          module.source.clone(),
          oxc::span::SPAN,
          message.to_string(),
        )
        .into(),
      );
    }
  }
  Ok(())
}
