use oxc::{
  allocator::TakeIn,
  ast::ast::{self, Expression},
  ast_visit::VisitJs,
  span::SPAN,
};
use oxc_traverse::Traverse;
use rolldown_ecmascript::{
  CJS_EXPORTS_REF_STR, CJS_MODULE_REF_STR, CJS_ROLLDOWN_EXPORTS_REF,
  CJS_ROLLDOWN_EXPORTS_REF_IDENT, CJS_ROLLDOWN_MODULE_REF, CJS_ROLLDOWN_MODULE_REF_IDENT,
};
use rolldown_ecmascript_utils::ExpressionFactoryExt as _;
use rustc_hash::FxHashSet;

use crate::hmr::{
  export_binding_writes::{ExportBindingWrites, HoistedExportBindings},
  hmr_ast_finalizer::HmrAstFinalizer,
  utils::{HmrAstBuilder, MODULE_ID_PARAM_FOR_HMR},
};

impl<'ast> Traverse<'ast, ()> for HmrAstFinalizer<'_, 'ast> {
  fn enter_program(
    &mut self,
    node: &mut ast::Program<'ast>,
    ctx: &mut oxc_traverse::TraverseCtx<'ast, ()>,
  ) {
    let taken_body = node.body.take_in(self);
    node.body.reserve_exact(taken_body.len());
    taken_body.into_iter().for_each(|top_level_stmt| {
      self.handle_top_level_stmt(&mut node.body, top_level_stmt, ctx.scoping());
    });
  }

