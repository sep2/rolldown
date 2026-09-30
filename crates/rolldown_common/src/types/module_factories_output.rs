use rolldown_sourcemap::SourceMap;

use crate::{ModuleId, StableModuleId};

/// Source identity and the corresponding key in the development runtime.
pub struct ModuleFactoryInfo {
  pub id: ModuleId,
  pub stable_id: StableModuleId,
}

/// Factory registrations rendered from an existing scan snapshot, not a build output.
pub struct ModuleFactoriesOutput {
  pub code: String,
  pub sourcemap: Option<SourceMap>,
  pub modules: Vec<ModuleFactoryInfo>,
}
