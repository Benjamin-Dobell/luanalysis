//! Original global declaration recipes. Native queries own type selection,
//! compatibility, recursion and caching; this index retains no computed types.
use super::NativeRegistry;
use crate::bind::{BoundFile, OccurrenceKind};
use crate::{Checker, Diagnostic};
use luanalysis_engine::{
    GlobalQuery, GlobalQueryError, GlobalSourceRole, GlobalValueKind, NativeEnvironmentKey,
    NativeExpressionKey, NativeFunctionKey, NativeGlobalBindingKey, NativeTypeDeclarationKey,
    NativeTypeProvider, NativeValueDeclarationKey, PackFunctionType, Type, TypeQueries,
    ValueDeclarationHeader, ValuePack,
};
use luanalysis_project::SourceRole;
use luanalysis_syntax::{ExprKind, File, Name, Span, StmtKind, TypeExpr};
use std::collections::{BTreeMap, HashMap, HashSet};

#[path = "native_namespace_members.rs"]
mod namespace;
use namespace::NamespaceTarget;
#[path = "native_ambient_data.rs"]
mod data;

pub(super) struct GlobalDeclarationSource {
    pub key: NativeValueDeclarationKey,
    pub name: Name,
    pub statement: Span,
    pub annotation: Option<TypeExpr>,
    pub function: Option<NativeFunctionKey>,
    pub initializer: Option<NativeExpressionKey>,
    pub expanded_initializer: bool,
    pub namespace: Option<NamespaceTarget>,
}

pub(super) struct NativeGlobals {
    pub declarations: Vec<GlobalDeclarationSource>,
    unsupported: Vec<Span>,
}

impl NativeGlobals {
    pub(super) fn member_namespace_end(&self, start: u32) -> u32 {
        self.declarations.iter().filter_map(|source| source.namespace.as_ref())
            .map(|target| target.member.index.checked_add(1).expect("namespace member identities fit u32"))
            .max().unwrap_or(start)
    }

    pub(super) fn new(
        snapshot: u64,
        file: &File,
        bound: &BoundFile,
        functions: &HashMap<(usize, usize), u32>,
        expressions: &super::NativeExpressions,
        member_start: u32,
    ) -> Self {
        let mut result = Self {
            declarations: Vec::new(),
            unsupported: Vec::new(),
        };
        for statement in &file.statements {
            match &statement.kind {
                StmtKind::Assign {
                    targets,
                    values,
                    annotation,
                } => {
                    for (index, target) in targets.iter().enumerate() {
                        let Some((name, namespace)) = namespace::declaration_target(
                            target,
                            snapshot,
                            member_start,
                            result.declarations.iter().filter(|source| source.namespace.is_some()).count(),
                        ) else {
                            result.unsupported.push(target.span);
                            continue;
                        };
                        let root = namespace.as_ref().map_or(&name, |target| &target.root);
                        if bound.binding_at(root.span.start).is_some()
                            || matches!(root.text.as_str(), "_G" | "_ENV")
                            || nonstandard_environment(bound, root.span.start)
                        {
                            result.unsupported.push(target.span);
                            continue;
                        }
                        // Nonfinal scalar demands retain their original RHS. A
                        // final expanded assignment needs a separate projection
                        // recipe; never pretend it is another scalar expression.
                        let initializer = values
                            .get(index)
                            .map(|value| expressions.key_for_span(value.span));
                        let key = NativeValueDeclarationKey {
                            snapshot,
                            index: result.declarations.len() as u32,
                        };
                        result.declarations.push(GlobalDeclarationSource {
                            key,
                            name,
                            statement: statement.span,
                            annotation: annotation
                                .as_ref()
                                .and_then(|annotation| annotation.types.get(index))
                                .cloned(),
                            function: None,
                            initializer,
                            expanded_initializer: index >= values.len()
                                && values.last().is_some_and(|value| {
                                    matches!(
                                        value.kind,
                                        ExprKind::Call { .. }
                                            | ExprKind::MethodCall { .. }
                                            | ExprKind::Vararg
                                    )
                                }),
                            namespace,
                        });
                    }
                }
                StmtKind::FunctionDeclaration {
                    target,
                    function,
                    ..
                } => {
                    let Some((name, namespace)) = namespace::declaration_target(
                        target,
                        snapshot,
                        member_start,
                        result
                            .declarations
                            .iter()
                            .filter(|source| source.namespace.is_some())
                            .count(),
                    ) else {
                        result.unsupported.push(target.span);
                        continue;
                    };
                    let root = namespace.as_ref().map_or(&name, |target| &target.root);
                    if bound.binding_at(root.span.start).is_some()
                        || matches!(root.text.as_str(), "_G" | "_ENV")
                        || nonstandard_environment(bound, root.span.start)
                    {
                        result.unsupported.push(target.span);
                        continue;
                    }
                    let key = NativeValueDeclarationKey {
                        snapshot,
                        index: result.declarations.len() as u32,
                    };
                    result.declarations.push(GlobalDeclarationSource {
                        key,
                        name,
                        statement: statement.span,
                        annotation: None,
                        function: Some(NativeFunctionKey {
                            snapshot,
                            index: functions[&(function.span.start, function.span.end)],
                        }),
                        initializer: None,
                        expanded_initializer: false,
                        namespace,
                    });
                }
                _ => result.unsupported.push(statement.span),
            }
        }
        result
    }