  fn exit_program(
    &mut self,
    node: &mut ast::Program<'ast>,
    ctx: &mut oxc_traverse::TraverseCtx<'ast, ()>,
  ) {
    let mut try_block = ast::BlockStatement::boxed(SPAN, [], self);

    // `initModule("<stable id>")` for EVERY static dep, uniformly registry-gated: a
    // co-carried factory runs, a resident module short-circuits. No payload-membership
    // split exists in the emitted bytes.
    // A dep can also carry statements of its own, emitted here before the body: a
    // `var import_dep = loadExports("dep.js")` binding, and one `__reExport(...)` copy per
    // `export * from`. Which ones it carries depends on how it was imported. The order is:
    //
    //   for each dep in `dependencies` (first-reference order):
    //     initModule(dep)
    //     if the dep has a binding: var import_dep = loadExports(dep)
    //     if any `export *` source is now ready: copy it, in `export *` order,
    //     stopping at the first one that is not ready yet
    //
    // Rule 1: as early as possible, copy a re-export (`export * from './dep.js'`) or make a name
    // readable (`export { x } from './dep.js'`, `export * as ns from './dep.js'` -
    // rolldown#10781). Example (the cycle from vitejs/vite#21626):
    //
    //   // index.js                     // b.js
    //   export * from './a.js'          import { valueA } from './index.js'
    //   export { fn } from './b.js'     export const fn = () => valueA.concat('!')
    //
    //   initModule("a.js")
    //   __reExport(exports, loadExports("a.js"))   // `valueA` is on the namespace now
    //   initModule("b.js")                         // b.js runs here and reads `valueA`
    //
    // b.js runs inside `initModule("b.js")`, before this body. If the copy of a.js came after
    // all `initModule` calls, b.js would read `valueA` as `undefined`.
    //
    // Rule 2: keep `export *` order. `__reExport` skips a name the target already has, so the
    // first copy owns a name that two sources export. Example:
    //
    //   import { helper } from './b.js'   // `dependencies` = [b, c]
    //   export * from './c.js'            // `re_export_all_dependencies` = [c, b]
    //   export * from './b.js'            // both c.js and b.js export `foo`
    //
    //   initModule("b.js")                           // c.js is not ready, so no copy yet
    //   initModule("c.js")
    //   __reExport(exports, loadExports("c.js"))     // `foo` comes from c.js
    //   __reExport(exports, loadExports("b.js"))     // `foo` already set, skipped
    //
    // Copying b.js right after `initModule("b.js")` would give `foo` to b.js, only because an
    // unrelated import mentioned b.js first.
    //
    // An external needs no `initModule`. Its `import * as` binding is hoisted, so it is always
    // ready to copy.
    let mut load_exports_stmts = std::mem::take(&mut self.generated_load_exports_stmts);
    let mut dependencies_init_fn_stmts: Vec<ast::Statement<'ast>> = Vec::new();
    let mut initialized = FxHashSet::default();
    let mut next_copy = 0;
    for dep in &self.dependencies {
      let module = &self.modules[*dep];
      if module.as_normal().is_some() {
        dependencies_init_fn_stmts.push(ast::Statement::new_expression_statement(
          SPAN,
          self.make_init_module_call(module),
          self,
        ));
      }
      if let Some(stmt) = load_exports_stmts.remove(dep) {
        dependencies_init_fn_stmts.push(stmt);
      }
      initialized.insert(*dep);
      while let Some(source) = self.re_export_all_dependencies.get_index(next_copy) {
        let source_module = &self.modules[*source];
        if source_module.as_normal().is_some() && !initialized.contains(source) {
          break;
        }
        dependencies_init_fn_stmts.push(self.create_re_export_call_stmt(source_module));
        next_copy += 1;
      }
    }
    // Every binding is created next to a `self.dependencies.insert`, so the loop above emits all
    // of them.
    debug_assert!(load_exports_stmts.is_empty());

    let runtime_module_register = self.generate_runtime_module_register_for_hmr(ctx.scoping());
    let mut hoisted = HoistedExportBindings {
      exported: &self.exported_bindings,
      symbols: FxHashSet::default(),
      anonymous_default: false,
    };
    if !self.exported_bindings.is_empty() {
      hoisted.visit_program(node);
    }
    let mut user_body = node.body.take_in(self);
    if !self.exported_bindings.is_empty() {
      for statement in user_body.take_in(self) {
        let (symbols, anonymous_default) = self.declaration_export_bindings(&statement);
        user_body.push(statement);
        if !symbols.is_empty() || anonymous_default {
          user_body.push(ast::Statement::new_expression_statement(
            SPAN,
            self.export_notification(
              Expression::new_void_0(SPAN, self),
              &symbols,
              anonymous_default,
            ),
            self,
          ));
        }
      }
    }

    // Factories uniformly take only `__rolldown_module_id__`; for CommonJS the
    // module/exports objects become locals the body's rewritten `module`/`exports`
    // references resolve to.
    let cjs_module_locals: Vec<ast::Statement<'ast>> = if self.module.exports_kind.is_commonjs() {
      let empty_exports_object = ast::Expression::new_object_expression(SPAN, [], self);
      let module_object = ast::Expression::new_object_expression(
        SPAN,
        [ast::ObjectPropertyKind::new_object_property(
          SPAN,
          ast::PropertyKind::Init,
          ast::PropertyKey::new_static_identifier(SPAN, "exports", self),
          empty_exports_object,
          true,
          false,
          false,
          self,
        )],
        self,
      );
      vec![
        // var __rolldown_module__ = { exports: {} };
        ast::Statement::from(ast::Declaration::new_variable_declaration(
          SPAN,
          ast::VariableDeclarationKind::Var,
          oxc::allocator::Vec::from_value_in(
            ast::VariableDeclarator::new(
              SPAN,
              ast::BindingPattern::new_binding_identifier(
                SPAN,
                CJS_ROLLDOWN_MODULE_REF_IDENT,
                self,
              ),
              None,
              Some(module_object),
              false,
              self,
            ),
            self,
          ),
          false,
          self,
        )),
        // var __rolldown_exports__ = __rolldown_module__.exports;
        ast::Statement::from(ast::Declaration::new_variable_declaration(
          SPAN,
          ast::VariableDeclarationKind::Var,
          oxc::allocator::Vec::from_value_in(
            ast::VariableDeclarator::new(
              SPAN,
              ast::BindingPattern::new_binding_identifier(
                SPAN,
                CJS_ROLLDOWN_EXPORTS_REF_IDENT,
                self,
              ),
              None,
              Some(Expression::new_member_access_expr(CJS_ROLLDOWN_MODULE_REF, "exports", self)),
              false,
              self,
            ),
            self,
          ),
          false,
          self,
        )),
      ]
    } else {
      vec![]
    };

