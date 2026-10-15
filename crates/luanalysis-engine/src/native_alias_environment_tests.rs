use crate::*;

fn graph(region: u32) -> NativeGraphKey {
    NativeGraphKey {
        snapshot: 7990,
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
fn reference(index: u32) -> NativeReferenceKey {
    NativeReferenceKey {
        graph: graph(0),
        reference: index,
    }
}
fn value(index: u32) -> NativeValueKey {
    NativeValueKey {
        snapshot: 7990,
        index,
    }
}
fn function() -> NativeFunctionKey {
    NativeFunctionKey {
        snapshot: 7991,
        index: 0,
    }
}
fn environment(index: u32) -> NativeEnvironmentKey {
    NativeEnvironmentKey {
        snapshot: 7992,
        index,
    }
}

struct Environments;
impl NativeTypeProvider for Environments {
    fn contains_global_environment(&self, key: NativeEnvironmentKey) -> bool {
        key.snapshot == 7992 && key.index < 2
    }
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        key == function()
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        Vec::new()
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        Some(Type::RuntimeTable)
    }
    fn function_inferred_return(
        &self,
        _: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        queries.global_environment_type(environment(0)).unwrap()
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("environment return is not recursive")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("environment return is finite")
    }
}
struct Flow {
    recursive_probe: bool,
}
impl NativeFlowProvider for Flow {
    fn contains_graph(&self, key: NativeGraphKey) -> bool {
        key == graph(0) || key == graph(1)
    }
    fn contains_reference(&self, key: NativeReferenceKey) -> bool {
        key.graph == graph(0) && key.reference < 2
    }
    fn reference_is_cell(&self, _: NativeReferenceKey) -> bool {
        true
    }
    fn node(&self, key: NativeFlowKey) -> Option<NativeFlowNode> {
        Some(match (key.graph.region, key.node) {
            (_, 0) => NativeFlowNode::Start,
            (0, 1) => NativeFlowNode::Write {
                before: at(0, 0),
                target: reference(0),
                value: value(0),
                input: at(0, 0),
            },
            (0, 2) => NativeFlowNode::Write {
                before: at(0, 1),
                target: reference(1),
                value: value(1),
                input: at(0, 1),
            },
            (1, 1) => NativeFlowNode::Return { before: at(1, 0) },
            (1, 2) => NativeFlowNode::Unreachable,
            _ => return None,
        })
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
    fn alias_environment_candidate(&self, key: NativeValueKey, _: NativeFlowKey) -> bool {
        matches!(key.index, 0 | 4 | 5)
    }
    fn value_alias_source(&self, key: NativeValueKey, _: NativeFlowKey) -> NativeAliasValue {
        match key.index {
            1 | 2 => NativeAliasValue::Call {
                callee: value(3),
                input: at(0, 0),
            },
            4 => NativeAliasValue::Allocation,
            _ => NativeAliasValue::Unknown,
        }
    }
    fn function_alias_returns(&self, key: NativeFunctionKey) -> Option<Vec<NativeAliasReturn>> {
        (key == function()).then(|| {
            vec![NativeAliasReturn {
                at: at(1, 1),
                values: vec![NativeAliasReturnValue {
                    value: value(0),
                    input: at(1, 0),
                }],
                expands_last: false,
            }]
        })
    }
    fn function_alias_exit(&self, key: NativeFunctionKey) -> Option<NativeFlowKey> {
        (key == function()).then_some(at(1, 2))
    }
    fn value_type(
        &self,
        key: NativeValueKey,
        _: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        match key.index {
            0 => {
                if self.recursive_probe {
                    assert_eq!(
                        queries.flow_alias_relation(
                            self,
                            reference(0),
                            at(0, 1),
                            value(0),
                            at(0, 0)
                        ),
                        NativeAliasRelation::Possible
                    );
                }
                queries.global_environment_type(environment(0)).unwrap()
            }
            3 => Type::NativeFunction(function()),
            4 => queries.table_type(&TableType::default()).unwrap(),
            5 => queries.global_environment_type(environment(1)).unwrap(),
            _ => panic!("only original callee and environment values are inspected"),
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
        panic!("{error:?}")
    }
}

#[test]
fn actual_environment_origins_distinguish_fresh_allocations_and_survive_return_calls() {
    Engine::new().with_native_provider(
        || Environments,
        |_, queries| {
            let flow = Flow {
                recursive_probe: false,
            };
            assert_eq!(
                queries.flow_alias_relation(&flow, reference(0), at(0, 1), value(0), at(0, 0)),
                NativeAliasRelation::Same
            );
            assert_eq!(
                queries.flow_alias_relation(&flow, reference(0), at(0, 1), value(4), at(0, 0)),
                NativeAliasRelation::Different,
                "candidate metadata cannot brand a fresh allocation as an environment"
            );
            assert_eq!(
                queries.flow_alias_relation(&flow, reference(0), at(0, 1), value(5), at(0, 0)),
                NativeAliasRelation::Different,
                "different original environment objects remain distinct"
            );
            assert_eq!(
                queries.flow_alias_relation(&flow, reference(0), at(0, 1), value(1), at(0, 0)),
                NativeAliasRelation::Same
            );
            assert_eq!(
                queries.flow_alias_relation(&flow, reference(1), at(0, 2), value(2), at(0, 0)),
                NativeAliasRelation::Same,
                "returning the same external object does not allocate it again per call"
            );
        },
    );
}

#[test]
fn circular_value_evidence_cannot_attest_an_environment_object() {
    Engine::new().with_native_provider(
        || Environments,
        |_, queries| {
            let flow = Flow {
                recursive_probe: true,
            };
            assert_eq!(
                queries.flow_alias_relation(&flow, reference(0), at(0, 1), value(4), at(0, 0)),
                NativeAliasRelation::Possible
            );
        },
    );
}
