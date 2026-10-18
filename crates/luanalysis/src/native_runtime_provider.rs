//! Source transport for structural schemas backed by real Lua allocations.
//! These helpers retain original recipes; native queries own inferred members,
//! recursion, substitution, presence and relations.
use crate::native_flow::{FlowValueQueries, ValueRecipe};
use crate::native_functions::NativeRegistry;
use crate::native_named::NativeTypeContext;
use crate::native_packs::AnnotationPack;
use crate::native_runtime_tables::RuntimeTableStoreKey;
use luanalysis_engine::{
    NativeExpressionKey, NativeFlowKey, NativeTypeDeclarationKey, NativeTypeMemberKey,
    ObjectDeclarationForm, Type, TypeDeclarationHeader, TypeDeclarationKind, TypeMemberHeader,
    TypeMemberKeyRecipe, TypeMemberOwner, TypeMemberVisibility, TypeQueries,
};
use luanalysis_syntax::{Expr, ExprKind, Span, TableFieldKind};
use luanalysis_engine::LuaKey;
use crate::bind::DeclarationKind;
use crate::collections::{HashMap, HashSet};

impl NativeRegistry {
    /// The runtime-table constructor whose schema owns this store, by Bun's
    /// expando rule: the binder declares `x.y = z` on `x`'s declaration, so
    /// ownership is decided from syntax and declarations alone, never from the
    /// receiver's flow or value type. Computed once per registry.
    pub(crate) fn runtime_store_owner(&self, store: usize) -> Option<NativeExpressionKey> {
        self.runtime_store_owners.get_or_init(|| {
            let mut memo = HashMap::default();
            self.runtime_tables.stores().iter().map(|store| {
                let receiver = &self.expressions().get(store.receiver).expression;
                let owner = self.syntactic_table_owner(receiver, Some(store.key.graph.region), 0, &mut memo);
                owner.filter(|&owner| self.runtime_tables.schema_key(owner).is_some())
            }).collect()
        }).get(store).copied().flatten()
    }

    /// The runtime schema of the constructor `expression` names syntactically, if it has one.
    pub(crate) fn runtime_table_owner_declaration(&self, expression: &Expr) -> Option<NativeTypeDeclarationKey> {
        let owner = self.syntactic_table_owner(expression, None, 0, &mut HashMap::default())?;
        self.runtime_tables.schema_key(owner)
    }

    /// The constructor `expression` names syntactically: the constructor itself,
    /// `setmetatable(t, mt)` (which returns `t`), a local by the assignment that
    /// syntactically reaches it in its own function body (`region`), else by its
    /// declaration (as Bun and v1 attribute expandos), a method's implicit
    /// `self` (owned like the declared receiver), or a static path to a nested
    /// table. Schemas belong to constructors, so a local rebound to a new
    /// constructor owns the new one from that point.
    fn syntactic_table_owner(&self, expression: &Expr, region: Option<u32>, depth: u32, memo: &mut HashMap<(usize, usize), Option<NativeExpressionKey>>) -> Option<NativeExpressionKey> {
        if depth > 32 { return None; }
        if let Some(&known) = memo.get(&(expression.span.start, expression.span.end)) { return known; }
        // A cycle through this expression has no owner.
        memo.insert((expression.span.start, expression.span.end), None);
        let owner = match &expression.kind {
            ExprKind::Parenthesized(inner) => self.syntactic_table_owner(inner, region, depth + 1, memo),
            // Any constructor, contracted or not, can own a nested table; only a
            // runtime schema owns stores (checked by `runtime_store_owner`).
            ExprKind::Table(_) => {
                let key = self.expressions().key_for_span(expression.span);
                self.runtime_tables.constructor(key).map(|_| key)
            }
            ExprKind::Call { callee, args } => match &callee.kind {
                ExprKind::Name(name) if name.text == "setmetatable" && self.bound().binding_at(name.span.start).is_none() => {
                    args.first().and_then(|table| self.syntactic_table_owner(table, region, depth + 1, memo))
                }
                _ => None,
            },
            ExprKind::Name(name) => self.bound().binding_at(name.span.start).and_then(|binding| {
                if self.bound().declaration(binding).kind == DeclarationKind::ImplicitReceiver {
                    return self.method_receiver_for(binding).and_then(|receiver| self.syntactic_table_owner(receiver, None, depth + 1, memo));
                }
                let sources = self.iterator_sources();
                let initializer = region.and_then(|region| sources.latest_binding_write_before(binding, region, expression.span.start))
                    .or_else(|| sources.declaration_initializer(binding))?;
                let region = Some(initializer.input.graph);
                let initializer = &sources.program.expression(initializer.expression).expression;
                self.syntactic_table_owner(initializer, region, depth + 1, memo)
            }),
            ExprKind::Field { base, name } => self.syntactic_member_owner(base, region, expression.span.start, &LuaKey::String(name.text.as_bytes().to_vec()), depth, memo),
            ExprKind::Index { base, index } => crate::native_tables::literal_key(index)
                .and_then(|key| self.syntactic_member_owner(base, region, expression.span.start, &key, depth, memo)),
            _ => None,
        };
        memo.insert((expression.span.start, expression.span.end), owner);
        owner
    }

