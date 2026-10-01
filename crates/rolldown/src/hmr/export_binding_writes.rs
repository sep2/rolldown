use oxc::{
  ast::ast::{
    BindingIdentifier, Function, IdentifierReference, VariableDeclaration, VariableDeclarationKind,
  },
  ast_visit::{VisitJs, walk_js},
  semantic::{ScopeFlags, Scoping, SymbolId},
};
use rustc_hash::FxHashSet;

/// Inspect only an assignment target, never a member's property write as a binding write.
/// The scan-time reference flags distinguish `x = ...` from `x.property = ...`.
/// See internal-docs/dev-module-factories/implementation.md.
#[derive(Debug)]
pub(super) struct ExportBindingWrites<'a> {
  pub scoping: &'a Scoping,
  pub exported: &'a FxHashSet<SymbolId>,
  /// Local traversal accumulator: deduplicate exported symbols written by one target.
  pub symbols: FxHashSet<SymbolId>,
}

/// Function and var exports are readable at factory instantiation, before static dependencies.
/// The accumulator uses source symbol identity so shadowed local declarations are ignored.
#[derive(Debug)]
pub(super) struct HoistedExportBindings<'a> {
  pub exported: &'a FxHashSet<SymbolId>,
  pub symbols: FxHashSet<SymbolId>,
  pub anonymous_default: bool,
}

impl<'ast> VisitJs<'ast> for HoistedExportBindings<'_> {
  fn visit_function(&mut self, function: &Function<'ast>, _flags: ScopeFlags) {
    if let Some(id) = &function.id {
      if let Some(symbol) = id.symbol_id.get() {
        if self.exported.contains(&symbol) {
          self.symbols.insert(symbol);
        }
      } else if id.name == "__rolldown_default__" {
        self.anonymous_default = true;
      }
    }
    // Nested function declarations cannot initialize an exported root binding.
  }

  fn visit_variable_declaration(&mut self, declaration: &VariableDeclaration<'ast>) {
    if declaration.kind == VariableDeclarationKind::Var {
      for declarator in &declaration.declarations {
        for id in declarator.id.get_binding_identifiers() {
          if let Some(symbol) = id.symbol_id.get()
            && self.exported.contains(&symbol)
          {
            self.symbols.insert(symbol);
          }
        }
      }
    }
    walk_js::walk_variable_declaration(self, declaration);
  }
}

impl<'ast> VisitJs<'ast> for ExportBindingWrites<'_> {
  fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'ast>) {
    if let Some(reference_id) = ident.reference_id.get() {
      let reference = self.scoping.get_reference(reference_id);
      if reference.is_write()
        && let Some(symbol) = reference.symbol_id()
        && self.exported.contains(&symbol)
      {
        self.symbols.insert(symbol);
      }
    }
  }

  fn visit_binding_identifier(&mut self, ident: &BindingIdentifier<'ast>) {
    // `for (var exported of values)` writes a root binding through a declaration.
    if let Some(symbol) = ident.symbol_id.get()
      && self.exported.contains(&symbol)
    {
      self.symbols.insert(symbol);
    }
  }
}
