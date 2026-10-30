//! Original global declaration recipes. Native queries own type selection,
//! compatibility, recursion and caching; this index retains no computed types.
use super::NativeRegistry;
use crate::bind::{BindingId, BoundFile, OccurrenceKind};
use crate::{Checker, Diagnostic};
use luanalysis_engine::{
    GlobalQuery, GlobalQueryError, GlobalSourceRole, GlobalValueKind, LuaKey, NativeEnvironmentKey,
    NativeExpressionKey, NativeFunctionKey, NativeGlobalBindingKey, NativeTypeDeclarationKey,
    NativeTypeProvider, NativeValueDeclarationKey, NativeSignatureKey, PackFunctionType, Type, TypeQueries,
    ValueDeclarationHeader, ValuePack,
};
use luanalysis_project::SourceRole;
use luanalysis_syntax::{Expr, ExprKind, File, Name, Span, Stmt, StmtKind, TypeExpr};
use std::collections::BTreeMap;
use crate::collections::{HashMap, HashSet};

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
    /// An assignment inside a function body or block. It declares the global only where no
    /// file assigns it at top level, as Lua creates a global wherever one is assigned.
    pub nested: bool,
}

pub(super) struct NativeGlobals {
    pub declarations: Vec<GlobalDeclarationSource>,
    pub(super) unsupported: Vec<Span>,
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
                        let target_declaration = match environment_member(target, bound) {
                            Some(name) => Some((name, None)),
                            None => namespace::declaration_target(
                                target,
                                snapshot,
                                member_start,
                                result.declarations.iter().filter(|source| source.namespace.is_some()).count(),
                            ),
                        };
                        let Some((name, namespace)) = target_declaration else {
                            result.unsupported.push(target.span);
                            continue;
                        };
                        let root = namespace.as_ref().map_or(&name, |target| &target.root);
                        // A `_G.name` target's environment was resolved above.
                        let environment_member = namespace.is_none() && !matches!(target.kind, ExprKind::Name(_));
                        if !environment_member && (bound.binding_at(root.span.start).is_some()
                            || root.text == "_ENV"
                            || nonstandard_environment(bound, root.span.start))
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
                            nested: false,
                        });
                    }
                }
                StmtKind::FunctionDeclaration {
                    target,
                    function,
                    ..
                } => {
                    let target_declaration = match environment_member(target, bound) {
                        Some(name) => Some((name, None)),
                        None => namespace::declaration_target(
                            target,
                            snapshot,
                            member_start,
                            result
                                .declarations
                                .iter()
                                .filter(|source| source.namespace.is_some())
                                .count(),
                        ),
                    };
                    let Some((name, namespace)) = target_declaration else {
                        result.unsupported.push(target.span);
                        continue;
                    };
                    let root = namespace.as_ref().map_or(&name, |target| &target.root);
                    // A `function _G.name()` target's environment was resolved above.
                    let environment_member = namespace.is_none() && !matches!(target.kind, ExprKind::Name(_));
                    if !environment_member && (bound.binding_at(root.span.start).is_some()
                        || root.text == "_ENV"
                        || nonstandard_environment(bound, root.span.start))
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
                        nested: false,
                    });
                }
                _ => result.unsupported.push(statement.span),
            }
        }
        result.nested_declarations(snapshot, file, bound, functions, expressions);
        result
    }

    /// Globals assigned inside function bodies and blocks: `name = value` and `function name()`.
    fn nested_declarations(
        &mut self,
        snapshot: u64,
        file: &File,
        bound: &BoundFile,
        functions: &HashMap<(usize, usize), u32>,
        expressions: &super::NativeExpressions,
    ) {
        let mut statements = Vec::new();
        for statement in &file.statements {
            nested_statements(statement, &mut statements);
        }
        for (_, source) in expressions.sources() {
            if let ExprKind::Function(function) = &source.expression.kind {
                for statement in &function.body {
                    statements.push(statement);
                    nested_statements(statement, &mut statements);
                }
            }
        }
        statements.sort_by_key(|statement| (statement.span.start, statement.span.end));
        statements.dedup_by_key(|statement| (statement.span.start, statement.span.end));
        for statement in statements {
            let global = |target: &Expr| match &target.kind {
                ExprKind::Name(name) if bound.binding_at(name.span.start).is_none()
                    && name.text != "_ENV" && !nonstandard_environment(bound, name.span.start) => Some(name.clone()),
                _ => environment_member(target, bound),
            };
            match &statement.kind {
                StmtKind::Assign { targets, values, annotation } => {
                    for (index, target) in targets.iter().enumerate() {
                        let Some(name) = global(target) else { continue };
                        let key = NativeValueDeclarationKey { snapshot, index: self.declarations.len() as u32 };
                        self.declarations.push(GlobalDeclarationSource {
                            key,
                            name,
                            statement: statement.span,
                            annotation: annotation.as_ref().and_then(|annotation| annotation.types.get(index)).cloned(),
                            function: None,
                            initializer: values.get(index).map(|value| expressions.key_for_span(value.span)),
                            expanded_initializer: index >= values.len() && values.last().is_some_and(|value| {
                                matches!(value.kind, ExprKind::Call { .. } | ExprKind::MethodCall { .. } | ExprKind::Vararg)
                            }),
                            namespace: None,
                            nested: true,
                        });
                    }
                }
                StmtKind::FunctionDeclaration { target, function, method_span: None } => {
                    let Some(name) = global(target) else { continue };
                    let key = NativeValueDeclarationKey { snapshot, index: self.declarations.len() as u32 };
                    self.declarations.push(GlobalDeclarationSource {
                        key,
                        name,
                        statement: statement.span,
                        annotation: None,
                        function: Some(NativeFunctionKey { snapshot, index: functions[&(function.span.start, function.span.end)] }),
                        initializer: None,
                        expanded_initializer: false,
                        namespace: None,
                        nested: true,
                    });
                }
                _ => {}
            }
        }
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
    /// Original class heads attached to declaration-library global roots.
    /// These are finite augmentation candidates, never proof of the reached
    /// receiver's class or environment identity.
    named_owners: Vec<Vec<NativeTypeDeclarationKey>>,
    declared_members: Vec<Vec<LuaKey>>,
    declaration_owners: HashSet<NativeValueDeclarationKey>,
    /// Standard names some implementation source may replace (`bind::STANDARD_NAMES`
    /// bits). Declaration files only describe the initial library.
    replaced_standard_names: u8,
    /// Whether some implementation source may mutate the module loader.
    loader_mutated: bool,
}

