use super::*;

#[derive(Clone, Copy)]
enum History {
    Original,
    Replaced,
    Call,
    Loop,
}
struct Fields {
    history: History,
}
fn node(context: u64, index: u32) -> NativeFlowKey {
    NativeFlowKey {
        graph: NativeGraphKey {
            context,
            ..graph(0)
        },
        node: index,
    }
}
fn cell(context: u64, index: u32) -> NativeReferenceKey {
    NativeReferenceKey {
        graph: node(context, 0).graph,
        reference: index,
    }
}
impl NativeFlowProvider for Fields {
    fn alias_node_dispatch(&self, at: NativeFlowKey) -> NativeAliasDispatch {
        match self.node(at) {
            Some(NativeFlowNode::Write { target, .. }) if !self.reference_is_cell(target) => {
                // This store replaces the existing original constructor field;
                // the native read guard must prove its raw presence.
                NativeAliasDispatch::ReferenceRead(target)
            },
            Some(_) => NativeAliasDispatch::NoDispatch,
            None => NativeAliasDispatch::Unknown,
        }
    }
    fn reference_is_cell(&self, reference: NativeReferenceKey) -> bool {
        reference.reference < 2
    }
    fn contains_graph(&self, graph: NativeGraphKey) -> bool {
        graph.snapshot == 7710 && graph.region == 0 && graph.context < 2
    }
    fn contains_reference(&self, reference: NativeReferenceKey) -> bool {
        self.contains_graph(reference.graph) && reference.reference < 4
    }
    fn node(&self, at: NativeFlowKey) -> Option<NativeFlowNode> {
        if !self.contains_graph(at.graph) {
            return None;
        }
        let context = at.graph.context;
        Some(match at.node {
            0 => NativeFlowNode::Start,
            1 => NativeFlowNode::Write {
                before: node(context, 0),
                target: cell(context, 0),
                value: value(0),
                input: node(context, 0),
            },
            2 => match self.history {
                History::Original => NativeFlowNode::Pass {
                    before: node(context, 1),
                },
                History::Replaced => NativeFlowNode::Write {
                    before: node(context, 1),
                    target: cell(context, 3),
                    value: value(21),
                    input: node(context, 1),
                },
                History::Call => NativeFlowNode::Call {
                    before: node(context, 1),
                    value: value(7),
                    input: node(context, 1),
                },
                History::Loop => NativeFlowNode::Loop {
                    edges: vec![node(context, 1), node(context, 2)],
                },
            },
            3 => NativeFlowNode::Write {
                before: node(context, 2),
                target: cell(context, 1),
                value: value(22),
                input: node(context, 2),
            },
            _ => return None,
        })
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool {
        false
    }
    fn write_affects(
        &self,
        reference: NativeReferenceKey,
        target: NativeReferenceKey,
        _: NativeFlowKey,
    ) -> NativeWriteRelation {
        if reference == target {
            NativeWriteRelation::Exact
        } else if reference.reference == 2 && target.reference == 1
            || reference.reference == 3 && target.reference == 0
        {
            NativeWriteRelation::Ancestor
        } else if reference.reference == 0 && target.reference == 3 {
            NativeWriteRelation::Descendant
        } else {
            NativeWriteRelation::Unaffected
        }
    }
    fn reference_member_path(
        &self,
        reference: NativeReferenceKey,
        member: NativeReferenceKey,
    ) -> Option<Vec<LuaKey>> {
        (reference.graph == member.graph
            && ((reference.reference == 1 && member.reference == 2)
                || (reference.reference == 0 && member.reference == 3)))
            .then(|| vec![LuaKey::String(b"child".to_vec())])
    }
    fn write_member_key(
        &self,
        _: NativeReferenceKey,
        target: NativeReferenceKey,
        _: NativeFlowKey,
    ) -> Option<LuaKey> {
        (target.reference == 3).then(|| LuaKey::String(b"child".to_vec()))
    }
    fn write_alias_receiver(&self, at: NativeFlowKey) -> Option<NativeAliasReturnValue> {
        (at.node == 2 && matches!(self.history, History::Replaced)).then(|| {
            NativeAliasReturnValue {
                value: value(23),
                input: node(at.graph.context, 1),
            }
        })
    }
    fn value_alias_source(
        &self,
        original: NativeValueKey,
        input: NativeFlowKey,
    ) -> NativeAliasValue {
        match original.index {
            0 | 20 | 21 => NativeAliasValue::Allocation,
            22 => NativeAliasValue::Reference {
                reference: cell(input.graph.context, 0),
                input: node(input.graph.context, 2),
            },
            23 => NativeAliasValue::Reference {
                reference: cell(input.graph.context, 0),
                input: node(input.graph.context, 1),
            },
            _ => NativeAliasValue::Unknown,
        }
    }
    fn value_alias_member_source(
        &self,
        original: NativeValueKey,
        input: NativeFlowKey,
        key: &LuaKey,
    ) -> Option<NativeAliasReturnValue> {
        (original == value(0) && key == &LuaKey::String(b"child".to_vec())).then(|| {
            NativeAliasReturnValue {
                value: value(if input.graph.context == 0 { 20 } else { 21 }),
                input: node(input.graph.context, 0),
            }
        })
    }
    fn reference_declaration(
        &self,
        original: NativeReferenceKey,
    ) -> Option<NativeVariableDeclaration> {
        self.reference_is_cell(original)
            .then(|| NativeVariableDeclaration {
                owner: original.graph,
                reference: original,
                writes: vec![],
            })
    }
    fn value_type(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type {
        panic!("projected identities use original recipes, not inferred types")
    }
    fn call_effects(
        &self,
        _: NativeValueKey,
        _: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> NativeCallEffects {
        panic!("heap-call provenance is conservative without inferring effects")
    }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) {
        panic!("invalid original graph {error:?}")
    }
}

#[test]
fn original_projected_queries_keep_graph_facets_and_input_epochs_separate() {
    Engine::new().with_native_provider(
        || Functions,
        |_, queries| {
            let source = Fields {
                history: History::Original,
            };
            for (context, expected) in [
                (0, NativeAliasRelation::Same),
                (1, NativeAliasRelation::Different),
                (0, NativeAliasRelation::Same),
            ] {
                assert_eq!(
                    queries.flow_alias_relation(
                        &source,
                        cell(context, 2),
                        node(context, 3),
                        value(20),
                        node(context, 0)
                    ),
                    expected
                );
            }
            assert_eq!(
                queries.flow_alias_relation(&source, cell(0, 2), node(0, 0), value(20), node(0, 0)),
                NativeAliasRelation::Possible
            );
            assert_eq!(
                queries.flow_alias_relation(&source, cell(0, 2), node(0, 3), value(20), node(0, 0)),
                NativeAliasRelation::Same
            );
        },
    );
}

#[test]
fn projected_aliases_follow_the_actual_latest_heap_store_before_a_copy() {
    Engine::new().with_native_provider(
        || Functions,
        |_, queries| {
            let source = Fields {
                history: History::Replaced,
            };
            assert_eq!(
                queries.flow_alias_relation(&source, cell(0, 2), node(0, 3), value(21), node(0, 1)),
                NativeAliasRelation::Same
            );
            assert_eq!(
                queries.flow_alias_relation(&source, cell(0, 2), node(0, 3), value(20), node(0, 0)),
                NativeAliasRelation::Different
            );
        },
    );
}

#[test]
fn calls_and_loop_boundaries_cannot_reuse_a_projected_constructor_identity() {
    for history in [History::Call, History::Loop] {
        Engine::new().with_native_provider(
            || Functions,
            |_, queries| {
                let source = Fields { history };
                for _ in 0..2 {
                    assert_eq!(
                        queries.flow_alias_relation(
                            &source,
                            cell(0, 2),
                            node(0, 3),
                            value(20),
                            node(0, 0)
                        ),
                        NativeAliasRelation::Possible
                    );
                }
            },
        );
    }
}
