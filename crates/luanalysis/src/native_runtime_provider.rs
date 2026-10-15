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
use luanalysis_syntax::{ExprKind, Span};
use std::collections::HashSet;

impl NativeRegistry {
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
        let mut used = HashSet::new();
        self.value_type_dependencies(
            &self.expressions().get(source.key).expression,
            &mut used,
            &mut HashSet::new(),
            &mut HashSet::new(),
        );
        for store in self.runtime_tables.stores() {
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
                        &mut HashSet::new(),
                        &mut HashSet::new(),
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
                            &mut HashSet::new(),
                            &mut HashSet::new(),
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
        Some(TypeDeclarationHeader {
            kind: TypeDeclarationKind::RuntimeTable,
            name: String::new(),
            object_form: Some(ObjectDeclarationForm::Record),
            parameters: Vec::new(),
            outer_parameters: outer,
        })
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
        let source = self.runtime_tables.store(key)?;
        let contextual = queries.current_contextual_function();
        let captured = queries.current_captured_function();
        let graph = self.iterator_sources().graph_with_contexts(
            source.receiver_input.graph.region,
            contextual.as_ref(),
            captured.as_ref(),
        );
        let input = NativeFlowKey {
            graph,
            node: source.receiver_input.node,
        };
        let recipe = self
            .iterator_sources()
            .value(source.receiver_value)
            .expect("original receiver value");
        Some(self.flow_value_type(self.iterator_sources(), recipe, input, queries))
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
        Some(self.flow_value_type(self.iterator_sources(), recipe, input, queries))
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
            && let Some(schema) = self.engine.runtime_table_schema(&actual)
        {
            return schema;
        }
        actual
    }
    pub(crate) fn finish_runtime_table_initializer(
        &mut self,
        expression: &luanalysis_syntax::Expr,
        expected: Option<&Type>,
        actual: Type,
    ) -> Type {
        if expected.is_some_and(|expected| expected != &Type::Any)
            || !matches!(expression.kind, ExprKind::Table(_))
        {
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
                self.error(
                    "LUA2039",
                    expression.span,
                    format!("Runtime table construction failed: {error:?}."),
                );
                actual
            }
        }
    }
}