    try_block.body.reserve_exact(
      cjs_module_locals.len()
        + runtime_module_register.len()
        + user_body.len()
        + dependencies_init_fn_stmts.len()
        + 1, /* import.meta.hot*/
    );
    try_block.body.extend(cjs_module_locals);
    try_block.body.extend(runtime_module_register);
    if !hoisted.symbols.is_empty() || hoisted.anonymous_default {
      try_block.body.push(ast::Statement::new_expression_statement(
        SPAN,
        self.export_notification(
          Expression::new_void_0(SPAN, self),
          &hoisted.symbols,
          hoisted.anonymous_default,
        ),
        self,
      ));
    }
    try_block.body.extend(dependencies_init_fn_stmts);
    try_block.body.push(self.create_module_hot_context_initializer_stmt());
    try_block.body.extend(user_body);

    node
      .body
      .extend(std::mem::take(&mut self.generated_static_import_stmts_from_external).into_values());

    let final_block = ast::BlockStatement::boxed(SPAN, [], self);

    let try_stmt =
      ast::Statement::new_try_statement(SPAN, try_block, None, Some(final_block), self);

    // The runtime calls the factory with the module's stable id as its argument, so it's
    // available inside the body as `__rolldown_module_id__`. This lets registerModule /
    // createModuleHotContext reference the id by identifier instead of duplicating the
    // string literal.
    let module_id_param = ast::FormalParameter::new(
      SPAN,
      [],
      ast::BindingPattern::new_binding_identifier(SPAN, MODULE_ID_PARAM_FOR_HMR, self),
      None,
      None,
      false,
      None,
      false,
      false,
      self,
    );
    let params = ast::FormalParameters::boxed(
      SPAN,
      ast::FormalParameterKind::Signature,
      [module_id_param],
      None,
      self,
    );
    // function () { [user code] }
    let mut user_code_wrapper = ast::Function::boxed(
      SPAN,
      ast::FunctionType::FunctionExpression,
      None,
      false,
      false,
      false,
      None,
      None,
      params,
      None,
      Some(ast::FunctionBody::boxed(SPAN, [], [try_stmt], self)),
      self,
    );
    // mark the callback as PIFE because the callback is executed when this chunk is loaded
    user_code_wrapper.pife = self.use_pife_for_module_wrappers;

    // __rolldown_runtime__.registerFactory(stable_id, function (__rolldown_module_id__) { [user code] })
    // Every factory is id-addressed and registry-gated at runtime; re-execution policy
    // is runtime data (evictions), never a per-payload flag.
    let mut register_factory_args = oxc::allocator::Vec::with_capacity_in(2, self);
    register_factory_args.push(ast::Argument::new_string_literal(
      SPAN,
      oxc::ast::ast::Str::from_str_in(&self.module.stable_id, self),
      None,
      self,
    ));
    register_factory_args
      .push(ast::Argument::from(ast::Expression::FunctionExpression(user_code_wrapper)));

    let register_factory_call = ast::Expression::new_call_expression(
      SPAN,
      Expression::new_identifier(SPAN, "__rolldown_runtime__.registerFactory", self),
      None,
      register_factory_args,
      false,
      self,
    );

