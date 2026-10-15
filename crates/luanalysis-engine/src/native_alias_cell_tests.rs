use super::*;

struct Cells {
    source: Source,
}
impl Cells {
    fn new() -> Self {
        Self {
            source: Source::new(),
        }
    }
    fn with_heap_store() -> Self {
        let mut source = Self::new();
        source.source.nodes.push(NativeFlowNode::Write {
            before: at(0, 2),
            target: reference(0, 3),
            value: value(2),
            input: at(0, 2),
        });
        source
    }
}
impl NativeFlowProvider for Cells {
    fn reference_is_cell(&self, original: NativeReferenceKey) -> bool {
        original.reference < 2
    }
    fn contains_graph(&self, original: NativeGraphKey) -> bool {
        self.source.contains_graph(original)
    }
    fn contains_reference(&self, original: NativeReferenceKey) -> bool {
        self.contains_graph(original.graph) && original.reference < 4
    }
    fn node(&self, original: NativeFlowKey) -> Option<NativeFlowNode> {
        self.source.node(original)
    }
    fn is_shared(&self, original: NativeFlowKey) -> bool {
        self.source.is_shared(original)
    }
    fn write_affects(
        &self,
        query: NativeReferenceKey,
        target: NativeReferenceKey,
        at: NativeFlowKey,
    ) -> NativeWriteRelation {
        self.source.write_affects(query, target, at)
    }
    fn write_affects_with_types(
        &self,
        query: NativeReferenceKey,
        target: NativeReferenceKey,
        at: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> NativeWriteRelation {
        assert!(
            !self.reference_is_cell(query),
            "lexical cell identity cannot depend on heap alias classification"
        );
        if query == reference(0, 2) && target == reference(0, 3) {
            NativeWriteRelation::Exact
        } else {
            self.source.write_affects(query, target, at)
        }
    }
    fn value_alias_source(
        &self,
        original: NativeValueKey,
        input: NativeFlowKey,
    ) -> NativeAliasValue {
        self.source.value_alias_source(original, input)
    }
    fn reference_declaration(
        &self,
        original: NativeReferenceKey,
    ) -> Option<NativeVariableDeclaration> {
        self.source.reference_declaration(original)
    }
    fn graph_creation(&self, original: NativeGraphKey) -> Option<NativeFunctionCreation> {
        self.source.graph_creation(original)
    }
    fn reference_in_graph(
        &self,
        original: NativeReferenceKey,
        graph: NativeGraphKey,
    ) -> Option<NativeReferenceKey> {
        self.source.reference_in_graph(original, graph)
    }
    fn function_alias_returns(
        &self,
        original: NativeFunctionKey,
    ) -> Option<Vec<NativeAliasReturn>> {
        self.source.function_alias_returns(original)
    }
    fn function_alias_exit(&self, original: NativeFunctionKey) -> Option<NativeFlowKey> {
        self.source.function_alias_exit(original)
    }
    fn value_type(
        &self,
        original: NativeValueKey,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        self.source.value_type(original, input, queries)
    }
    fn call_effects(
        &self,
        original: NativeValueKey,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> NativeCallEffects {
        self.source.call_effects(original, input, queries)
    }
    fn flow_error(&self, at: NativeFlowKey, error: NativeFlowError) {
        self.source.flow_error(at, error)
    }
}

#[test]
fn member_writes_do_not_reclassify_the_owning_lexical_cells_identity() {
    Engine::new().with_native_provider(
        || Functions,
        |_, queries| {
            let source = Cells::with_heap_store();
            assert_eq!(
                queries.flow_alias_relation(&source, reference(0, 0), at(0, 6), value(3), at(0, 2)),
                NativeAliasRelation::Same
            );
        },
    );
}

#[test]
fn noncell_field_identities_still_follow_typed_heap_alias_writes() {
    Engine::new().with_native_provider(
        || Functions,
        |_, queries| {
            let source = Cells::with_heap_store();
            assert_eq!(
                queries.flow_alias_relation(&source, reference(0, 2), at(0, 6), value(2), at(0, 2)),
                NativeAliasRelation::Same
            );
        },
    );
}

#[test]
fn actual_cell_rebindings_keep_frozen_copy_inputs() {
    Engine::new().with_native_provider(
        || Functions,
        |_, queries| {
            let source = Cells::new();
            assert_eq!(
                queries.flow_alias_relation(&source, reference(0, 0), at(0, 3), value(3), at(0, 2)),
                NativeAliasRelation::Different
            );
            assert_eq!(
                queries.flow_alias_relation(&source, reference(0, 1), at(0, 3), value(0), at(0, 0)),
                NativeAliasRelation::Same
            );
        },
    );
}

#[test]
fn intervening_calls_preserve_stable_cells_but_not_cross_function_writes() {
    for foreign in [false, true] {
        Engine::new().with_native_provider(
            || Functions,
            |_, queries| {
                let mut source = Cells::new();
                source.source.foreign_write = foreign;
                assert_eq!(
                    queries.flow_alias_relation(
                        &source,
                        reference(0, 0),
                        at(0, 4),
                        value(17),
                        at(0, 3)
                    ),
                    if foreign {
                        NativeAliasRelation::Possible
                    } else {
                        NativeAliasRelation::Same
                    }
                );
            },
        );
    }
}