    /// A nested table under an owned table: the owned store `base.key = {...}`
    /// that syntactically reaches `position` in the same function body, else the
    /// constructor field `key = {...}`, else the first such store in source
    /// order (a namespace built by assignments, as Bun's nested expandos).
    fn syntactic_member_owner(&self, base: &Expr, region: Option<u32>, position: usize, key: &LuaKey, depth: u32, memo: &mut HashMap<(usize, usize), Option<NativeExpressionKey>>) -> Option<NativeExpressionKey> {
        let owner = self.syntactic_table_owner(base, region, depth + 1, memo)?;
        let sources = self.iterator_sources();
        let (mut reaching, mut first) = (None, None);
        for store in self.runtime_tables.stores() {
            if !matches!(&store.member, RuntimeTableStoreKey::Fixed(stored) if stored == key) { continue; }
            let receiver = &self.expressions().get(store.receiver).expression;
            let store_region = Some(store.key.graph.region);
            if self.syntactic_table_owner(receiver, store_region, depth + 1, memo) != Some(owner) { continue; }
            let (Some(ValueRecipe::Evaluation(value)) | Some(ValueRecipe::PackElement { evaluation: value, index: 0 })) = sources.value(store.value) else { continue; };
            let value = &sources.program.expression(sources.program.evaluation(*value).expression).expression;
            let Some(nested) = self.syntactic_table_owner(value, store_region, depth + 1, memo) else { continue; };
            if store_region == region && store.statement_end <= position
                && reaching.is_none_or(|(end, _)| store.statement_end > end)
            { reaching = Some((store.statement_end, nested)); }
            if first.is_none_or(|(start, _)| store.span.start < start) { first = Some((store.span.start, nested)); }
        }
        if let Some((_, nested)) = reaching { return Some(nested); }
        let constructor = self.runtime_tables.constructor(owner)?;
        constructor.fields.iter().find(|field| match &field.kind {
            TableFieldKind::Named(name) => LuaKey::String(name.text.as_bytes().to_vec()) == *key,
            TableFieldKind::Keyed(expression) => crate::native_tables::literal_key(expression).as_ref() == Some(key),
            TableFieldKind::Sequence { index } => LuaKey::Integer(*index as i64) == *key,
        }).and_then(|field| self.syntactic_table_owner(&field.value, region, depth + 1, memo))
            .or_else(|| first.map(|(_, nested)| nested))
    }