    node.body.push(ast::Statement::new_expression_statement(SPAN, register_factory_call, self));
  }

  fn enter_call_expression(
    &mut self,
    node: &mut ast::CallExpression<'ast>,
    _ctx: &mut oxc_traverse::TraverseCtx<'ast, ()>,
  ) {
    self.rewrite_hot_accept_call_deps(node);
  }

  fn exit_expression(
    &mut self,
    node: &mut oxc::ast::ast::Expression<'ast>,
    ctx: &mut oxc_traverse::TraverseCtx<'ast, ()>,
  ) {
    // Rewrite top-level `this` to `exports` for CommonJS modules
    // Use `this_expr_replace_map` from scanning to avoid rewriting `this` inside classes
    if let ast::Expression::ThisExpression(this_expr) = node
      && self.module.exports_kind.is_commonjs()
      && self.module.ecma_view.this_expr_replace_map.contains_key(&this_expr.node_id())
    {
      *node = Expression::new_id_ref_expr(SPAN, CJS_ROLLDOWN_EXPORTS_REF, self);
      return;
    }

    self.try_rewrite_dynamic_import(node);
    self.try_rewrite_require(node, ctx);
    self.rewrite_import_meta_hot(node);
    if !self.exported_bindings.is_empty() {
      let mut writes = ExportBindingWrites {
        scoping: ctx.scoping(),
        exported: &self.exported_bindings,
        symbols: FxHashSet::default(),
      };
      match node {
        Expression::AssignmentExpression(assignment) => {
          writes.visit_assignment_target(&assignment.left);
        }
        Expression::UpdateExpression(update) => {
          writes.visit_simple_assignment_target(&update.argument);
        }
        _ => {}
      }
      if !writes.symbols.is_empty() {
        *node = self.export_notification(node.take_in(self), &writes.symbols, false);
      }
    }
  }

  fn exit_statement(
    &mut self,
    statement: &mut ast::Statement<'ast>,
    _ctx: &mut oxc_traverse::TraverseCtx<'ast, ()>,
  ) {
    // A block does not change var scope. Lexical declarations are instead published
    // after their original top-level statement, preserving their lexical scope.
    if let ast::Statement::VariableDeclaration(declaration) = statement
      && declaration.kind == ast::VariableDeclarationKind::Var
      && !self.exported_bindings.is_empty()
    {
      let (symbols, anonymous_default) = self.declaration_export_bindings(statement);
      if !symbols.is_empty() || anonymous_default {
        *statement = ast::Statement::new_block_statement(
          SPAN,
          [
            statement.take_in(self),
            ast::Statement::new_expression_statement(
              SPAN,
              self.export_notification(
                Expression::new_void_0(SPAN, self),
                &symbols,
                anonymous_default,
              ),
              self,
            ),
          ],
          self,
        );
      }
    }
  }

  fn exit_for_statement(
    &mut self,
    node: &mut ast::ForStatement<'ast>,
    ctx: &mut oxc_traverse::TraverseCtx<'ast, ()>,
  ) {
    if !self.exported_bindings.is_empty()
      && let Some(ast::ForStatementInit::VariableDeclaration(declaration)) = &node.init
      && declaration.kind == ast::VariableDeclarationKind::Var
    {
      let mut writes = ExportBindingWrites {
        scoping: ctx.scoping(),
        exported: &self.exported_bindings,
        symbols: FxHashSet::default(),
      };
      for declarator in &declaration.declarations {
        writes.visit_binding_pattern(&declarator.id);
      }
      if !writes.symbols.is_empty() {
        // Publish after the var initializer, before the first condition reads exports.
        // A comma prefix also preserves an omitted condition as an always-true test.
        let test =
          node.test.take().unwrap_or_else(|| Expression::new_boolean_literal(SPAN, true, self));
        node.test = Some(Expression::new_sequence_expression(
          SPAN,
          [
            self.export_notification(Expression::new_void_0(SPAN, self), &writes.symbols, false),
            test,
          ],
          self,
        ));
      }
    }
  }

  fn exit_for_in_statement(
    &mut self,
    node: &mut ast::ForInStatement<'ast>,
    ctx: &mut oxc_traverse::TraverseCtx<'ast, ()>,
  ) {
    self.notify_loop_export_writes(&node.left, &mut node.body, ctx);
  }

  fn exit_for_of_statement(
    &mut self,
    node: &mut ast::ForOfStatement<'ast>,
    ctx: &mut oxc_traverse::TraverseCtx<'ast, ()>,
  ) {
    self.notify_loop_export_writes(&node.left, &mut node.body, ctx);
  }

  fn exit_identifier_reference(
    &mut self,
    node: &mut ast::IdentifierReference<'ast>,
    ctx: &mut oxc_traverse::TraverseCtx<'ast, ()>,
  ) {
    self.rewrite_identifier_reference(node, ctx);
  }
}