    fn declaration(&self, key: NativeValueDeclarationKey) -> Option<&GlobalDeclarationSource> {
        self.declarations
            .get(key.index as usize)
            .filter(|source| source.key == key)
    }
}

pub(crate) struct ProjectGlobalIndex {
    pub environment: NativeEnvironmentKey,
    names: BTreeMap<String, NativeGlobalBindingKey>,
    declarations: Vec<Vec<NativeValueDeclarationKey>>,
}

impl ProjectGlobalIndex {
    pub(super) fn new(environment: NativeEnvironmentKey, sources: &[&NativeRegistry]) -> Self {
        let mut by_name = BTreeMap::<String, Vec<NativeValueDeclarationKey>>::new();
        for source in sources {
            let mut introduced = HashSet::new();
            for declaration in &source.globals.declarations {
                if declaration.namespace.is_some() {
                    continue;
                }
                let explicit = source.is_global_contract_owner(declaration);
                let first = introduced.insert(&declaration.name.text);
                if !explicit && !first {
                    continue;
                }
                by_name
                    .entry(declaration.name.text.clone())
                    .or_default()
                    .push(declaration.key);
            }
        }
        let mut names = BTreeMap::new();
        let mut declarations = Vec::new();
        for (name, mut owners) in by_name {
            owners.sort_by_key(|key| (key.snapshot, key.index));
            names.insert(
                name,
                NativeGlobalBindingKey {
                    environment,
                    index: declarations.len() as u32,
                },
            );
            declarations.push(owners);
        }
        Self {
            environment,
            names,
            declarations,
        }
    }

    pub(super) fn binding(&self, name: &str) -> Option<NativeGlobalBindingKey> {
        self.names.get(name).copied()
    }
    pub(super) fn contains(&self, key: NativeGlobalBindingKey) -> bool {
        key.environment == self.environment && (key.index as usize) < self.declarations.len()
    }
    pub(super) fn declarations(
        &self,
        key: NativeGlobalBindingKey,
    ) -> Vec<NativeValueDeclarationKey> {
        if !self.contains(key) {
            return Vec::new();
        }
        self.declarations[key.index as usize].clone()
    }
}