impl ProjectGlobalIndex {
    pub(super) fn new(environment: NativeEnvironmentKey, sources: &[&dyn GlobalIndexSource]) -> Self {
        let mut by_name = BTreeMap::<String, Vec<NativeValueDeclarationKey>>::new();
        let mut named_by_name = BTreeMap::<String, Vec<NativeTypeDeclarationKey>>::new();
        let mut members_by_name = BTreeMap::<String, Vec<LuaKey>>::new();
        let mut replaced_standard_names = 0;
        let mut loader_mutated = false;
        let mut declaration_owners = HashSet::default();
        // A global assigned at the top level of any file is declared there; assignments inside
        // functions declare only the globals nothing declares at top level.
        let top_level: HashSet<&str> = sources.iter()
            .flat_map(|source| source.index_globals().declarations.iter())
            .filter(|declaration| !declaration.nested && declaration.namespace.is_none())
            .map(|declaration| declaration.name.text.as_str())
            .collect();
        for source in sources {
            if source.index_role() != SourceRole::Declaration {
                let (names, loader) = source.index_environment_replacements();
                replaced_standard_names |= source.index_bound().replaced_standard_names() | names;
                loader_mutated |= loader || !source.index_bound().module_environment_mutations().is_empty();
            }
            let mut introduced = HashSet::default();
            for declaration in &source.index_globals().declarations {
                if declaration.namespace.is_some() || declaration.nested && top_level.contains(declaration.name.text.as_str()) {
                    continue;
                }
                let explicit = global_contract_owner(*source, declaration);
                let first = introduced.insert(&declaration.name.text);
                if !explicit && !first {
                    continue;
                }
                if source.index_role() == SourceRole::Declaration {
                    declaration_owners.insert(declaration.key);
                }
                if source.index_role() == SourceRole::Declaration
                    && let Some(owner) = attached_type(source.index_named(), declaration)
                    && let Some(named) = source.index_named().declarations().iter().find(|named| {
                        named.key.snapshot == owner.snapshot && named.key.index == owner.index
                            && matches!(named.syntax.kind, luanalysis_syntax::NamedTypeDeclarationKind::Record {
                                flavor: luanalysis_syntax::NamedRecordKind::Class, ..
                            })
                    })
                {
                    named_by_name.entry(declaration.name.text.clone()).or_default().push(owner);
                    let members = members_by_name.entry(declaration.name.text.clone()).or_default();
                    for &member in &named.members {
                        let field = &source.index_named().member(member).expect("original declared field").field;
                        let key = match &field.key {
                            luanalysis_syntax::TypeFieldKey::Named(name) => Some(LuaKey::String(name.text.as_bytes().to_vec())),
                            luanalysis_syntax::TypeFieldKey::Indexed(ty) => crate::native_tables::literal_type_key(ty),
                            luanalysis_syntax::TypeFieldKey::Sequence { index } => Some(LuaKey::Integer((*index).try_into().expect("source field index fits i64"))),
                        };
                        if let Some(key) = key { members.push(key); }
                    }
                    members.extend(source.index_globals().declarations.iter().filter_map(|member| {
                        let target = member.namespace.as_ref()?;
                        (target.root.text == declaration.name.text && target.path.len() == 1)
                            .then(|| LuaKey::String(member.name.text.as_bytes().to_vec()))
                    }));
                }
                by_name
                    .entry(declaration.name.text.clone())
                    .or_default()
                    .push(declaration.key);
            }
        }
        let mut names = BTreeMap::new();
        let mut declarations = Vec::new();
        let mut named_owners = Vec::new();
        let mut declared_members = Vec::new();
        for (name, mut owners) in by_name {
            owners.sort_by_key(|key| (key.snapshot, key.index));
            let mut named = named_by_name.remove(&name).unwrap_or_default();
            named.sort_by_key(|key| (key.snapshot, key.index));
            named.dedup();
            declared_members.push(members_by_name.remove(&name).unwrap_or_default());
            names.insert(
                name,
                NativeGlobalBindingKey {
                    environment,
                    index: declarations.len() as u32,
                },
            );
            declarations.push(owners);
            named_owners.push(named);
        }
        Self {
            environment,
            names,
            declarations,
            named_owners,
            declared_members,
            declaration_owners,
            replaced_standard_names,
            loader_mutated,
        }
    }