impl<'ast> HmrAstFinalizer<'_, 'ast> {
  fn declaration_export_bindings(
    &self,
    statement: &ast::Statement<'ast>,
  ) -> (FxHashSet<oxc::semantic::SymbolId>, bool) {
    let identifiers = match statement {
      ast::Statement::VariableDeclaration(declaration) => declaration
        .declarations
        .iter()
        .flat_map(|declarator| declarator.id.get_binding_identifiers())
        .collect::<Vec<_>>(),
      ast::Statement::ClassDeclaration(class) => class.id.iter().collect::<Vec<_>>(),
      _ => return (FxHashSet::default(), false),
    };
    let mut symbols = FxHashSet::default();
    let mut anonymous_default = false;
    for id in identifiers {
      if let Some(symbol) = id.symbol_id.get() {
        if self.exported_bindings.contains(&symbol) {
          symbols.insert(symbol);
        }
      } else if id.name == "__rolldown_default__" && !self.exported_bindings.is_empty() {
        anonymous_default = true;
      }
    }
    (symbols, anonymous_default)
  }

  fn export_notification(
    &self,
    value: Expression<'ast>,
    symbols: &FxHashSet<oxc::semantic::SymbolId>,
    anonymous_default: bool,
  ) -> Expression<'ast> {
    let mut names = self
      .module
      .named_exports
      .iter()
      .filter_map(|(name, export)| {
        (symbols.contains(&export.referenced.symbol) || (anonymous_default && name == "default"))
          .then_some(name.as_str())
      })
      .collect::<Vec<_>>();
    names.sort_unstable();
    let names = ast::Argument::new_array_expression(
      SPAN,
      oxc::allocator::Vec::from_iter_in(
        names.into_iter().map(|name| {
          ast::ArrayExpressionElement::from(Expression::new_string_literal(
            SPAN,
            ast::Str::from_str_in(name, self),
            None,
            self,
          ))
        }),
        self,
      ),
      self,
    );
    Expression::new_call_expression(
      SPAN,
      Expression::new_member_access_expr("__rolldown_runtime__", "notifyExports", self),
      None,
      [
        ast::Argument::new_identifier(SPAN, MODULE_ID_PARAM_FOR_HMR, self),
        ast::Argument::from(value),
        names,
      ],
      false,
      self,
    )
  }

  fn notify_loop_export_writes(
    &self,
    left: &ast::ForStatementLeft<'ast>,
    body: &mut ast::Statement<'ast>,
    ctx: &oxc_traverse::TraverseCtx<'ast, ()>,
  ) {
    if self.exported_bindings.is_empty() {
      return;
    }
    let mut writes = ExportBindingWrites {
      scoping: ctx.scoping(),
      exported: &self.exported_bindings,
      symbols: FxHashSet::default(),
    };
    writes.visit_for_statement_left(left);
    if !writes.symbols.is_empty() {
      let notification = ast::Statement::new_expression_statement(
        SPAN,
        self.export_notification(Expression::new_void_0(SPAN, self), &writes.symbols, false),
        self,
      );
      if let ast::Statement::BlockStatement(block) = body {
        block.body.insert(0, notification);
      } else {
        *body = ast::Statement::new_block_statement(SPAN, [notification, body.take_in(self)], self);
      }
    }
  }

  /// Rewrite a bare `exports` / `module` identifier to the wrapper-parameter
  /// name (`__rolldown_exports__` / `__rolldown_module__`), or an import-binding
  /// identifier to its generated binding name.
  fn rewrite_identifier_reference(
    &self,
    ident: &mut ast::IdentifierReference<'ast>,
    ctx: &oxc_traverse::TraverseCtx<'ast, ()>,
  ) {
    let Some(reference_id) = ident.reference_id.get() else {
      return;
    };
    let reference = ctx.scoping().get_reference(reference_id);
    if let Some(symbol_id) = reference.symbol_id() {
      if let Some(binding_name) = self.import_bindings.get(&symbol_id) {
        ident.name = oxc::ast::ast::Str::from_str_in(binding_name.as_str(), self).into();
      }
    } else if ident.name == CJS_EXPORTS_REF_STR {
      ident.name = CJS_ROLLDOWN_EXPORTS_REF_IDENT;
    } else if ident.name == CJS_MODULE_REF_STR {
      ident.name = CJS_ROLLDOWN_MODULE_REF_IDENT;
    }
  }
}