impl NativeRegistry {
    fn is_global_contract_owner(&self, source: &GlobalDeclarationSource) -> bool {
        self.source_role() == SourceRole::Declaration
            || source.annotation.is_some()
            || self.global_attached_type(source).is_some()
            || source.function.is_some_and(|key| {
                let annotations = &self.function(key).annotations;
                annotations.type_annotation.is_some()
                    || !annotations.params.is_empty()
                    || annotations.returns.is_some()
                    || annotations.vararg.is_some()
            })
    }
    pub(super) fn global_binding(&self, name: &str) -> Option<NativeGlobalBindingKey> {
        self.project_globals.as_ref()?.binding(name)
    }
    pub(super) fn contains_global_declaration(&self, key: NativeValueDeclarationKey) -> bool {
        self.globals.declaration(key).is_some()
    }
    pub(super) fn global_header(&self, key: NativeValueDeclarationKey) -> ValueDeclarationHeader {
        let source = self
            .globals
            .declaration(key)
            .expect("original global declaration");
        ValueDeclarationHeader {
            role: match self.source_role() {
                SourceRole::Implementation => GlobalSourceRole::Implementation,
                SourceRole::Declaration => GlobalSourceRole::Declaration,
            },
            kind: if source.namespace.is_some() {
                GlobalValueKind::Member
            } else if source.function.is_some() {
                GlobalValueKind::Function
            } else {
                GlobalValueKind::Variable
            },
        }
    }
    fn global_attached_type(
        &self,
        source: &GlobalDeclarationSource,
    ) -> Option<NativeTypeDeclarationKey> {
        let mut attached = self
            .named
            .declarations()
            .iter()
            .filter(|named| named.syntax.attachment == Some(source.statement));
        let declaration = attached.next()?;
        if attached.next().is_some() || !declaration.syntax.generics.is_empty() {
            return None;
        }
        Some(NativeTypeDeclarationKey {
            snapshot: declaration.key.snapshot,
            index: declaration.key.index,
        })
    }
    pub(super) fn is_ambient_global_attachment(&self, span: Span) -> bool {
        self.source_role() == SourceRole::Declaration
            && self
                .globals
                .declarations
                .iter()
                .any(|source| source.statement == span)
    }
    pub(super) fn global_contract(
        &self,
        key: NativeValueDeclarationKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        let source = self.globals.declaration(key)?;
        if let Some(annotation) = &source.annotation {
            let mut checker = self.checker(queries);
            let ty = checker.annotation(annotation);
            self.merge_report(checker.report);
            return Some(ty);
        }
        if let Some(function) = source.function {
            let original = self.function(function);
            if self.source_role() == SourceRole::Implementation {
                let complete = original.annotations.type_annotation.is_some()
                    || (self.declared_pack_recipe(function).is_some()
                        && original.params.iter().all(|parameter| {
                            original
                                .annotations
                                .params
                                .iter()
                                .any(|annotation| annotation.name.text == parameter.text)
                        })
                        && (original.vararg.is_none() || original.annotations.vararg.is_some()));
                return complete.then_some(Type::NativeFunction(function));
            }
            if self.source_role() == SourceRole::Declaration {
                return self.ambient_function_contract(function, queries);
            }
            return None;
        }
        if let Some(declaration) = self.global_attached_type(source) {
            return queries.named_declared_type(declaration).ok();
        }
        if self.source_role() == SourceRole::Declaration {
            return source.initializer.and_then(|expression| {
                data::literal_contract(&self.expressions.get(expression).expression)
            });
        }
        None
    }
    /// One pure original declaration contract for global and local namespace
    /// members. Placeholder values and bodies never supply inferred evidence.
    pub(crate) fn ambient_function_contract(
        &self,
        function: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        assert_eq!(self.source_role(), SourceRole::Declaration);
        let original = self.function(function);
        let heads = self.function_overload_signatures(function);
        if !heads.is_empty() {
            self.validate_ambient_function_annotations(function, queries);
            return Some(queries.signature_group_type(&heads));
        }
        if let Some(annotation) = original.annotations.type_annotation.as_ref()
            .and_then(|annotation| annotation.types.first())
        {
            self.metadata(function, queries);
            let mut checker = self.checker(queries);
            checker.type_context = Some(function);
            let ty = checker.annotation(annotation);
            self.merge_report(checker.report);
            if matches!(ty, Type::RuntimeFunction) { return Some(ty) }
            if let Some(head) = self.implementation_annotation_signature(function) {
                return Some(queries.signature_group_type(&[head]));
            }
            return Some(ty);
        }
        let params = self.metadata(function, queries).0;
        // This syntax convention does not infer executable placeholder code.
        let raw_returns = self.declared_pack_contract(function, queries)
            .or_else(|| original.body.is_empty().then(ValuePack::empty))?;
        let returns = queries.declared_result_pack(&raw_returns);
        let returns = queries.pack_with_implicit_empty(&returns);
        let rest = self.source_rest_pack(function, queries);
        let type_parameters = self.function_type_parameters(function, queries);
        Some(queries.pack_function(&PackFunctionType { params, rest, type_parameters, returns }))
    }
    pub(super) fn global_initializer(
        &self,
        key: NativeValueDeclarationKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        let source = self
            .globals
            .declaration(key)
            .expect("original global initializer");
        assert_eq!(
            self.source_role(),
            SourceRole::Implementation,
            "ambient placeholders cannot be inferred"
        );
        if let Some(function) = source.function {
            return Type::NativeFunction(function);
        }
        let Some(expression) = source.initializer else {
            if source.expanded_initializer {
                self.report.borrow_mut().diagnostics.push(Diagnostic {
                    code: "LUA2035", span: source.name.span,
                    message: "Global inference from a later expanded result slot requires an original pack-projection declaration recipe.".into(),
                });
                return Type::Any;
            }
            return Type::Nil;
        };
        let mut checker = self.checker_for_expression(
            expression,
            luanalysis_engine::NativeCheckMode::empty(),
            queries,
        );
        let ty = checker.expression(&self.expressions.get(expression).expression);
        self.merge_report(checker.report);
        ty
    }
    pub(super) fn global_error(&self, key: NativeValueDeclarationKey, error: &GlobalQueryError) {
        let Some(source) = self.globals.declaration(key) else {
            return;
        };
        let (code, message) = match error {
            GlobalQueryError::InvalidPrimitiveMemberNamespace { .. } => (
                "LUA2035",
                format!("Selected primitive string namespace global '{}' must declare an original table or object contract.", source.name.text),
            ),
            GlobalQueryError::MissingDeclarationContract(_) => (
                "LUA2035",
                format!(
                    "Declaration global '{}' requires a written type, complete function contract, or literal data declaration; executable placeholder initializers and bodies are not inferred.",
                    source.name.text
                ),
            ),
            GlobalQueryError::ConflictingDeclarations { .. } => (
                "TS2403",
                format!(
                    "Global '{}' has incompatible declaration owners; every conflicting original declaration is reported.",
                    source.name.text
                ),
            ),
            GlobalQueryError::CircularQuery(_) => (
                "TS7022",
                format!(
                    "Global '{}' has a circular declaration query.",
                    source.name.text
                ),
            ),
            GlobalQueryError::TooDeep(_) => (
                "TS2589",
                format!(
                    "Global '{}' declaration query is excessively deep.",
                    source.name.text
                ),
            ),
            _ => (
                "LUA2035",
                format!(
                    "Global '{}' declaration query failed: {error:?}.",
                    source.name.text
                ),
            ),
        };
        self.report.borrow_mut().diagnostics.push(Diagnostic {
            code,
            span: source.name.span,
            message,
        });
    }
    pub(crate) fn check_global_declarations(&self, queries: &mut dyn TypeQueries) {
        if self.source_role() == SourceRole::Declaration {
            self.check_signature_declarations(queries);
        }
        for source in &self.globals.declarations {
            if let Some(function) = source.function {
                for parameter in self.function_type_parameters(function, queries) {
                    queries.type_parameter_constraint(parameter);
                }
            }
            if source.namespace.is_some() {
                if self.source_role() == SourceRole::Declaration {
                    self.check_namespace_declaration(source, queries);
                }
                continue;
            }
            // Demand each selected binding, including declarations never read by
            // implementation code. The backend owns duplicate compatibility.
            if let Some(binding) = self.global_binding(&source.name.text) {
                let _ = queries.global_binding_type(binding);
            }
        }
        if self.source_role() == SourceRole::Declaration {
            for &span in &self.globals.unsupported {
                if self.check_ambient_local_statement(span, queries) { continue; }
                self.report.borrow_mut().diagnostics.push(Diagnostic { code: "LUA2034", span, message: "This executable statement is unsupported in a declaration library; use original global type/function contracts or standalone type declarations.".into() });
            }
        }
    }
    fn validate_ambient_function_annotations(
        &self,
        function: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) {
        // The written heads are the public schema. Check finite written tags
        // without creating an implementation signature or diagnosing untyped
        // placeholder parameters that are not public declaration parameters.
        let source = self.function(function);
        let mut checker = self.checker(queries);
        checker.type_context = Some(function);
        let mut names = HashSet::new();
        for parameter in &source.annotations.params {
            if !names.insert(&parameter.name.text)
                || (!(parameter.name.text == "self" && self.implicit_receiver(source).is_some()) && !source
                    .params
                    .iter()
                    .any(|name| name.text == parameter.name.text))
            {
                checker.error(
                    "LUA2011",
                    parameter.name.span,
                    "Parameter annotation has a duplicate or unknown original parameter owner.",
                );
            }
            checker.annotation(&parameter.ty);
        }
        if let Some(vararg) = &source.annotations.vararg {
            checker.annotation(&vararg.ty);
        }
        for annotation in source
            .annotations
            .returns
            .iter()
            .chain(source.annotations.type_annotation.iter())
        {
            for ty in &annotation.types {
                checker.annotation(ty);
            }
        }
        self.merge_report(checker.report);
    }
}