    pub(crate) fn original_literal_self_type(
        &self,
        function: luanalysis_engine::NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        let constructor = self.expressions().table_function_owner(function)?;
        let declaration = self.runtime_tables.schema_key(constructor)?;
        // v1's ordinary table-field owner is the containing literal. Reuse its
        // original structural schema; native queries retain fixed fields,
        // recursion, and the source generic mapper. No inferred type is kept.
        Some(match queries.named_type_reference(declaration, &[]) {
            Ok(ty) => ty,
            Err(error) => {
                self.record_named_application_error(error, self.function(function).span, queries);
                queries.error_type()
            }
        })
    }
    pub(crate) fn runtime_store_in_scope(&self, declaration: NativeTypeDeclarationKey, at: NativeFlowKey) -> bool {
        let Some(constructor) = self.runtime_tables.schema(declaration) else { return false; };
        let Some(store) = self.runtime_tables.store(at) else { return false; };
        constructor.is_lexically_uncontracted() && store.annotation.is_none()
            && constructor.input.graph.snapshot == at.graph.snapshot
            && constructor.input.graph.region == at.graph.region
    }
    pub(crate) fn runtime_schema_contains(&self, key: NativeTypeDeclarationKey) -> bool {
        self.runtime_tables.schema(key).is_some()
    }
    pub(crate) fn runtime_schema_constructor(
        &self,
        key: NativeTypeDeclarationKey,
    ) -> Option<NativeExpressionKey> {
        Some(self.runtime_tables.schema(key)?.key)
    }
    pub(crate) fn runtime_schema_header(
        &self,
        key: NativeTypeDeclarationKey,
    ) -> Option<TypeDeclarationHeader> {
        let source = self.runtime_tables.schema(key)?;
        let header = |outer| TypeDeclarationHeader {
            kind: TypeDeclarationKind::RuntimeTable,
            name: String::new(),
            object_form: Some(ObjectDeclarationForm::Record),
            parameters: Vec::new(),
            outer_parameters: outer,
        };
        // Only type parameters in scope where the table is created can be outer parameters, and
        // most tables are created where there are none.
        if !source.owner.is_some_and(|owner| self.types.any_parameter_in_scope(owner)) { return Some(header(Vec::new())) }
        let mut used = HashSet::default();
        self.value_type_dependencies(
            &self.expressions().get(source.key).expression,
            &mut used,
            &mut HashSet::default(),
            &mut HashSet::default(),
        );
        for (index, store) in self.runtime_tables.stores().iter().enumerate() {
            // Only this table's own stores describe its members.
            if self.runtime_store_owner(index) != Some(source.key) { continue }
            if let Some((annotation, _)) = &store.annotation {
                let context = self
                    .expressions()
                    .get(store.receiver)
                    .owner
                    .map(NativeTypeContext::Function);
                for ty in &annotation.types {
                    self.annotation_parameter_dependencies(context, ty, &mut used);
                }
            }
            match self
                .iterator_sources()
                .value(store.value)
                .expect("original runtime store value")
            {
                ValueRecipe::Evaluation(evaluation)
                | ValueRecipe::PackElement { evaluation, .. } => {
                    let expression = &self
                        .iterator_sources()
                        .program
                        .expression(
                            self.iterator_sources()
                                .program
                                .evaluation(*evaluation)
                                .expression,
                        )
                        .expression;
                    self.value_type_dependencies(
                        expression,
                        &mut used,
                        &mut HashSet::default(),
                        &mut HashSet::default(),
                    );
                }
                ValueRecipe::Function(graph) => {
                    if let Some(function) = self
                        .iterator_sources()
                        .function_for_graph(self.iterator_sources().graph(*graph, 0))
                    {
                        self.function_type_dependencies(
                            function,
                            &mut used,
                            &mut HashSet::default(),
                            &mut HashSet::default(),
                        );
                    }
                }
                _ => {}
            }
        }
        let mut outer = source.owner.map_or_else(Vec::new, |owner| {
            self.types
                .own_parameters(owner)
                .into_iter()
                .chain(self.types.outer_parameters(owner, &used))
                .filter(|key| used.contains(key))
                .collect()
        });
        outer.sort_by_key(|key| (key.snapshot, key.index));
        outer.dedup();
        Some(header(outer))
    }
    pub(crate) fn runtime_schema_seed(
        &self,
        key: NativeTypeDeclarationKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        let source = self.runtime_tables.schema(key)?;
        let mut checker = self.checker_for_expression(
            source.key,
            luanalysis_engine::NativeCheckMode::empty(),
            queries,
        );
        let ty = checker.native_table_expression(&source.fields, source.span, None);
        if !checker.incomplete_flow {
            self.merge_report(checker.report);
        }
        Some(ty)
    }
    pub(crate) fn runtime_store_receiver_type(
        &self,
        key: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        // Membership needs only the owner schema's identity, decided
        // syntactically (`runtime_store_owner`); the receiver is not flow-typed.
        let store = self.runtime_tables.store_index(key)?;
        let schema = self.runtime_tables.schema_key(self.runtime_store_owner(store)?)?;
        let view = queries.table_type(&luanalysis_engine::TableType { fresh: true, ..Default::default() }).ok()?;
        queries.runtime_table_value(schema, &view).ok()
    }
    pub(crate) fn runtime_member_header(
        &self,
        key: NativeTypeMemberKey,
    ) -> Option<TypeMemberHeader> {
        let source = self.runtime_tables.member(key)?;
        Some(TypeMemberHeader {
            owner: TypeMemberOwner::RuntimeStore(source.key),
            key: match &source.member {
                RuntimeTableStoreKey::Fixed(key) => TypeMemberKeyRecipe::Fixed(key.clone()),
                RuntimeTableStoreKey::Computed(_) => TypeMemberKeyRecipe::Indexer { sparse: true },
            },
            visibility: TypeMemberVisibility::Public,
            optional: false,
            readonly: false,
        })
    }
    pub(crate) fn runtime_member_type(
        &self,
        key: NativeTypeMemberKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        let source = self.runtime_tables.member(key)?;
        if let Some((annotation, index)) = &source.annotation {
            let mut checker = self.checker_for_expression(
                source.receiver,
                luanalysis_engine::NativeCheckMode::empty(),
                queries,
            );
            let pack =
                AnnotationPack::annotation(annotation).lower(&mut |ty| checker.annotation(ty));
            let ty = checker.engine.pack_slot(&pack, *index);
            self.merge_report(checker.report);
            return Some(ty);
        }
        let contextual = queries.current_contextual_function();
        let captured = queries.current_captured_function();
        let graph = self.iterator_sources().graph_with_contexts(
            source.epoch.graph.region,
            contextual.as_ref(),
            captured.as_ref(),
        );
        let input = NativeFlowKey {
            graph,
            node: source.epoch.node,
        };
        let recipe = self
            .iterator_sources()
            .value(source.value)
            .expect("original runtime store value");
        let value = self.flow_value_type(self.iterator_sources(), recipe, input, queries);
        // A prospective original field contract must agree with the literal
        // read from its reached store. Scalar local storage still widens.
        Some(queries.regular_type(&value))
    }
    pub(crate) fn runtime_member_index_key(
        &self,
        key: NativeTypeMemberKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        let source = self.runtime_tables.member(key)?;
        let RuntimeTableStoreKey::Computed(expression) = source.member else {
            return None;
        };
        let mut checker = self.checker_for_expression(
            expression,
            luanalysis_engine::NativeCheckMode::empty(),
            queries,
        );
        let ty = checker.expression(&self.expressions().get(expression).expression);
        self.merge_report(checker.report);
        Some(ty)
    }
    pub(crate) fn runtime_declaration_origin(&self, key: NativeTypeDeclarationKey) -> Option<Span> {
        Some(self.runtime_tables.schema(key)?.span)
    }
    pub(crate) fn runtime_member_origin(&self, key: NativeTypeMemberKey) -> Option<Span> {
        Some(self.runtime_tables.member(key)?.span)
    }
}

