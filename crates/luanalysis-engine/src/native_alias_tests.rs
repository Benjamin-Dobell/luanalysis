use crate::*;
#[path = "native_alias_projected_tests.rs"]
mod projected_tests;
#[path = "native_alias_cell_tests.rs"]
mod cell_tests;
fn graph(region: u32) -> NativeGraphKey {
    NativeGraphKey {
        snapshot: 7710,
        region,
        context: 0,
    }
}
fn at(region: u32, node: u32) -> NativeFlowKey {
    NativeFlowKey {
        graph: graph(region),
        node,
    }
}
fn reference(region: u32, index: u32) -> NativeReferenceKey {
    NativeReferenceKey {
        graph: graph(region),
        reference: index,
    }
}
fn value(index: u32) -> NativeValueKey {
    NativeValueKey {
        snapshot: 7710,
        index,
    }
}
fn function(index: u32) -> NativeFunctionKey {
    NativeFunctionKey {
        snapshot: 7711,
        index,
    }
}
struct Functions;
impl NativeTypeProvider for Functions {
    fn contains_function(&self, original: NativeFunctionKey) -> bool {
        original.snapshot == 7711 && original.index < 2
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        vec![]
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        None
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("original return provenance does not infer a type")
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("unexpected type cycle")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("unexpected type depth")
    }
}
#[derive(Clone, Copy)]
enum Returns {
    Allocation,
    SharedPair,
    DistinctPair,
    Cached,
    Memoized,
    Recursive,
    PartialRecursive,
}
struct Source {
    nodes: Vec<NativeFlowNode>,
    returns: Returns,
    foreign_write: bool,
    nested_capture: bool,
}
impl Source {
    fn write(before: u32, target: u32, value_index: u32) -> NativeFlowNode {
        NativeFlowNode::Write {
            before: at(0, before),
            target: reference(0, target),
            value: value(value_index),
            input: at(0, before),
        }
    }
    fn new() -> Self {
        Self {
            nodes: vec![
                NativeFlowNode::Start,
                Self::write(0, 0, 0),
                Self::write(1, 1, 1),
                Self::write(2, 0, 2),
                NativeFlowNode::Call {
                    before: at(0, 3),
                    value: value(5),
                    input: at(0, 3),
                },
                Self::write(0, 0, 6),
            ],
            returns: Returns::Allocation,
            foreign_write: false,
            nested_capture: false,
        }
    }
}
impl NativeFlowProvider for Source {
    fn reference_is_cell(&self, original: NativeReferenceKey) -> bool { original.reference < 2 }
    fn alias_node_dispatch(&self, original: NativeFlowKey) -> NativeAliasDispatch {
        if self.node(original).is_some() { NativeAliasDispatch::NoDispatch }
        else { NativeAliasDispatch::Unknown }
    }
    fn alias_call_source(&self, original: NativeValueKey, input: NativeFlowKey) -> Option<NativeAliasCall> {
        let (callee, callee_input) = match original.index {
            6 | 9 => (value(7), input),
            14 => (value(15), input),
            19 => (value(7), at(1, 0)),
            _ => return None,
        };
        Some(NativeAliasCall {
            callee: NativeAliasReturnValue { value: callee, input: callee_input },
            arguments: vec![],
            execution_input: input,
        })
    }
    fn function_alias_execution(&self, original: NativeFunctionKey, creation: NativeFlowKey) -> Option<NativeAliasExecution> {
        (original == function(0) && creation == at(0, 0)).then_some(NativeAliasExecution {
            entry: at(1, 0),
            terminals: vec![at(1, 2), at(1, 4), at(1, 3)],
        })
    }
    fn contains_graph(&self, original: NativeGraphKey) -> bool {
        original.snapshot == 7710 && original.region < 3 && original.context == 0
    }
    fn contains_reference(&self, original: NativeReferenceKey) -> bool {
        self.contains_graph(original.graph) && original.reference < 3
    }
    fn node(&self, original: NativeFlowKey) -> Option<NativeFlowNode> {
        if !self.contains_graph(original.graph) {
            return None;
        }
        if original.graph.region == 0 {
            return self.nodes.get(original.node as usize).cloned();
        }
        if original.graph.region == 2 {
            return Some(match original.node {
                0 => NativeFlowNode::Start,
                1 => NativeFlowNode::Write {
                    before: at(2, 0),
                    target: reference(2, 0),
                    value: value(13),
                    input: at(2, 0),
                },
                _ => return None,
            });
        }
        Some(match original.node {
            0 => NativeFlowNode::Start,
            1 => NativeFlowNode::Write {
                before: at(1, 0),
                target: reference(1, 0),
                value: value(10),
                input: at(1, 0),
            },
            2 => NativeFlowNode::Return { before: at(1, 1) },
            3 => NativeFlowNode::Unreachable,
            4 => NativeFlowNode::Return { before: at(1, 0) },
            _ => return None,
        })
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool {
        false
    }
    fn write_affects(
        &self,
        query: NativeReferenceKey,
        target: NativeReferenceKey,
        _: NativeFlowKey,
    ) -> NativeWriteRelation {
        if query == target {
            NativeWriteRelation::Exact
        } else {
            NativeWriteRelation::Unaffected
        }
    }
    fn value_type(
        &self,
        _: NativeValueKey,
        _: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> Type {
        panic!("original return provenance must not identify callees through expression types")
    }
    fn call_effects(
        &self,
        _: NativeValueKey,
        _: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> NativeCallEffects {
        NativeCallEffects::None
    }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) {
        panic!("invalid original graph {error:?}")
    }
    fn reference_declaration(
        &self,
        original: NativeReferenceKey,
    ) -> Option<NativeVariableDeclaration> {
        if self.nested_capture && original == reference(1, 1) {
            return Some(NativeVariableDeclaration {
                owner: graph(2),
                reference: reference(2, 0),
                writes: vec![],
            });
        }
        if original.graph.region != 0 || original.reference == 2 {
            return None;
        }
        Some(NativeVariableDeclaration {
            owner: graph(0),
            reference: original,
            writes: if self.foreign_write {
                vec![NativeVariableWrite {
                    owner: graph(1),
                    statement_end: 0,
                }]
            } else {
                vec![]
            },
        })
    }
    fn graph_creation(&self, original: NativeGraphKey) -> Option<NativeFunctionCreation> {
        if !self.nested_capture {
            return None;
        }
        match original.region {
            1 => Some(NativeFunctionCreation {
                input: at(2, 1),
                position: 10,
            }),
            2 => Some(NativeFunctionCreation {
                input: at(0, 0),
                position: 5,
            }),
            _ => None,
        }
    }
    fn reference_in_graph(
        &self,
        original: NativeReferenceKey,
        target: NativeGraphKey,
    ) -> Option<NativeReferenceKey> {
        if self.nested_capture && original == reference(1, 1) && target == graph(2) {
            Some(reference(2, 0))
        } else {
            (original.graph == target).then_some(original)
        }
    }
    fn value_alias_source(
        &self,
        original: NativeValueKey,
        input: NativeFlowKey,
    ) -> NativeAliasValue {
        match original.index {
            0 | 2 | 10 | 13 => NativeAliasValue::Allocation,
            1 => NativeAliasValue::Reference {
                reference: reference(0, 0),
                input: at(0, 1),
            },
            3 => NativeAliasValue::Reference {
                reference: reference(0, 1),
                input: at(0, 2),
            },
            4 => NativeAliasValue::NotTable,
            7 => NativeAliasValue::Function { function: function(0), creation: at(0, 0) },
            15 => NativeAliasValue::Function { function: function(1), creation: at(0, 0) },
            6 | 9 => NativeAliasValue::Call {
                callee: value(7),
                input,
            },
            8 => NativeAliasValue::Projection {
                value: value(6),
                input: at(0, 0),
                index: 1,
            },
            11 | 12 => NativeAliasValue::Reference {
                reference: reference(1, 0),
                input: at(1, 1),
            },
            14 => NativeAliasValue::Call {
                callee: value(15),
                input,
            },
            16 => NativeAliasValue::Reference {
                reference: reference(1, 1),
                input: at(1, 0),
            },
            17 => NativeAliasValue::Reference {
                reference: reference(0, 0),
                input: at(0, 3),
            },
            18 => NativeAliasValue::Reference {
                reference: reference(0, 2),
                input: at(0, 3),
            },
            19 => NativeAliasValue::Call {
                callee: value(7),
                input: at(1, 0),
            },
            _ => NativeAliasValue::Unknown,
        }
    }
    fn function_alias_returns(
        &self,
        original: NativeFunctionKey,
    ) -> Option<Vec<NativeAliasReturn>> {
        if original != function(0) {
            return None;
        }
        let original_value = |index| NativeAliasReturnValue {
            value: value(index),
            input: at(1, if matches!(index, 11 | 12) { 1 } else { 0 }),
        };
        let values = match self.returns {
            Returns::Allocation | Returns::Memoized => vec![original_value(10)],
            Returns::SharedPair => vec![original_value(11), original_value(12)],
            Returns::DistinctPair => vec![original_value(10), original_value(13)],
            Returns::Cached => vec![original_value(16)],
            Returns::Recursive => vec![original_value(19)],
            Returns::PartialRecursive => vec![original_value(10), original_value(10)],
        };
        let mut rows = vec![NativeAliasReturn {
            at: at(1, 2),
            values,
            expands_last: matches!(self.returns, Returns::Recursive),
        }];
        if matches!(self.returns, Returns::Memoized) {
            rows.push(NativeAliasReturn {
                at: at(1, 4),
                values: vec![original_value(16)],
                expands_last: false,
            });
        }
        if matches!(self.returns, Returns::PartialRecursive) {
            rows.push(NativeAliasReturn {
                at: at(1, 4),
                values: vec![original_value(19)],
                expands_last: true,
            });
        }
        Some(rows)
    }
    fn function_alias_exit(&self, original: NativeFunctionKey) -> Option<NativeFlowKey> {
        (original == function(0)).then_some(at(1, 3))
    }
}
#[test]
fn original_alias_copies_follow_frozen_epochs_across_rebinding() {
    Engine::new().with_native_provider(
        || Functions,
        |_, q| {
            let source = Source::new();
            assert_eq!(
                q.flow_alias_relation(&source, reference(0, 0), at(0, 2), value(3), at(0, 2)),
                NativeAliasRelation::Same
            );
            assert_eq!(
                q.flow_alias_relation(&source, reference(0, 0), at(0, 3), value(3), at(0, 3)),
                NativeAliasRelation::Different
            );
            assert_eq!(
                q.flow_alias_relation(&source, reference(0, 0), at(0, 3), value(4), at(0, 3)),
                NativeAliasRelation::Different
            );
            assert_eq!(
                q.flow_alias_relation(&source, reference(0, 0), at(0, 3), value(5), at(0, 3)),
                NativeAliasRelation::Possible
            );
        },
    );
}
#[test]
fn proven_fresh_calls_differ_but_cached_memoized_and_unknown_calls_stay_possible() {
    for (returns, expected) in [
        (Returns::Allocation, NativeAliasRelation::Different),
        (Returns::Cached, NativeAliasRelation::Possible),
        (Returns::Memoized, NativeAliasRelation::Possible),
    ] {
        Engine::new().with_native_provider(
            || Functions,
            |_, q| {
                let mut source = Source::new();
                source.returns = returns;
                assert_eq!(
                    q.flow_alias_relation(&source, reference(0, 0), at(0, 5), value(9), at(0, 0)),
                    expected
                );
            },
        );
    }
    Engine::new().with_native_provider(
        || Functions,
        |_, q| {
            let source = Source::new();
            assert_eq!(
                q.flow_alias_relation(&source, reference(0, 0), at(0, 5), value(14), at(0, 0)),
                NativeAliasRelation::Possible
            );
        },
    );
}
#[test]
fn sibling_pack_projections_share_or_distinguish_the_actual_returned_allocations() {
    for (returns, expected) in [
        (Returns::SharedPair, NativeAliasRelation::Same),
        (Returns::DistinctPair, NativeAliasRelation::Different),
    ] {
        Engine::new().with_native_provider(
            || Functions,
            |_, q| {
                let mut source = Source::new();
                source.returns = returns;
                assert_eq!(
                    q.flow_alias_relation(&source, reference(0, 0), at(0, 5), value(8), at(0, 0)),
                    expected
                );
            },
        );
    }
}
#[test]
/// Useful rather than sound, as in TypeScript: an unresolved call preserves
/// the cells and fields it does not visibly write, including a cell that another
/// function writes. Strict runtime effects are covered by the host CLI tests.
fn unresolved_calls_preserve_cells_fields_and_cross_function_cells() {
    Engine::new().with_native_provider(
        || Functions,
        |_, q| {
            let source = Source::new();
            assert_eq!(
                q.flow_alias_relation(&source, reference(0, 0), at(0, 4), value(17), at(0, 3)),
                NativeAliasRelation::Same
            );
            assert_eq!(
                q.flow_alias_relation(&source, reference(0, 2), at(0, 4), value(18), at(0, 3)),
                NativeAliasRelation::Same
            );
        },
    );
    Engine::new().with_native_provider(
        || Functions,
        |_, q| {
            let mut source = Source::new();
            source.foreign_write = true;
            assert_eq!(
                q.flow_alias_relation(&source, reference(0, 0), at(0, 4), value(17), at(0, 3)),
                NativeAliasRelation::Same
            );
        },
    );
}
#[test]
fn loops_and_recursive_return_provenance_never_equate_repeated_allocation_sites() {
    Engine::new().with_native_provider(
        || Functions,
        |_, q| {
            let mut source = Source::new();
            source.nodes.push(NativeFlowNode::Loop {
                edges: vec![at(0, 1), at(0, 6)],
            });
            assert_eq!(
                q.flow_alias_relation(&source, reference(0, 0), at(0, 6), value(0), at(0, 0)),
                NativeAliasRelation::Possible
            );
        },
    );
    Engine::new().with_native_provider(
        || Functions,
        |_, q| {
            let mut source = Source::new();
            source.returns = Returns::Recursive;
            assert_eq!(
                q.flow_alias_intrinsic(&source, value(9), at(0, 0)),
                NativeAliasIntrinsic::Pending,
                "an actual recursive return dependency is still unfinished"
            );
            assert_eq!(
                q.flow_alias_relation(&source, reference(0, 0), at(0, 5), value(9), at(0, 0)),
                NativeAliasRelation::Possible
            );
        },
    );
}

#[test]
fn captures_from_distinct_factory_activations_do_not_share_a_static_allocation_identity() {
    Engine::new().with_native_provider(
        || Functions,
        |_, queries| {
            let mut source = Source::new();
            source.returns = Returns::Cached;
            source.nested_capture = true;
            assert_eq!(
                queries.flow_alias_relation(&source, reference(0, 0), at(0, 5), value(9), at(0, 0)),
                NativeAliasRelation::Possible
            );
        },
    );
}

#[test]
fn matching_known_allocations_do_not_hide_an_untracked_recursive_return_alternative() {
    Engine::new().with_native_provider(
        || Functions,
        |_, queries| {
            let mut source = Source::new();
            source.returns = Returns::PartialRecursive;
            for _ in 0..2 {
                assert_eq!(
                    queries.flow_alias_relation(
                        &source,
                        reference(0, 0),
                        at(0, 5),
                        value(8),
                        at(0, 0)
                    ),
                    NativeAliasRelation::Possible
                );
            }
            assert_eq!(
                queries.flow_alias_relation(&source, reference(0, 0), at(0, 2), value(3), at(0, 2)),
                NativeAliasRelation::Same
            );
        },
    );
}

struct ContextSource;
fn context_at(context: u64, node: u32) -> NativeFlowKey {
    NativeFlowKey {
        graph: NativeGraphKey {
            context,
            ..graph(0)
        },
        node,
    }
}
fn context_reference(context: u64) -> NativeReferenceKey {
    NativeReferenceKey {
        graph: context_at(context, 0).graph,
        reference: 0,
    }
}
impl NativeFlowProvider for ContextSource {
    fn alias_node_dispatch(&self, original: NativeFlowKey) -> NativeAliasDispatch {
        if self.node(original).is_some() { NativeAliasDispatch::NoDispatch }
        else { NativeAliasDispatch::Unknown }
    }
    fn contains_graph(&self, key: NativeGraphKey) -> bool {
        key.snapshot == 7710 && key.region == 0 && key.context < 2
    }
    fn contains_reference(&self, key: NativeReferenceKey) -> bool {
        self.contains_graph(key.graph) && key.reference == 0
    }
    fn node(&self, key: NativeFlowKey) -> Option<NativeFlowNode> {
        if !self.contains_graph(key.graph) {
            return None;
        }
        match key.node {
            0 => Some(NativeFlowNode::Start),
            1 => Some(NativeFlowNode::Write {
                before: context_at(key.graph.context, 0),
                target: context_reference(key.graph.context),
                value: value(0),
                input: context_at(key.graph.context, 0),
            }),
            _ => None,
        }
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool {
        false
    }
    fn write_affects(
        &self,
        left: NativeReferenceKey,
        right: NativeReferenceKey,
        _: NativeFlowKey,
    ) -> NativeWriteRelation {
        if left == right {
            NativeWriteRelation::Exact
        } else {
            NativeWriteRelation::Unaffected
        }
    }
    fn value_alias_source(&self, key: NativeValueKey, input: NativeFlowKey) -> NativeAliasValue {
        match key.index {
            0 if input.graph.context == 0 => NativeAliasValue::Allocation,
            0 => NativeAliasValue::NotTable,
            1 => NativeAliasValue::Reference {
                reference: context_reference(input.graph.context),
                input: context_at(input.graph.context, 1),
            },
            _ => NativeAliasValue::Unknown,
        }
    }
    fn value_type(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type {
        panic!("alias metadata needs no inferred type")
    }
    fn call_effects(
        &self,
        _: NativeValueKey,
        _: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> NativeCallEffects {
        panic!("no call in original graph")
    }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) {
        panic!("invalid original context {error:?}")
    }
}

#[test]
fn memoized_alias_origins_retain_original_body_context_and_input_flow() {
    Engine::new().with_native_provider(
        || Functions,
        |_, queries| {
            let source = ContextSource;
            for (context, expected) in [
                (0, NativeAliasRelation::Same),
                (1, NativeAliasRelation::Different),
                (0, NativeAliasRelation::Same),
            ] {
                assert_eq!(
                    queries.flow_alias_relation(
                        &source,
                        context_reference(context),
                        context_at(context, 1),
                        value(1),
                        context_at(context, 1)
                    ),
                    expected
                );
            }
            assert_eq!(
                queries.flow_alias_relation(
                    &source,
                    context_reference(0),
                    context_at(0, 0),
                    value(1),
                    context_at(0, 1)
                ),
                NativeAliasRelation::Possible
            );
            assert_eq!(
                queries.flow_alias_relation(
                    &source,
                    context_reference(0),
                    context_at(0, 1),
                    value(1),
                    context_at(0, 1)
                ),
                NativeAliasRelation::Same
            );
        },
    );
}