pub(super) fn query_declarations(error: &GlobalQueryError) -> Vec<NativeValueDeclarationKey> {
    match error {
        GlobalQueryError::InvalidDeclaration(key)
        | GlobalQueryError::MissingDeclarationContract(key) => vec![*key],
        GlobalQueryError::ConflictingDeclarations { declarations, .. } => declarations.clone(),
        GlobalQueryError::CircularQuery(query)
        | GlobalQueryError::TooDeep(query)
        | GlobalQueryError::UnresolvedQuery(query) => match query {
            GlobalQuery::ValueDeclaration(key) | GlobalQuery::ValueDeclarationContract(key) => {
                vec![*key]
            }
            GlobalQuery::GlobalBinding(_) => Vec::new(),
        },
        _ => Vec::new(),
    }
}

impl Checker<'_> {
    pub(crate) fn is_global_annotation_target(&self, target: &luanalysis_syntax::Expr) -> bool {
        let ExprKind::Name(name) = &target.kind else {
            return false;
        };
        self.bound.binding_at(name.span.start).is_none()
            && self.native.is_some_and(|native| {
                native
                    .globals
                    .declarations
                    .iter()
                    .any(|source| source.name.span == name.span && source.annotation.is_some())
            })
    }
    pub(crate) fn rejects_generic_global_write(&mut self, name: &Name) -> bool {
        let (Some(native), Some(function)) = (self.native, self.type_context) else {
            return false;
        };
        if native
            .function_type_parameters(function, self.engine)
            .is_empty()
            && native.function_outer_type_parameters(function).is_empty()
        {
            return false;
        }
        self.error("LUA2037", name.span, "Global writes from a generic function require an original environment write contract that prevents declaration type parameters escaping their owner.");
        true
    }
    pub(crate) fn global_read(&mut self, name: &Name) -> Option<Type> {
        let native = self.native?;
        if name.text == "_G"
            || name.text == "_ENV"
            || nonstandard_environment(self.bound, name.span.start)
        {
            self.error("LUA2036", name.span, "Global lookup through an explicit or modified Lua environment requires environment-identity integration.");
            return Some(Type::Any);
        }
        let key = native.global_binding(&name.text)?;
        Some(self.engine.global_binding_type(key).unwrap_or(Type::Any))
    }
}

fn nonstandard_environment(bound: &BoundFile, offset: usize) -> bool {
    if bound.occurrences.iter().any(|occurrence| {
        occurrence.name == "_ENV"
            && occurrence.kind == OccurrenceKind::Write
            && occurrence.binding.is_none()
    }) {
        return true;
    }
    let mut scope = bound.scope_at(offset);
    while let Some(current) = scope {
        let node = bound.scope(current);
        if node.bindings.iter().any(|&binding| {
            let declaration = bound.declaration(binding);
            declaration.name == "_ENV" && declaration.activation <= offset
        }) {
            return true;
        }
        scope = node.parent;
    }
    false
}
