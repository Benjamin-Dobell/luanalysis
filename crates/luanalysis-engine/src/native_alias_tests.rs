use crate::*;
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
        original: NativeValueKey,
        _: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> Type {
        match original.index {
            7 => Type::NativeFunction(function(0)),
            15 => Type::NativeFunction(function(1)),
            _ => Type::Nil,
        }
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
            4 | 7 | 15 => NativeAliasValue::NotTable,
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
    Engine::new().with_native_provider(
        || Functions,
        |_, q| {
            for (returns, expected) in [
                (Returns::Allocation, NativeAliasRelation::Different),
                (Returns::Cached, NativeAliasRelation::Possible),
                (Returns::Memoized, NativeAliasRelation::Possible),
            ] {
                let mut source = Source::new();
                source.returns = returns;
                assert_eq!(
                    q.flow_alias_relation(&source, reference(0, 0), at(0, 5), value(9), at(0, 0)),
                    expected
                );
            }
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
    Engine::new().with_native_provider(
        || Functions,
        |_, q| {
            for (returns, expected) in [
                (Returns::SharedPair, NativeAliasRelation::Same),
                (Returns::DistinctPair, NativeAliasRelation::Different),
            ] {
                let mut source = Source::new();
                source.returns = returns;
                assert_eq!(
                    q.flow_alias_relation(&source, reference(0, 0), at(0, 5), value(8), at(0, 0)),
                    expected
                );
            }
        },
    );
}
#[test]
fn calls_preserve_immutable_cells_but_invalidate_fields_and_cross_function_writes() {
    Engine::new().with_native_provider(
        || Functions,
        |_, q| {
            let mut source = Source::new();
            assert_eq!(
                q.flow_alias_relation(&source, reference(0, 0), at(0, 4), value(17), at(0, 3)),
                NativeAliasRelation::Same
            );
            assert_eq!(
                q.flow_alias_relation(&source, reference(0, 2), at(0, 4), value(18), at(0, 3)),
                NativeAliasRelation::Possible
            );
            source.foreign_write = true;
            assert_eq!(
                q.flow_alias_relation(&source, reference(0, 0), at(0, 4), value(17), at(0, 3)),
                NativeAliasRelation::Possible
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
            source.returns = Returns::Recursive;
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
            assert_eq!(
                queries.flow_alias_relation(&source, reference(0, 0), at(0, 5), value(8), at(0, 0)),
                NativeAliasRelation::Possible
            );
        },
    );
}