    pub(super) fn binding(&self, name: &str) -> Option<NativeGlobalBindingKey> {
        self.names.get(name).copied()
    }
    pub(super) fn named_owners(&self, binding: NativeGlobalBindingKey) -> &[NativeTypeDeclarationKey] {
        if !self.contains(binding) { return &[]; }
        &self.named_owners[binding.index as usize]
    }
    pub(super) fn declares_member(&self, binding: NativeGlobalBindingKey, member: &LuaKey) -> bool {
        self.contains(binding) && self.declared_members[binding.index as usize].contains(member)
    }
    pub(super) fn replaced_standard_names(&self) -> u8 {
        self.replaced_standard_names
    }
    pub(super) fn loader_mutated(&self) -> bool {
        self.loader_mutated
    }
    pub(super) fn bindings(&self) -> Vec<(Vec<u8>, NativeGlobalBindingKey)> {
        self.names.iter().map(|(name, key)| (name.as_bytes().to_vec(), *key)).collect()
    }
    pub(super) fn is_declaration_owner(&self, key: NativeValueDeclarationKey) -> bool {
        self.declaration_owners.contains(&key)
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

/// What the project's global index reads of a source: its recipes and syntax, which a project
/// reads of every source, and never a checking session's state. A source's registry gives it, as
/// does a source whose registry the project has not built (`ProjectRegistry`).
pub(super) trait GlobalIndexSource {
    fn index_globals(&self) -> &NativeGlobals;
    fn index_role(&self) -> SourceRole;
    fn index_environment_replacements(&self) -> (u8, bool);
    fn index_bound(&self) -> &BoundFile;
    fn index_named(&self) -> &crate::native_named::NativeNamed;
    fn index_function(&self, key: NativeFunctionKey) -> &luanalysis_syntax::Function;
}

impl GlobalIndexSource for NativeRegistry {
    fn index_globals(&self) -> &NativeGlobals { &self.globals }
    fn index_role(&self) -> SourceRole { self.source_role() }
    fn index_environment_replacements(&self) -> (u8, bool) { self.flow.environment_replacements() }
    fn index_bound(&self) -> &BoundFile { &self.bound }
    fn index_named(&self) -> &crate::native_named::NativeNamed { &self.named }
    fn index_function(&self, key: NativeFunctionKey) -> &luanalysis_syntax::Function { self.function(key) }
}

/// Whether a global declaration's source writes its contract, rather than assigning a value.
fn global_contract_owner(owner: &dyn GlobalIndexSource, source: &GlobalDeclarationSource) -> bool {
    owner.index_role() == SourceRole::Declaration
        || source.annotation.is_some()
        || attached_type(owner.index_named(), source).is_some()
        || source.function.is_some_and(|key| {
            let annotations = &owner.index_function(key).annotations;
            annotations.type_annotation.is_some()
                || !annotations.params.is_empty()
                || annotations.returns.is_some()
                || annotations.vararg.is_some()
        })
}

/// The class or shape a global declaration's statement attaches, as its type.
fn attached_type(named: &crate::native_named::NativeNamed, source: &GlobalDeclarationSource) -> Option<NativeTypeDeclarationKey> {
    // A class or shape names the table the statement assigns. An alias, as `---@enum X`
    // above `X = { ... }` is (`typeof X[keyof typeof X]`), is a type of its own, as above a
    // local; taking it as the global's type would make the global its own type.
    let mut attached = named
        .declarations()
        .iter()
        .filter(|named| named.syntax.attachment == Some(source.statement)
            && !matches!(named.syntax.kind, luanalysis_syntax::NamedTypeDeclarationKind::Alias { .. }));
    let declaration = attached.next()?;
    if attached.next().is_some() || !declaration.syntax.generics.is_empty() {
        return None;
    }
    Some(NativeTypeDeclarationKey {
        snapshot: declaration.key.snapshot,
        index: declaration.key.index,
    })
}

impl NativeRegistry {
    /// The required parameters of the function a global declaration declares (and whether it
    /// takes more), from its signature alone: no function body or value is inferred.
    pub(super) fn global_function_parameters(&self, key: NativeValueDeclarationKey, queries: &mut dyn TypeQueries) -> Option<(Vec<String>, bool)> {
        let source = self.globals.declaration(key)?;
        let function = source.function?;
        let (parameters, variadic) = (self.metadata(function, queries).0, self.function(function).vararg.is_some());
        let required: Vec<String> = parameters.iter().take_while(|parameter| !parameter.optional).map(|parameter| parameter.name.clone()).collect();
        let more = variadic || parameters.len() > required.len();
        Some((required, more))
    }
    pub(crate) fn global_binding(&self, name: &str) -> Option<NativeGlobalBindingKey> {
        self.project_globals.as_ref()?.binding(name)
    }
    /// The original assignments that can own an implementation global table.
    /// This exposes source identities without demanding its inferred type.
    pub(crate) fn original_global_initializers(&self) -> impl Iterator<Item = (&str, Span, NativeExpressionKey)> {
        self.globals.declarations.iter().filter_map(|source| {
            if source.namespace.is_some() { return None; }
            Some((source.name.text.as_str(), source.statement, source.initializer?))
        })
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
        attached_type(&self.named, source)
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
            if let Some(contract) = self.plain_namespace_contract(source, queries) {
                return Some(contract);
            }
            return source.initializer.and_then(|expression| {
                let expression = &self.expressions.get(expression).expression;
                self.ambient_local_alias_contract(expression, queries)
                    .or_else(|| data::literal_contract(expression))
            });
        }
        None
    }
    pub(super) fn global_function_signatures(&self,key:NativeValueDeclarationKey)->Option<Vec<NativeSignatureKey>> {
        let function=self.globals.declaration(key)?.function?;
        let heads=self.function_overload_signatures(function);
        if !heads.is_empty() { return Some(heads); }
        Some(vec![self.implementation_annotation_signature(function)?])
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
        // A value assigned inside a generic function may be of its type parameters, which mean
        // nothing outside it: the global it declares is `any`.
        if source.nested
            && let Some(owner) = self.expressions.get(expression).owner
            && (!self.function_type_parameters(owner, queries).is_empty() || !self.function_outer_type_parameters(owner).is_empty())
        {
            return Type::Any;
        }
        let mut checker = self.checker_for_expression(
            expression,
            luanalysis_engine::NativeCheckMode::empty(),
            queries,
        );
        let ty = checker.expression(&self.expressions.get(expression).expression);
        self.merge_report(checker.report);
        // This query publishes a declaration to other chunks; they have no
        // continuation through this chunk's actual allocation stores. Publish
        // its original schema, while checking the initializer and same-chunk
        // reads still use the allocation carrier's reached value view.
        queries.runtime_table_schema(&ty).unwrap_or(ty)
    }
    pub(super) fn global_error(&self, key: NativeValueDeclarationKey, error: &GlobalQueryError) {
        let Some(source) = self.globals.declaration(key) else {
            return;
        };
        // As in TypeScript, a global whose initializer reads it is `any`, which only
        // `--no-implicit-any` reports.
        if matches!(error, GlobalQueryError::CircularQuery(_)) && !self.no_implicit_any() {
            return;
        }
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
            GlobalQueryError::SubsequentDeclaration { expected, actual, .. } => {
                // TypeScript's wording; the global keeps its first declaration's type.
                let message = format!("Subsequent variable declarations must have the same type. Variable '{}' must be of type '{{}}', but here has type '{{}}'.", source.name.text);
                self.report.borrow_mut().push_type_diagnostic("TS2403", source.name.span, &message, &[expected, actual]);
                return;
            }
            GlobalQueryError::MixedCallableDeclarations { .. } => ("LUA2034",format!("Global '{}' mixes bodyless function declarations with runtime or variable declarations; this combination is not supported.",source.name.text)),
            GlobalQueryError::InvalidDeclarationSignatures { .. } => ("LUA2034",format!("Global '{}' has invalid original bodyless function signature identities.",source.name.text)),
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
    /// Candidate discovery only. The native policy query still proves the
    /// reached value; later assignments can invalidate this original copy.
    pub(crate) fn has_standard_callable_initializer(&self, binding: BindingId) -> bool {
        let mut binding = binding;
        let mut visited = HashSet::default();
        while visited.insert(binding) {
            let Some(write) = self.flow.declaration_write(binding) else { return false };
            let crate::flow::NodeKind::Write { value, .. } = &write.kind else { return false };
            let evaluation = match value {
                crate::flow::ValueDemand::Evaluation(evaluation)
                | crate::flow::ValueDemand::PackElement { evaluation, index: 0 } => *evaluation,
                _ => return false,
            };
            let mut expression: &luanalysis_syntax::Expr = &self.flow.program.expression(self.flow.program.evaluation(evaluation).expression).expression;
            while let ExprKind::Parenthesized(inner) = &expression.kind { expression = inner; }
            let ExprKind::Name(name) = &expression.kind else { return false };
            if let Some(next) = self.bound.binding_at(name.span.start) { binding = next; }
            else { return matches!(name.text.as_str(), "type" | "assert" | "require" | "setmetatable"); }
        }
        false
    }

    pub(crate) fn check_global_declarations(&self, queries: &mut dyn TypeQueries) {
        if self.source_role() == SourceRole::Declaration {
            self.check_all_signatures(queries);
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
        let mut names = HashSet::default();
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
        GlobalQueryError::SubsequentDeclaration { declaration, .. } => vec![*declaration],
        GlobalQueryError::ConflictingDeclarations { declarations, .. } | GlobalQueryError::MixedCallableDeclarations { declarations, .. } => declarations.clone(),
        GlobalQueryError::InvalidDeclarationSignatures {declaration,..} => vec![*declaration],
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
    fn standard_callable_declaration_type(&mut self, declaration: NativeValueDeclarationKey) -> Option<Type> {
        match self.engine.value_declaration_type(declaration) {
            Ok(written) => Some(written),
            Err(GlobalQueryError::UnresolvedQuery(_)) => { self.incomplete_flow = true; None },
            Err(_) => None,
        }
    }

    /// This is an ordinary written type view, never physical value evidence.
    pub(crate) fn standard_callable_read(&mut self, expression: &luanalysis_syntax::Expr, at: luanalysis_engine::NativeFlowKey) -> Option<Type> {
        if self.runtime_value { return None; }
        let native = self.native?;
        let evaluation = native.flow.evaluation_for_expression(expression, at.graph)?;
        match self.engine.flow_standard_operation(
            &crate::native_flow::FlowAdapter { sources: &native.flow, values: native },
            native.flow.evaluation_value(evaluation.id),
            native.flow.flow_key(evaluation.input, at.graph.context),
        ) {
            luanalysis_engine::NativeStandardOperation::Standard(site) => {
                // A builtin assigned another builtin takes the replacement's
                // exact original public heads, not the old binding's contract.
                self.standard_callable_declaration_type(site.declaration)
            },
            luanalysis_engine::NativeStandardOperation::Pending => { self.incomplete_flow = true; None },
            luanalysis_engine::NativeStandardOperation::Ordinary => None,
        }
    }

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
    /// The type that the global declaration this name is, if it is one with its own contract
    /// (a `---@type` or a class it defines), declares.
    pub(crate) fn own_global_contract(&mut self, name: &Name) -> Option<Type> {
        let native = self.native?;
        let source = native.globals.declarations.iter().find(|source| source.name.span == name.span)?;
        if source.annotation.is_none() && native.global_attached_type(source).is_none() { return None; }
        self.engine.value_declaration_type(source.key).ok()
    }
    pub(crate) fn global_read(&mut self, name: &Name) -> Option<Type> {
        let native = self.native?;
        if native.project_globals.is_none() { return None; }
        let environment = self.environment_value(name.span.start);
        if name.text == "_ENV" { return Some(environment); }
        if self.engine.global_environment_origin(&environment).is_some() {
            let initial = {
                let key = native.global_binding(&name.text)?;
                // The original `_G` contract is a view of this actual object.
                self.engine.property_type(&environment, &name.text)
                    .unwrap_or_else(|| self.engine.global_binding_type(key).unwrap_or(Type::Unknown))
            };
            if let Some(at) = self.flow_at {
                let expression = luanalysis_syntax::Expr { span: name.span, kind: ExprKind::Name(name.clone()) };
                if matches!(name.text.as_str(), "type" | "assert" | "require" | "setmetatable")
                    && let Some(written) = self.standard_callable_read(&expression, at)
                {
                    // Ordinary standard operations use the selected policy's
                    // reached value. Raw captured flow may reset a capability
                    // because a different function could write it in the future.
                    // This view cannot certify physical aliases or environment identity.
                    return Some(written);
                }
                // A source allocation's descendants belong to its frozen Lua
                // address. Keep that full reference so later member stores
                // update its actual view; the canonical environment-global
                // reference only tracks replacement of the global itself.
                let reference = if self.engine.runtime_table_schema(&initial).is_some() {
                    native.flow.expression_reference(&expression, at.graph)
                } else {
                    native.flow.environment_member_reference(at.graph, &expression)
                };
                if let Some(reference) = reference {
                    // Unknown is the native reset type for a value capability:
                    // a computed write may replace it even if the declaration's
                    // callable shape remains valid. Ordinary globals keep TS's
                    // declaration-based transfer policy.
                    let declared = if matches!(name.text.as_str(), "_G" | "type" | "assert" | "require" | "setmetatable") { Type::Unknown } else { initial.clone() };
                    let result = self.engine.flow_type(&crate::native_flow::FlowAdapter { sources: &native.flow, values: native }, at, reference, &declared, &initial);
                    self.incomplete_flow |= result.incomplete;
                    return Some(result.ty);
                }
            }
            return Some(initial);
        }
        // A free name under a lexical `_ENV` is an ordinary member access.
        // Apply the same reference flow as `_ENV.name` after its lookup.
        let declared = self.native_named_property_read(&environment, name, name.span);
        let expression = luanalysis_syntax::Expr { span: name.span, kind: ExprKind::Name(name.clone()) };
        Some(self.native_member_flow_type(&expression, declared, None))
    }

    pub(super) fn environment_value(&mut self, offset: usize) -> Type {
        let Some(native) = self.native else { return Type::Unknown; };
        let binding = self.bound.environment_binding_at(offset);
        let (declared, initial) = if let Some(binding) = binding {
            let Some(value) = self.resolve_local(binding).cloned() else { return Type::Unknown; };
            (value.constraint, value.initial)
        } else {
            let Some(globals) = native.project_globals.as_ref() else { return Type::Unknown; };
            (Type::Unknown, self.engine.global_environment_type(globals.environment).unwrap_or(Type::Unknown))
        };
        // This is the actual lexical upvalue's immutability proof. Demanding
        // flow across unrelated calls for an unchanged environment makes call
        // classification depend recursively on its own callee's environment.
        // Explicit upvalue replacement retains the normal native traversal.
        if binding.is_none() && native.flow.environment_cell_is_immutable() { return initial; }
        let Some(at) = self.flow_at else { return initial; };
        let Some(reference) = native.flow.environment_reference(at.graph, binding) else { return initial; };
        let result = self.engine.flow_type(&crate::native_flow::FlowAdapter { sources: &native.flow, values: native }, at, reference, &declared, &initial);
        self.incomplete_flow |= result.incomplete;
        result.ty
    }

    /// Static assertions do not change which runtime table a receiver names.
    pub(crate) fn environment_origin_of_expression(&mut self, expression: &luanalysis_syntax::Expr) -> Option<NativeEnvironmentKey> {
        let mut expression = expression;
        while let ExprKind::Parenthesized(inner) = &expression.kind { expression = inner; }
        let value = self.expression_at_current_flow(expression, None);
        self.engine.global_environment_origin(&value)
    }
}

/// The global a `_G.name` or `_G["name"]` target names when `_G` is the original global
/// environment (no local `_G`, no rebound `_ENV`), as `name` would.
fn environment_member(target: &Expr, bound: &BoundFile) -> Option<Name> {
    let (base, name) = match &target.kind {
        ExprKind::Field { base, name } => (base, name.clone()),
        ExprKind::Index { base, index } => match &index.kind {
            // A quoted name: its span is inside the quotes.
            ExprKind::String(bytes) if index.span.end - index.span.start == bytes.len() + 2 => {
                let text = std::str::from_utf8(bytes).ok()?;
                let mut chars = text.chars();
                if !chars.next().is_some_and(luanalysis_syntax::identifier_start)
                    || !chars.all(luanalysis_syntax::identifier_continue)
                { return None; }
                (base, Name { text: text.to_owned(), span: Span::new(index.span.start + 1, index.span.end - 1) })
            },
            _ => return None,
        },
        _ => return None,
    };
    let ExprKind::Name(root) = &base.kind else { return None };
    (root.text == "_G" && bound.binding_at(root.span.start).is_none() && !nonstandard_environment(bound, root.span.start)).then_some(name)
}

pub(crate) fn nonstandard_environment(bound: &BoundFile, offset: usize) -> bool {
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

#[cfg(test)]
mod standard_callable_pending_tests {
    use super::*;
    use luanalysis_engine::{ContextualTypeOwner, Engine, NativeFlowKey, NativeGraphKey, Parameter};
    use std::cell::RefCell;

    const DECLARATION: NativeValueDeclarationKey = NativeValueDeclarationKey { snapshot: 99731, index: 0 };
    const FUNCTION: NativeFunctionKey = NativeFunctionKey { snapshot: 99731, index: 0 };

    struct Provider { ready: bool, errors: RefCell<Vec<GlobalQueryError>> }
    impl NativeTypeProvider for Provider {
        fn contains_value_declaration(&self, key: NativeValueDeclarationKey) -> bool { key == DECLARATION }
        fn value_declaration_header(&self, _: NativeValueDeclarationKey) -> ValueDeclarationHeader {
            ValueDeclarationHeader { role: GlobalSourceRole::Declaration, kind: GlobalValueKind::Function }
        }
        fn value_declaration_contract(&self, _: NativeValueDeclarationKey, queries: &mut dyn TypeQueries) -> Option<Type> {
            let _ = queries.contextual_type_owner(FUNCTION, NativeFlowKey {
                graph: NativeGraphKey { snapshot: 99731, region: 0, context: 0 }, node: 0,
            });
            // Even a provider that ignores Pending cannot publish this fallback.
            Some(Type::RuntimeFunction)
        }
        fn global_query_error(&self, error: GlobalQueryError, _: &mut dyn TypeQueries) { self.errors.borrow_mut().push(error); }
        fn contains_function(&self, key: NativeFunctionKey) -> bool { key == FUNCTION }
        fn contextual_type_owner(&self, _: NativeFunctionKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> ContextualTypeOwner {
            if self.ready { ContextualTypeOwner::Absent } else { ContextualTypeOwner::Pending }
        }
        fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { panic!("declaration lookup must not demand a runtime body") }
        fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { panic!("declaration lookup must not demand a runtime body") }
        fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type { panic!("declaration lookup must not infer a runtime body") }
        fn return_cycle(&self, _: NativeFunctionKey) { panic!("no runtime return query") }
        fn return_type_too_deep(&self, _: NativeFunctionKey) { panic!("no runtime return query") }
    }

    #[test]
    fn a_pending_standard_declaration_cannot_become_a_completed_unknown_read() {
        for ready in [false, true] {
            Engine::new().with_native_provider(
                move || Provider { ready, errors: RefCell::new(Vec::new()) },
                |provider, queries| {
                    let file = luanalysis_syntax::parse("");
                    let bound = crate::bind::bind(&file);
                    let registry = NativeRegistry::new(file, bound);
                    let mut reader = registry.checker(queries);
                    assert_eq!(reader.standard_callable_declaration_type(DECLARATION).is_some(), provider.ready);
                    assert_eq!(reader.incomplete_flow, !provider.ready);
                    // A completed fallback read cannot clear the earlier source flag.
                    assert_eq!(reader.engine.regular_type(&Type::Unknown), Type::Unknown);
                    assert_eq!(reader.incomplete_flow, !provider.ready);
                    assert_eq!(provider.errors.borrow().iter().any(|error| matches!(error, GlobalQueryError::UnresolvedQuery(_))), !provider.ready);
                },
            );
        }
    }
}

/// The statements nested in `statement`: the bodies of its blocks, loops, branches and named
/// functions, at any depth. Function expressions are visited from their own sources.
fn nested_statements<'a>(statement: &'a Stmt, result: &mut Vec<&'a Stmt>) {
    let visit = |body: &'a [Stmt], result: &mut Vec<&'a Stmt>| {
        for statement in body {
            result.push(statement);
            nested_statements(statement, result);
        }
    };
    match &statement.kind {
        StmtKind::Block { body, .. }
        | StmtKind::While { body, .. }
        | StmtKind::Repeat { body, .. }
        | StmtKind::NumericFor { body, .. }
        | StmtKind::GenericFor { body, .. } => visit(body, result),
        StmtKind::LocalFunction { function, .. } | StmtKind::FunctionDeclaration { function, .. } => visit(&function.body, result),
        StmtKind::If { branches, else_branch } => {
            for branch in branches {
                visit(&branch.body, result);
            }
            if let Some(branch) = else_branch {
                visit(&branch.body, result);
            }
        }
        _ => {}
    }
}
