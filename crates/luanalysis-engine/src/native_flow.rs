//! Original-source graph records with owned types at the checker boundary.

use super::native_packs::import_pack;
use super::{Checker, SessionQueries, Type, TypeHandles, TypeQueries, ValuePack, import_type};
use bun_sema::check::native_flow as core;

pub use core::{
    NativeCallEffects, NativeFlowError, NativeFlowKey, NativeFlowNode, NativeGraphKey,
    NativeNumericConstant, NativeNumericInput, NativeNumericIterationStage, NativePackProjection,
    NativePredicate, NativeReferenceKey, NativeRuntimeType, NativeValueKey, NativeWriteRelation, NativeIteratorProjection, NativeIteratorStage,
    NativeVariableDeclaration, NativeVariableWrite, NativeFunctionCreation,
};
pub use bun_sema::check::native_aliases::{NativeAliasValue, NativeAliasRelation, NativeAliasReturn, NativeAliasReturnValue};

#[derive(Debug, Clone, PartialEq)]
pub struct NativeFlowResult {
    pub ty: Type,
    pub incomplete: bool,
    pub unresolved_auto: bool,
}

/// Immutable graph recipes and source-value demands. No graph join, loop
/// iteration, or narrowing algorithm is performed by this interface.
pub trait NativeFlowProvider {
    /// Immutable source classification: a lexical cell, not a mutable heap slot.
    fn reference_is_cell(&self, _: NativeReferenceKey) -> bool { false }
    /// Original written binding/member contract, independent of flow epoch.
    fn reference_written_contract(&self, _: NativeReferenceKey, _: &mut dyn TypeQueries) -> Option<Type> { None }
    fn value_alias_source(&self, _: NativeValueKey, _: NativeFlowKey) -> NativeAliasValue { NativeAliasValue::Unknown }
    fn value_alias_member_source(&self, _: NativeValueKey, _: NativeFlowKey, _: &super::LuaKey) -> Option<NativeAliasReturnValue> { None }
    fn function_alias_returns(&self, _: super::NativeFunctionKey) -> Option<Vec<NativeAliasReturn>> { None }
    fn function_alias_exit(&self, _: super::NativeFunctionKey) -> Option<NativeFlowKey> { None }
    fn reference_tracks_runtime_value(&self, _: NativeReferenceKey) -> bool { false }
    fn reference_follows_creation(&self, _: NativeReferenceKey) -> bool { false }
    fn runtime_type_operation(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> bool { true }
    fn assertion_predicate(&self, _: NativeValueKey, _: NativeFlowKey) -> Option<NativePredicate> { None }
    fn reference_declaration(&self, _: NativeReferenceKey) -> Option<NativeVariableDeclaration> { None }
    fn graph_creation(&self, _: NativeGraphKey) -> Option<NativeFunctionCreation> { None }
    fn reference_in_graph(&self, reference: NativeReferenceKey, graph: NativeGraphKey) -> Option<NativeReferenceKey> {
        (reference.graph == graph).then_some(reference)
    }
    fn iterator_error(&self, _: NativeFlowKey, _: NativeIteratorStage, _: &Type, _: super::CallError, _: &mut dyn TypeQueries) {}
    fn iterator_projection(&self, _: NativeReferenceKey) -> Option<NativeIteratorProjection> { None }
    fn pack_projection(&self, _: NativeReferenceKey) -> Option<NativePackProjection> {
        None
    }
    fn value_pack(
        &self,
        value: NativeValueKey,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        ValuePack::single(self.value_type(value, input, queries))
    }
    fn contains_graph(&self, graph: NativeGraphKey) -> bool;
    fn contains_reference(&self, reference: NativeReferenceKey) -> bool;
    fn node(&self, node: NativeFlowKey) -> Option<NativeFlowNode>;
    fn is_shared(&self, node: NativeFlowKey) -> bool;
    fn write_affects(
        &self,
        reference: NativeReferenceKey,
        target: NativeReferenceKey,
        at: NativeFlowKey,
    ) -> NativeWriteRelation;
    /// Resolve actual frozen receiver identity before comparing addresses.
    fn write_affects_with_types(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeWriteRelation {
        self.write_affects(reference, target, at)
    }
    fn call_invalidates_reference(&self, _: NativeReferenceKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> bool { false }
    /// Original fixed child key for a descendant write. Types and the actual
    /// persistent receiver update are evaluated by the native flow worker.
    fn write_member_key(&self, _: NativeReferenceKey, _: NativeReferenceKey, _: NativeFlowKey) -> Option<super::LuaKey> { None }
    /// A tested fixed child of the same original receiver.
    fn reference_member_key(&self, _: NativeReferenceKey, _: NativeReferenceKey) -> Option<super::LuaKey> { None }
    fn value_type(
        &self,
        value: NativeValueKey,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> Type;
    fn call_effects(
        &self,
        value: NativeValueKey,
        input: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> NativeCallEffects;
    fn flow_error(&self, at: NativeFlowKey, error: NativeFlowError);
}

pub(super) struct FlowProviderAdapter<'a> {
    pub provider: &'a dyn NativeFlowProvider,
    pub handles: &'a TypeHandles,
}

impl core::NativeFlowProvider for FlowProviderAdapter<'_> {
    fn reference_is_cell(&self, reference: NativeReferenceKey) -> bool { self.provider.reference_is_cell(reference) }
    fn reference_written_contract(&self, reference: NativeReferenceKey, checker: &mut Checker<'_, '_>) -> Option<bun_sema::types::TypeId> {
        let contract = self.provider.reference_written_contract(reference, &mut SessionQueries { checker, handles: self.handles })?;
        Some(import_type(checker, self.handles, &contract))
    }
    fn value_alias_source(&self, value: NativeValueKey, input: NativeFlowKey) -> NativeAliasValue { self.provider.value_alias_source(value, input) }
    fn value_alias_member_source(&self, value: NativeValueKey, input: NativeFlowKey, key: bun_sema::types::NativeTableKey, checker: &Checker<'_, '_>) -> Option<NativeAliasReturnValue> {
        self.provider.value_alias_member_source(value, input, &super::native_tables::export_literal_key(checker, key))
    }
    fn function_alias_returns(&self, function: bun_sema::types::NativeFunctionId) -> Option<Vec<NativeAliasReturn>> { self.provider.function_alias_returns(super::source_key(function)) }
    fn function_alias_exit(&self, function: bun_sema::types::NativeFunctionId) -> Option<NativeFlowKey> { self.provider.function_alias_exit(super::source_key(function)) }
    fn reference_tracks_runtime_value(&self, reference: NativeReferenceKey) -> bool { self.provider.reference_tracks_runtime_value(reference) }
    fn reference_follows_creation(&self, reference: NativeReferenceKey) -> bool { self.provider.reference_follows_creation(reference) }
    fn runtime_type_operation(&self, value: NativeValueKey, input: NativeFlowKey, checker: &mut Checker<'_, '_>) -> bool {
        self.provider.runtime_type_operation(value, input, &mut SessionQueries { checker, handles: self.handles })
    }
    fn assertion_predicate(&self, condition: NativeValueKey, input: NativeFlowKey) -> Option<NativePredicate> {
        self.provider.assertion_predicate(condition, input)
    }
    fn reference_declaration(&self, reference: NativeReferenceKey) -> Option<NativeVariableDeclaration> { self.provider.reference_declaration(reference) }
    fn graph_creation(&self, graph: NativeGraphKey) -> Option<NativeFunctionCreation> { self.provider.graph_creation(graph) }
    fn reference_in_graph(&self, reference: NativeReferenceKey, graph: NativeGraphKey) -> Option<NativeReferenceKey> { self.provider.reference_in_graph(reference, graph) }
    fn iterator_projection(&self, reference: NativeReferenceKey) -> Option<NativeIteratorProjection> { self.provider.iterator_projection(reference) }
    fn iterator_error(&self, at: NativeFlowKey, stage: NativeIteratorStage, callee: bun_sema::types::TypeId, error: bun_sema::check::ResolvedTypeCallError, checker: &mut Checker<'_, '_>) {
        let mut queries = SessionQueries { checker, handles: self.handles };
        let callee = queries.handles.retain(callee);
        let result = queries.export_call_result(bun_sema::check::ResolvedTypeCall { sig: None, ret: bun_sema::types::TypeId::ANY, errors: vec![error] });
        for error in result.errors { self.provider.iterator_error(at, stage, &callee, error, &mut queries); }
    }
    fn pack_projection(&self, reference: NativeReferenceKey) -> Option<NativePackProjection> {
        self.provider.pack_projection(reference)
    }
    fn value_pack(
        &self,
        value: NativeValueKey,
        input: NativeFlowKey,
        checker: &mut Checker<'_, '_>,
    ) -> bun_sema::types::NativeResultPack {
        let pack = self.provider.value_pack(
            value,
            input,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        import_pack(checker, self.handles, &pack)
    }
    fn contains_graph(&self, graph: NativeGraphKey) -> bool {
        self.provider.contains_graph(graph)
    }
    fn contains_reference(&self, reference: NativeReferenceKey) -> bool {
        self.provider.contains_reference(reference)
    }
    fn node(&self, node: NativeFlowKey) -> Option<NativeFlowNode> {
        self.provider.node(node)
    }
    fn is_shared(&self, node: NativeFlowKey) -> bool {
        self.provider.is_shared(node)
    }
    fn write_affects(
        &self,
        reference: NativeReferenceKey,
        target: NativeReferenceKey,
        at: NativeFlowKey,
    ) -> NativeWriteRelation {
        self.provider.write_affects(reference, target, at)
    }
    fn write_affects_with_types(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey, checker: &mut Checker<'_, '_>) -> NativeWriteRelation {
        self.provider.write_affects_with_types(reference, target, at, &mut SessionQueries { checker, handles: self.handles })
    }
    fn call_invalidates_reference(&self, reference: NativeReferenceKey, at: NativeFlowKey, checker: &mut Checker<'_, '_>) -> bool {
        self.provider.call_invalidates_reference(reference, at, &mut SessionQueries { checker, handles: self.handles })
    }
    fn write_member_key(&self, reference: NativeReferenceKey, target: NativeReferenceKey, at: NativeFlowKey, checker: &mut Checker<'_, '_>) -> Option<bun_sema::types::NativeTableKey> {
        let key = self.provider.write_member_key(reference, target, at)?;
        Some(super::native_tables::import_literal_key(checker, &key))
    }
    fn reference_member_key(&self, reference: NativeReferenceKey, member: NativeReferenceKey, checker: &mut Checker<'_, '_>) -> Option<bun_sema::types::NativeTableKey> {
        let key = self.provider.reference_member_key(reference, member)?;
        Some(super::native_tables::import_literal_key(checker, &key))
    }
    fn value_type(
        &self,
        value: NativeValueKey,
        input: NativeFlowKey,
        checker: &mut Checker<'_, '_>,
    ) -> bun_sema::types::TypeId {
        let ty = self.provider.value_type(
            value,
            input,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        import_type(checker, self.handles, &ty)
    }
    fn call_effects(
        &self,
        value: NativeValueKey,
        input: NativeFlowKey,
        checker: &mut Checker<'_, '_>,
    ) -> NativeCallEffects {
        self.provider.call_effects(
            value,
            input,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        )
    }
    fn flow_error(&self, at: NativeFlowKey, error: NativeFlowError) {
        self.provider.flow_error(at, error);
    }
}

impl SessionQueries<'_, '_, '_> {
    pub(super) fn flow_pack_projection_native(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        projection: NativePackProjection,
        ordinary: &Type,
    ) -> NativeFlowResult {
        let adapter = FlowProviderAdapter {
            provider,
            handles: self.handles,
        };
        let ordinary = import_type(self.checker, self.handles, ordinary);
        let result = self
            .checker
            .native_flow_pack_projection(&adapter, at, projection, ordinary);
        NativeFlowResult {
            ty: self.handles.retain(result.ty),
            incomplete: result.incomplete,
            unresolved_auto: result.unresolved_auto,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;

    #[derive(Default)]
    struct Graph {
        errors: std::cell::RefCell<Vec<NativeFlowError>>,
        reads: std::cell::Cell<usize>,
    }
    impl Graph {
        fn key() -> NativeGraphKey {
            NativeGraphKey {
                snapshot: 777,
                region: 0,
                context: 0,
            }
        }
        fn at(node: u32) -> NativeFlowKey {
            NativeFlowKey {
                graph: Self::key(),
                node,
            }
        }
        fn reference() -> NativeReferenceKey {
            NativeReferenceKey {
                graph: Self::key(),
                reference: 0,
            }
        }
        fn value(index: u32) -> NativeValueKey {
            NativeValueKey {
                snapshot: 777,
                index,
            }
        }
        fn declared() -> Type {
            Type::Union(vec![Type::Number, Type::String, Type::Nil])
        }
        fn write(before: u32, value: u32, input: u32) -> NativeFlowNode {
            NativeFlowNode::Write {
                before: Self::at(before),
                target: Self::reference(),
                value: Self::value(value),
                input: Self::at(input),
            }
        }
    }
    impl NativeFlowProvider for Graph {
        fn contains_graph(&self, graph: NativeGraphKey) -> bool {
            graph == Self::key()
        }
        fn contains_reference(&self, reference: NativeReferenceKey) -> bool {
            reference == Self::reference()
        }
        fn node(&self, node: NativeFlowKey) -> Option<NativeFlowNode> {
            if node.graph != Self::key() {
                return None;
            }
            Some(match node.node {
                0 => NativeFlowNode::Start,
                1 => Self::write(0, 0, 0),
                2 => Self::write(0, 1, 0),
                3 => NativeFlowNode::Merge {
                    edges: vec![Self::at(1), Self::at(2)],
                },
                4 => NativeFlowNode::Loop {
                    edges: vec![Self::at(0), Self::at(5)],
                },
                5 => Self::write(4, 0, 4),
                6 => NativeFlowNode::Predicate {
                    before: Self::at(0),
                    predicate: NativePredicate::Truthy {
                        subject: Some(Self::reference()),
                        value: Self::value(2),
                        input: Self::at(0),
                    },
                    sense: true,
                },
                7 => NativeFlowNode::Unreachable,
                8 => NativeFlowNode::Call {
                    before: Self::at(0),
                    value: Self::value(2),
                    input: Self::at(0),
                },
                9 => Self::write(1, 3, 0),
                10 => NativeFlowNode::Loop {
                    edges: vec![Self::at(0), Self::at(11)],
                },
                11 => Self::write(10, 4, 10),
                12 => NativeFlowNode::Predicate {
                    before: Self::at(0),
                    predicate: NativePredicate::Truthy {
                        subject: None,
                        value: Self::value(5),
                        input: Self::at(0),
                    },
                    sense: true,
                },
                13 => Self::write(12, 1, 12),
                14 => NativeFlowNode::Merge {
                    edges: vec![Self::at(1), Self::at(13)],
                },
                15 => NativeFlowNode::Loop {
                    edges: vec![Self::at(0), Self::at(16)],
                },
                16 => Self::write(17, 0, 17),
                17 => NativeFlowNode::Predicate {
                    before: Self::at(15),
                    predicate: NativePredicate::Truthy {
                        subject: None,
                        value: Self::value(5),
                        input: Self::at(15),
                    },
                    sense: true,
                },
                _ => return None,
            })
        }
        fn is_shared(&self, node: NativeFlowKey) -> bool {
            matches!(node.node, 0 | 4)
        }
        fn write_affects(
            &self,
            reference: NativeReferenceKey,
            target: NativeReferenceKey,
            _: NativeFlowKey,
        ) -> NativeWriteRelation {
            if reference == target {
                NativeWriteRelation::Exact
            } else {
                NativeWriteRelation::Unaffected
            }
        }
        fn value_type(
            &self,
            value: NativeValueKey,
            input: NativeFlowKey,
            queries: &mut dyn TypeQueries,
        ) -> Type {
            self.reads.set(self.reads.get() + 1);
            match value.index {
                0 => Type::Number,
                1 => Type::String,
                2 => {
                    queries
                        .flow_type(
                            self,
                            input,
                            Self::reference(),
                            &Self::declared(),
                            &Type::Nil,
                        )
                        .ty
                }
                3 => {
                    queries
                        .flow_type(
                            self,
                            input,
                            Self::reference(),
                            &Self::declared(),
                            &Type::Nil,
                        )
                        .ty
                }
                4 => {
                    let active = queries.flow_type(
                        self,
                        input,
                        Self::reference(),
                        &Self::declared(),
                        &Type::Nil,
                    );
                    assert!(
                        active.incomplete,
                        "back edge should read the active native loop"
                    );
                    queries.union_reduced(&[active.ty, Type::Number])
                }
                5 => Type::BooleanLiteral(false),
                _ => panic!("missing source value"),
            }
        }
        fn call_effects(
            &self,
            _: NativeValueKey,
            _: NativeFlowKey,
            queries: &mut dyn TypeQueries,
        ) -> NativeCallEffects {
            assert!(queries.is_assignable(&Type::Never, &Type::Number));
            NativeCallEffects::Never
        }
        fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) {
            self.errors.borrow_mut().push(error);
        }
    }

    #[test]
    fn correlated_pack_projection_rejects_foreign_frozen_input_before_value_demand() {
        Engine::new().with_session(|queries| {
            let graph = Graph::default();
            let mut input = Graph::at(0);
            input.graph.region = 1;
            let projection = NativePackProjection {
                group: Graph::reference(),
                value: Graph::value(0),
                input,
                index: 1,
            };
            queries.flow_pack_projection(&graph, Graph::at(0), projection, &Type::Number);
            assert_eq!(graph.reads.get(), 0);
            assert!(matches!(
                graph.errors.borrow().as_slice(),
                [NativeFlowError::ForeignEdge { .. }]
            ));
        });
    }

    #[test]
    fn flow_queries_keep_native_joins_loops_and_frozen_input_in_one_session() {
        Engine::new().with_session(|queries| {
            let graph = Graph::default();
            let merge = queries.flow_type(
                &graph,
                Graph::at(3),
                Graph::reference(),
                &Graph::declared(),
                &Type::Nil,
            );
            let expected = queries.union_reduced(&[Type::Number, Type::String]);
            assert_eq!(merge.ty, expected);
            assert!(!merge.incomplete);
            let loop_type = queries.flow_type(
                &graph,
                Graph::at(4),
                Graph::reference(),
                &Graph::declared(),
                &Type::Nil,
            );
            let expected = queries.union_reduced(&[Type::Nil, Type::Number]);
            assert_eq!(loop_type.ty, expected);
            let frozen = queries.flow_type(
                &graph,
                Graph::at(9),
                Graph::reference(),
                &Graph::declared(),
                &Type::Nil,
            );
            assert_eq!(frozen.ty, Type::Nil);
            let active_loop = queries.flow_type(
                &graph,
                Graph::at(10),
                Graph::reference(),
                &Graph::declared(),
                &Type::Nil,
            );
            assert_eq!(active_loop.ty, expected);
            let pruned = queries.flow_type(
                &graph,
                Graph::at(14),
                Graph::reference(),
                &Graph::declared(),
                &Type::Nil,
            );
            assert_eq!(pruned.ty, Type::Number);
            let false_loop = queries.flow_type(
                &graph,
                Graph::at(15),
                Graph::reference(),
                &Graph::declared(),
                &Type::Nil,
            );
            assert_eq!(false_loop.ty, Type::Nil);
            let truthy = queries.flow_type(
                &graph,
                Graph::at(6),
                Graph::reference(),
                &Graph::declared(),
                &Type::Nil,
            );
            assert_eq!(truthy.ty, Type::Never);
            assert!(queries.flow_reachable(&graph, Graph::at(3)));
            assert!(!queries.flow_reachable(&graph, Graph::at(7)));
            assert!(!queries.flow_reachable(&graph, Graph::at(8)));
            assert!(graph.reads.get() > 0);
            assert!(
                graph.errors.borrow().is_empty(),
                "{:?}",
                graph.errors.borrow()
            );
        });
    }

    struct FacetGraph {
        context: u64,
        value: Type,
    }
    impl FacetGraph {
        fn graph(&self) -> NativeGraphKey {
            NativeGraphKey {
                snapshot: 778,
                region: 0,
                context: self.context,
            }
        }
        fn at(&self, node: u32) -> NativeFlowKey {
            NativeFlowKey {
                graph: self.graph(),
                node,
            }
        }
        fn reference(&self) -> NativeReferenceKey {
            NativeReferenceKey {
                graph: self.graph(),
                reference: 0,
            }
        }
    }
    impl NativeFlowProvider for FacetGraph {
        fn contains_graph(&self, graph: NativeGraphKey) -> bool {
            graph == self.graph()
        }
        fn contains_reference(&self, reference: NativeReferenceKey) -> bool {
            reference == self.reference()
        }
        fn node(&self, at: NativeFlowKey) -> Option<NativeFlowNode> {
            if at.graph != self.graph() {
                return None;
            }
            Some(match at.node {
                0 => NativeFlowNode::Start,
                1 => NativeFlowNode::Write {
                    before: self.at(0),
                    target: self.reference(),
                    value: NativeValueKey {
                        snapshot: 778,
                        index: 0,
                    },
                    input: self.at(0),
                },
                2 | 3 => NativeFlowNode::Predicate {
                    before: self.at(0),
                    predicate: NativePredicate::Truthy {
                        subject: Some(self.reference()),
                        value: NativeValueKey {
                            snapshot: 778,
                            index: 0,
                        },
                        input: self.at(0),
                    },
                    sense: at.node == 2,
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
            } else {
                NativeWriteRelation::Unaffected
            }
        }
        fn value_type(
            &self,
            _: NativeValueKey,
            _: NativeFlowKey,
            queries: &mut dyn TypeQueries,
        ) -> Type {
            assert!(queries.is_assignable(&self.value, &Type::Unknown));
            self.value.clone()
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
            panic!("unexpected native flow error: {error:?}");
        }
    }

    #[test]
    fn flow_cache_facets_and_lua_truthiness_preserve_source_semantics() {
        Engine::new().with_session(|queries| {
            let declared = Type::Union(vec![Type::Number, Type::String, Type::Nil]);
            for (context, value) in [(1, Type::Number), (2, Type::String), (3, Type::Number)] {
                let graph = FacetGraph {
                    context,
                    value: value.clone(),
                };
                let actual = queries.flow_type(
                    &graph,
                    graph.at(1),
                    graph.reference(),
                    &declared,
                    &Type::Nil,
                );
                assert_eq!(actual.ty, value);
            }
            for (index, value) in [Type::NumberLiteral(0.0), Type::StringLiteral(Vec::new())]
                .into_iter()
                .enumerate()
            {
                let graph = FacetGraph {
                    context: 4 + index as u64,
                    value: value.clone(),
                };
                let yes = queries.flow_type(&graph, graph.at(2), graph.reference(), &value, &value);
                assert_eq!(yes.ty, value);
                let no = queries.flow_type(&graph, graph.at(3), graph.reference(), &value, &value);
                assert_eq!(no.ty, Type::Never);
                assert!(queries.flow_reachable(&graph, graph.at(2)));
                assert!(!queries.flow_reachable(&graph, graph.at(3)));
            }
            let unknown = FacetGraph {
                context: 6,
                value: Type::Unknown,
            };
            let truthy = queries
                .flow_type(
                    &unknown,
                    unknown.at(2),
                    unknown.reference(),
                    &Type::Unknown,
                    &Type::Unknown,
                )
                .ty;
            assert!(queries.is_assignable(&Type::NumberLiteral(0.0), &truthy));
            assert!(queries.is_assignable(&Type::StringLiteral(Vec::new()), &truthy));
            assert!(!queries.is_assignable(&Type::Nil, &truthy));
            assert!(!queries.is_assignable(&Type::BooleanLiteral(false), &truthy));
            assert!(queries.flow_reachable(&unknown, unknown.at(2)));
            assert!(queries.flow_reachable(&unknown, unknown.at(3)));
            for (index, value) in [Type::Nil, Type::BooleanLiteral(false)]
                .into_iter()
                .enumerate()
            {
                let graph = FacetGraph {
                    context: 7 + index as u64,
                    value,
                };
                assert!(!queries.flow_reachable(&graph, graph.at(2)));
                assert!(queries.flow_definitely_unreachable(&graph, graph.at(2)));
                assert!(queries.flow_reachable(&graph, graph.at(3)));
                assert!(!queries.flow_definitely_unreachable(&graph, graph.at(3)));
            }
        });
    }

    #[test]
    fn flow_sentinel_types_keep_their_exact_session_identity() {
        super::super::with_program(|program, handles| {
            let mut checker = program.checker();
            let silent = super::super::export_type(
                &mut checker,
                handles,
                bun_sema::types::TypeId::SILENT_NEVER,
            );
            assert!(matches!(silent, Type::SessionType(_)));
            let mut queries = SessionQueries {
                checker: &mut checker,
                handles,
            };
            assert_eq!(queries.union_reduced(std::slice::from_ref(&silent)), silent);
            assert_eq!(
                super::super::import_type(&mut checker, handles, &silent),
                bun_sema::types::TypeId::SILENT_NEVER
            );
        });
    }
}
#[cfg(test)]
mod effect_query_tests {
    use super::*;
    use crate::Engine;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    struct EffectsLoop {
        nested: bool,
        effects: Cell<usize>,
        incomplete_reads: Cell<usize>,
        errors: RefCell<Vec<NativeFlowError>>,
    }
    impl EffectsLoop {
        fn graph() -> NativeGraphKey {
            NativeGraphKey {
                snapshot: 44_001,
                region: 0,
                context: 0,
            }
        }
        fn at(node: u32) -> NativeFlowKey {
            NativeFlowKey {
                graph: Self::graph(),
                node,
            }
        }
        fn reference() -> NativeReferenceKey {
            NativeReferenceKey {
                graph: Self::graph(),
                reference: 0,
            }
        }
        fn value(index: u32) -> NativeValueKey {
            NativeValueKey {
                snapshot: 44_001,
                index,
            }
        }
        fn declared() -> Type {
            Type::Union(vec![Type::Number, Type::Nil])
        }
        fn read(&self, at: NativeFlowKey, queries: &mut dyn TypeQueries) -> NativeFlowResult {
            queries.flow_type(self, at, Self::reference(), &Self::declared(), &Type::Nil)
        }
    }
    impl NativeFlowProvider for EffectsLoop {
        fn contains_graph(&self, key: NativeGraphKey) -> bool {
            key.snapshot == Self::graph().snapshot && key.context == 0 && key.region < 2
        }
        fn contains_reference(&self, key: NativeReferenceKey) -> bool {
            self.contains_graph(key.graph) && key.reference == 0
        }
        fn node(&self, at: NativeFlowKey) -> Option<NativeFlowNode> {
            if !self.contains_graph(at.graph) {
                return None;
            }
            if at.graph.region == 1 {
                let node = |node| NativeFlowKey { graph: at.graph, node };
                let reference = NativeReferenceKey { graph: at.graph, reference: 0 };
                return Some(match at.node {
                    0 => NativeFlowNode::Start,
                    1 => NativeFlowNode::Write { before: node(0), target: reference, value: Self::value(0), input: node(0) },
                    2 => NativeFlowNode::Loop { edges: vec![node(1), node(3)] },
                    3 => NativeFlowNode::Write { before: node(2), target: reference, value: Self::value(1), input: node(2) },
                    _ => return None,
                });
            }
            Some(match at.node {
                0 => NativeFlowNode::Start,
                1 => NativeFlowNode::Write {
                    before: Self::at(0),
                    target: Self::reference(),
                    value: Self::value(0),
                    input: Self::at(0),
                },
                2 => NativeFlowNode::Loop {
                    edges: vec![Self::at(1), Self::at(4)],
                },
                3 => NativeFlowNode::Call {
                    before: Self::at(2),
                    value: Self::value(2),
                    input: Self::at(2),
                },
                4 => NativeFlowNode::Write {
                    before: Self::at(3),
                    target: Self::reference(),
                    value: Self::value(1),
                    input: Self::at(3),
                },
                _ => return None,
            })
        }
        fn is_shared(&self, at: NativeFlowKey) -> bool {
            at.node == 2
        }
        fn write_affects(
            &self,
            reference: NativeReferenceKey,
            target: NativeReferenceKey,
            _: NativeFlowKey,
        ) -> NativeWriteRelation {
            if reference == target {
                NativeWriteRelation::Exact
            } else {
                NativeWriteRelation::Unaffected
            }
        }
        fn value_type(
            &self,
            value: NativeValueKey,
            input: NativeFlowKey,
            queries: &mut dyn TypeQueries,
        ) -> Type {
            if input.graph.region == 1 {
                return if value.index == 0 { Type::Number } else { Type::Nil };
            }
            match value.index {
                0 => Type::Number,
                1 => {
                    let before = self.effects.get();
                    if self.nested {
                        let graph = NativeGraphKey { region: 1, ..Self::graph() };
                        let result = queries.flow_type(
                            self,
                            NativeFlowKey { graph, node: 2 },
                            NativeReferenceKey { graph, reference: 0 },
                            &Self::declared(),
                            &Type::Nil,
                        );
                        assert_eq!(result.ty, queries.union_reduced(&[Type::Number, Type::Nil]));
                    }
                    for _ in 0..8 {
                        assert!(queries.flow_reachable(self, Self::at(3)));
                        assert!(!queries.flow_definitely_unreachable(self, Self::at(3)));
                    }
                    assert_eq!(
                        self.effects.get(),
                        before,
                        "the same incomplete effect should be reused within its native loop epoch"
                    );
                    Type::Nil
                }
                _ => panic!("a call's result is not needed to classify effects"),
            }
        }
        fn call_effects(
            &self,
            _: NativeValueKey,
            input: NativeFlowKey,
            queries: &mut dyn TypeQueries,
        ) -> NativeCallEffects {
            self.effects.set(self.effects.get() + 1);
            let value = self.read(input, queries);
            if value.incomplete {
                self.incomplete_reads.set(self.incomplete_reads.get() + 1);
            }
            if value.ty == Type::Number {
                NativeCallEffects::Never
            } else {
                NativeCallEffects::None
            }
        }
        fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) {
            self.errors.borrow_mut().push(error);
        }
    }

    #[test]
    fn loop_owned_effect_provisionals_reuse_without_a_typescript_query_frame() {
        check_loop(false);
    }

    #[test]
    fn nested_loop_queries_restore_the_outer_effect_provisional_owner() {
        check_loop(true);
    }

    fn check_loop(nested: bool) {
        Engine::new().with_session(|queries| {
            let graph = EffectsLoop { nested, ..EffectsLoop::default() };
            let result = graph.read(EffectsLoop::at(2), queries);
            let expected = queries.union_reduced(&[Type::Number, Type::Nil]);
            assert_eq!(result.ty, expected);
            assert!(graph.incomplete_reads.get() > 0);
            assert!(
                graph.errors.borrow().is_empty(),
                "{:?}",
                graph.errors.borrow()
            );
            // The provisional Never seen before the nil backedge cannot become
            // a permanent unreachable call after the loop has settled.
            assert!(queries.flow_reachable(&graph, EffectsLoop::at(3)));
            let completed = graph.effects.get();
            assert!(queries.flow_reachable(&graph, EffectsLoop::at(3)));
            assert_eq!(graph.effects.get(), completed);
        });
    }
}
