use oxc::{
  allocator,
  ast::ast::{self, Argument, ArrowFunctionBody, Expression, Statement},
  span::SPAN,
};
use rolldown_ecmascript_utils::{ExpressionFactoryExt as _, StatementFactoryExt as _};
use rustc_hash::FxHashSet;

use super::ScopeHoistingFinalizer;

impl<'ast> ScopeHoistingFinalizer<'_, 'ast> {
  pub(super) fn create_commonjs_initializer_stmt(
    &self,
    name: &str,
    helper: Expression<'ast>,
    statements: allocator::Vec<'ast, Statement<'ast>>,
  ) -> Statement<'ast> {
    if self.ctx.options.is_dev_module_factories_enabled() {
      return self.create_dev_initializer_stmt(name, statements, true);
    }
    Statement::new_commonjs_wrapper_stmt(
      name,
      helper,
      statements,
      self.ctx.module.ast_usage,
      self.ctx.options.profiler_names,
      &self.ctx.module.stable_id,
      self.ctx.linking_info.is_tla_or_contains_tla_dependency,
      self,
    )
  }

  /// Lower the already-linked wrapper body, not a second source-level compilation.
  /// Chunk imports, canonical variables, and entry exports remain in the normal pipeline.
  /// See internal-docs/dev-module-factories/implementation.md.
  pub(super) fn create_dev_initializer_stmt(
    &self,
    name: &str,
    statements: allocator::Vec<'ast, Statement<'ast>>,
    commonjs: bool,
  ) -> Statement<'ast> {
    let factory =
      self.module_factory_callback(if commonjs { &["exports", "module"] } else { &[] }, statements);
    let mut args = allocator::Vec::new_in(self);
    args.push(Argument::new_string_literal(
      SPAN,
      ast::Str::from_str_in(&self.ctx.module.stable_id, self),
      None,
      self,
    ));
    if !commonjs {
      args.push(self.module_export_bindings_callback(name));
    }
    args.push(factory);
    let method = if commonjs { "createCommonJsInitializer" } else { "createModuleInitializer" };
    Statement::new_var_decl(
      name,
      Expression::new_call_expression(
        SPAN,
        Expression::new_member_access_expr("__rolldown_runtime__", method, self),
        None,
        args,
        false,
        self,
      ),
      self,
    )
  }

  fn module_factory_callback(
    &self,
    parameters: &[&str],
    statements: allocator::Vec<'ast, Statement<'ast>>,
  ) -> Argument<'ast> {
    Argument::new_arrow_function_expression(
      SPAN,
      false,
      None,
      ast::FormalParameters::boxed(
        SPAN,
        ast::FormalParameterKind::ArrowFormalParameters,
        allocator::Vec::from_iter_in(
          parameters.iter().map(|name| {
            ast::FormalParameter::new(
              SPAN,
              [],
              ast::BindingPattern::new_binding_identifier(
                SPAN,
                ast::Str::from_str_in(name, self),
                self,
              ),
              None,
              None,
              false,
              None,
              false,
              false,
              self,
            )
          }),
          self,
        ),
        None,
        self,
      ),
      None,
      ArrowFunctionBody::FunctionBody(ast::FunctionBody::boxed(SPAN, [], statements, self)),
      self,
    )
  }

  fn module_export_bindings_callback(&self, wrapper_name: &str) -> Argument<'ast> {
    // Reuse two deconflicted synthetic names as callback parameters. Neither is an
    // application export, so shadowing them cannot hide a variable this callback assigns.
    // This avoids an O(chunk symbols) name scan for every module.
    let namespace_name = wrapper_name;
    let names_name = self.canonical_name_for(
      self.ctx.module.hmr_hot_ref.expect("dev modules have a hot-context symbol"),
    );
    let mut statements = allocator::Vec::new_in(self);
    // Alias exports may share a canonical binding; assign that binding only once.
    let mut seen = FxHashSet::default();
    for (exported, export) in self.ctx.linking_info.canonical_exports(false) {
      let symbol_ref = self.ctx.symbol_db.canonical_ref_for(export.symbol_ref);
      if symbol_ref.owner != self.ctx.idx
        || self.ctx.symbol_db.get(symbol_ref).namespace_alias.is_some()
        || !self.ctx.used_symbol_refs.contains(&symbol_ref)
        || !seen.insert(symbol_ref)
      {
        continue;
      }
      let value = Expression::new_computed_member_expression(
        SPAN,
        Expression::new_identifier(SPAN, ast::Str::from_str_in(namespace_name, self), self),
        Expression::new_string_literal(SPAN, ast::Str::from_str_in(exported, self), None, self),
        false,
        self,
      );
      let names = Expression::new_identifier(SPAN, ast::Str::from_str_in(names_name, self), self);
      let condition = Expression::new_logical_expression(
        SPAN,
        Expression::new_unary_expression(SPAN, ast::UnaryOperator::LogicalNot, names, self),
        ast::LogicalOperator::Or,
        Expression::new_call_with_arg(
          Expression::new_member_access_expr(names_name, "has", self),
          Expression::new_string_literal(SPAN, ast::Str::from_str_in(exported, self), None, self),
          false,
          self,
        ),
        self,
      );
      statements.push(Statement::new_if_statement(
        SPAN,
        condition,
        self.export_binding_assignment(self.canonical_name_for(symbol_ref), value),
        None,
        self,
      ));
    }
    if self.module_namespace_included {
      statements.push(Statement::new_if_statement(
        SPAN,
        Expression::new_unary_expression(
          SPAN,
          ast::UnaryOperator::LogicalNot,
          Expression::new_identifier(SPAN, ast::Str::from_str_in(names_name, self), self),
          self,
        ),
        self.export_binding_assignment(
          self.canonical_name_for(self.ctx.module.namespace_object_ref),
          Expression::new_identifier(SPAN, ast::Str::from_str_in(namespace_name, self), self),
        ),
        None,
        self,
      ));
    }
    self.module_factory_callback(&[namespace_name, names_name], statements)
  }

  fn export_binding_assignment(&self, name: &str, value: Expression<'ast>) -> Statement<'ast> {
    Statement::new_expression_statement(
      SPAN,
      Expression::new_assignment_expression(
        SPAN,
        ast::AssignmentOperator::Assign,
        ast::AssignmentTarget::new_assignment_target_identifier(
          SPAN,
          ast::Str::from_str_in(name, self),
          self,
        ),
        value,
        self,
      ),
      self,
    )
  }
}
