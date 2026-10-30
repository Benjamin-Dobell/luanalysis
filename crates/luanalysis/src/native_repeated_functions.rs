//! Function statements that repeat an earlier one's target: `function a.b() end` twice.
use super::NativeRegistry;
use crate::collections::HashMap;
use crate::{CheckReport, Diagnostic};
use luanalysis_project::SourceRole;
use luanalysis_engine::{NativeFunctionKey, NativeTypeProvider, PackFunctionType, Type, TypeQueries, ValuePack};
use luanalysis_syntax::{Expr, ExprKind, Span, Stmt, StmtKind};

/// A `function a.b()` statement's target: a local's or a global's name and the fields after it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct FunctionStatementTarget<'a> {
    local: Option<crate::bind::BindingId>,
    root: &'a str,
    path: Vec<&'a str>,
}

/// A function statement that defines what an earlier one in the same run of statements did.
pub(crate) struct RepeatedFunction {
    first: NativeFunctionKey,
    later: NativeFunctionKey,
    /// The name it defines, as written: `Mod.f`.
    name: String,
    span: Span,
    global: bool,
    target: Span,
    /// Where the later statement's own function is, which the statement's check reads.
    function: Span,
}

impl NativeRegistry {
    fn function_statement_target<'a>(&self, target: &'a Expr) -> Option<FunctionStatementTarget<'a>> {
        match &target.kind {
            ExprKind::Name(name) => Some(FunctionStatementTarget {
                local: self.bound().binding_at(name.span.start),
                root: &name.text,
                path: Vec::new(),
            }),
            ExprKind::Field { base, name } => {
                let mut target = self.function_statement_target(base)?;
                target.path.push(&name.text);
                Some(target)
            }
            _ => None,
        }
    }

    /// Function statements of one target that all run if any does: in one block, or through a
    /// `do`. A branch, a loop or a function has its own, so defining a function conditionally
    /// repeats nothing, and an assignment to the target (or what holds it) starts it again.
    fn collect_repeated_functions<'a>(&'a self, statements: &'a [Stmt], seen: &mut HashMap<FunctionStatementTarget<'a>, (NativeFunctionKey, bool)>, repeated: &mut Vec<RepeatedFunction>) {
        let mut previous_end = statements.first().map_or(0, |statement| statement.span.start);
        for statement in statements {
            match &statement.kind {
                StmtKind::Block { body, .. } => self.collect_repeated_functions(body, seen, repeated),
                StmtKind::FunctionDeclaration { target, function, .. } => {
                    if let Some(key) = self.function_statement_target(target) {
                        let index = self.functions.iter().position(|other| other.span == function.span)
                            .expect("a function statement's function is indexed");
                        let later = NativeFunctionKey { snapshot: self.snapshot, index: index as u32 };
                        // `---@version` says which Lua each definition is for: an alternative, as a branch is.
                        let versioned = self.comments_between(previous_end, statement.span.start).any(|comment| comment.text.contains("@version"));
                        match seen.get(&key).copied() {
                            Some((first, first_versioned)) if !versioned && !first_versioned => {
                                let span = match &target.kind { ExprKind::Field { name, .. } | ExprKind::Name(name) => name.span, _ => target.span };
                                repeated.push(RepeatedFunction {
                                    first,
                                    later,
                                    name: std::iter::once(key.root).chain(key.path.iter().copied()).collect::<Vec<_>>().join("."),
                                    span,
                                    global: key.local.is_none() && key.path.is_empty(),
                                    target: target.span,
                                    function: function.span,
                                });
                            }
                            Some(_) => {}
                            None => { seen.insert(key, (later, versioned)); }
                        }
                    }
                }
                StmtKind::Assign { targets, .. } => {
                    for target in targets {
                        let Some(written) = self.function_statement_target(target) else { continue };
                        seen.retain(|key, _| !(key.local == written.local && key.root == written.root && key.path.starts_with(&written.path)));
                    }
                }
                StmtKind::If { branches, else_branch } => {
                    for branch in branches { self.collect_repeated_functions(&branch.body, &mut HashMap::default(), repeated); }
                    if let Some(branch) = else_branch { self.collect_repeated_functions(&branch.body, &mut HashMap::default(), repeated); }
                }
                StmtKind::While { body, .. } | StmtKind::Repeat { body, .. } | StmtKind::NumericFor { body, .. } | StmtKind::GenericFor { body, .. } => {
                    self.collect_repeated_functions(body, &mut HashMap::default(), repeated);
                }
                _ => {}
            }
            previous_end = statement.span.end;
        }
    }

    fn comments_between(&self, start: usize, end: usize) -> impl Iterator<Item = &luanalysis_syntax::Comment> {
        self.file.comments.iter().filter(move |comment| comment.span.start >= start && comment.span.end <= end)
    }

    pub(crate) fn repeated_functions(&self) -> &[RepeatedFunction] {
        self.repeated_functions.get_or_init(|| {
            let mut repeated = Vec::new();
            self.collect_repeated_functions(self.statements(), &mut HashMap::default(), &mut repeated);
            for function in self.functions.iter() {
                self.collect_repeated_functions(&function.body, &mut HashMap::default(), &mut repeated);
            }
            repeated
        })
    }

    /// Whether this function is a later statement's, which `report_repeated_functions` compares
    /// with the first: it is not also a write that the first's contract checks.
    pub(crate) fn repeats_function(&self, function: Span) -> bool {
        self.repeated_functions().iter().any(|repeated| repeated.function == function)
    }

    /// What a function statement's signature is to compare with another's, as written: what only
    /// one of them writes is not compared, and neither are inferred results.
    fn compared_signatures(&self, first: NativeFunctionKey, later: NativeFunctionKey, queries: &mut dyn TypeQueries) -> (Type, Type) {
        let (first_function, later_function) = (self.function(first), self.function(later));
        let heads = (self.function_overload_signatures(first), self.function_overload_signatures(later));
        if !heads.0.is_empty() || !heads.1.is_empty() {
            let group = |heads: &[_], queries: &mut dyn TypeQueries| if heads.is_empty() { Type::RuntimeFunction } else { queries.signature_group_type(heads) };
            return (group(&heads.0, queries), group(&heads.1, queries));
        }
        let annotated = |function: NativeFunctionKey, queries: &mut dyn TypeQueries| {
            let annotation = self.function(function).annotations.type_annotation.as_ref()?.types.first()?;
            let mut checker = self.checker(queries);
            checker.type_context = Some(function);
            let ty = checker.annotation(annotation);
            self.merge_report(checker.report);
            Some(ty)
        };
        match (annotated(first, queries), annotated(later, queries)) {
            (Some(first), Some(later)) => return (first.clone(), later),
            (Some(_), None) | (None, Some(_)) => return (Type::RuntimeFunction, Type::RuntimeFunction),
            (None, None) => {}
        }
        let written = |function: &luanalysis_syntax::Function, name: &str| function.annotations.params.iter().any(|param| param.name.text == name);
        let (mut first_params, mut later_params) = (self.metadata(first, queries).0, self.metadata(later, queries).0);
        for (one, other) in first_params.iter_mut().zip(later_params.iter_mut()) {
            if written(first_function, &one.name) && written(later_function, &other.name) || queries.is_identical(&one.ty, &other.ty) { continue; }
            (one.ty, other.ty) = (Type::Any, Type::Any);
            (one.optional, other.optional) = (false, false);
        }
        let result = |function: NativeFunctionKey, queries: &mut dyn TypeQueries| {
            let returns = self.declared_pack_contract(function, queries)?;
            let returns = queries.declared_result_pack(&returns);
            Some(queries.pack_with_implicit_empty(&returns))
        };
        let (first_returns, later_returns) = match (result(first, queries), result(later, queries)) {
            (Some(first), Some(later)) => (first, later),
            _ => (ValuePack::empty(), ValuePack::empty()),
        };
        let rest = |function: NativeFunctionKey, compared: bool, queries: &mut dyn TypeQueries| {
            let rest = self.source_rest_pack(function, queries)?;
            Some(if compared { rest } else { ValuePack::variadic(Type::Any) })
        };
        let compared = first_function.annotations.vararg.is_some() && later_function.annotations.vararg.is_some();
        let first_type = PackFunctionType { params: first_params, rest: rest(first, compared, queries), type_parameters: self.function_type_parameters(first, queries), returns: first_returns };
        let later_type = PackFunctionType { params: later_params, rest: rest(later, compared, queries), type_parameters: self.function_type_parameters(later, queries), returns: later_returns };
        (queries.pack_function(&first_type), queries.pack_function(&later_type))
    }

    /// A function statement repeating an earlier one's target unconditionally must have its
    /// signature (TS2717, as a field declared again; TS2403 for a global). With the same one it
    /// is only redundant, which an editor shows (LUA4006). An assignment is neither: it is
    /// checked against the target's type like any other.
    pub(crate) fn report_repeated_functions(&self, queries: &mut dyn TypeQueries) {
        for repeated in self.repeated_functions() {
            // A declaration file reports what it cannot declare, and a global's declarations there
            // report its repeats themselves.
            if self.source_role() == SourceRole::Declaration && self.globals.unsupported.contains(&repeated.target) { continue; }
            let (first, later) = self.compared_signatures(repeated.first, repeated.later, queries);
            if queries.is_identical(&first, &later) {
                if self.inspections() {
                    self.merge_report(CheckReport { diagnostics: vec![Diagnostic {
                        code: "LUA4006",
                        span: repeated.span,
                        message: format!("'{}' is defined again here, which replaces the earlier definition.", repeated.name),
                    }], ..CheckReport::default() });
                }
            } else if repeated.global && self.source_role() == SourceRole::Declaration {
            } else if repeated.global {
                // As the global's own declarations report it: this is that diagnostic if they do.
                self.merge_report(CheckReport { diagnostics: vec![Diagnostic {
                    code: "TS2403",
                    span: repeated.span,
                    message: format!("Global '{}' has incompatible declaration owners; every conflicting original declaration is reported.", repeated.name),
                }], ..CheckReport::default() });
            } else {
                let name = repeated.name.rsplit('.').next().unwrap_or(&repeated.name);
                let template = format!("Subsequent property declarations must have the same type. Property '{name}' must be of type '{{}}', but here has type '{{}}'.");
                self.report.borrow_mut().push_type_diagnostic("TS2717", repeated.span, &template, &[&first, &later]);
            }
        }
    }
}