impl crate::Checker<'_> {
    pub(crate) fn runtime_member_contract(
        &mut self,
        expression: &luanalysis_syntax::Expr,
        actual: Type,
    ) -> Type {
        if let Some(native) = self.native
            && self.type_context.is_some()
            && let Some(reference) = self.reference(expression)
            && native.binding_function_owner(reference.binding) != self.type_context
        {
            if reference.path.is_empty()
                && let Some(contract) = self.class_namespace_member_contract(reference.binding, &actual)
            { return contract; }
            if let Some(schema) = self.engine.runtime_table_schema(&actual) { return schema; }
        }
        actual
    }
    pub(crate) fn finish_runtime_table_initializer(
        &mut self,
        expression: &luanalysis_syntax::Expr,
        expected: Option<&Type>,
        actual: Type,
    ) -> Type {
        // An inferred property can supply its allocation schema as contextual
        // guidance. That does not make the RHS a written shape: retain this
        // constructor's own allocation key, rather than erasing its carrier or
        // reusing the earlier property's allocation identity.
        if expected.is_some_and(|expected| expected != &Type::Any
            && self.engine.runtime_table_schema(expected).is_none())
            || !matches!(expression.kind, ExprKind::Table(_))
        {
            return actual;
        }
        // Constructor checking already reported its error. Its recovery value
        // has no actual table view to retain as an allocation carrier.
        if actual == Type::Unknown || actual == self.engine.error_type() {
            return actual;
        }
        let Some(native) = self.native else {
            return actual;
        };
        if native.source_role() != luanalysis_project::SourceRole::Implementation {
            return actual;
        }
        let key = native.expressions().key_for_span(expression.span);
        let Some(schema) = native.runtime_tables.schema_key(key) else {
            return actual;
        };
        match self.engine.runtime_table_value(schema, &actual) {
            Ok(value) => value,
            Err(error) => {
                let (template, types) = crate::diagnostic_types::named_error_description(&error);
                self.error_types(
                    "LUA2039",
                    expression.span,
                    format!("Runtime table construction failed: {template}"),
                    &types.iter().collect::<Vec<_>>(),
                );
                actual
            }
        }
    }
}
