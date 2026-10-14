//! Original-source graph records with owned types at the checker boundary.

use super::{Checker, SessionQueries, Type, TypeHandles, TypeQueries, import_type};
use bun_sema::check::native_flow as core;

pub use core::{
    NativeCallEffects, NativeFlowError, NativeFlowKey, NativeFlowNode, NativeGraphKey,
    NativeNumericConstant, NativeNumericInput, NativeNumericIterationStage, NativePredicate,
    NativeReferenceKey, NativeRuntimeType, NativeValueKey, NativeWriteRelation,
};

#[derive(Debug, Clone, PartialEq)]
pub struct NativeFlowResult {
    pub ty: Type,
    pub incomplete: bool,
}

/// Immutable graph recipes and source-value demands. No graph join, loop
/// iteration, or narrowing algorithm is performed by this interface.
pub trait NativeFlowProvider {
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
                assert!(queries.flow_reachable(&graph, graph.at(3)));
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
