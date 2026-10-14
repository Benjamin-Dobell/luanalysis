//! Generic-for queries preserve the checker's opaque correlated hidden frame.

use super::native_flow::FlowProviderAdapter;
use super::{
    NativeFlowKey, NativeFlowProvider, NativeFlowResult, NativeIteratorProjection,
    NativeReferenceKey, SessionQueries, Type, ValuePack, import_type,
};

impl SessionQueries<'_, '_, '_> {
    pub(super) fn flow_iterator_pack_native(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        frame: NativeReferenceKey,
    ) -> ValuePack {
        let adapter = FlowProviderAdapter {
            provider,
            handles: self.handles,
        };
        let pack = self.checker.native_flow_iterator_pack(&adapter, at, frame);
        self.handles.retain_pack(self.checker, pack)
    }

    pub(super) fn iterator_result_native(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        frame: NativeReferenceKey,
        index: usize,
    ) -> Type {
        let adapter = FlowProviderAdapter {
            provider,
            handles: self.handles,
        };
        let ty = self
            .checker
            .native_iterator_result(&adapter, at, frame, index);
        self.handles.retain(ty)
    }

    pub(super) fn iterator_result_with_status_native(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        frame: NativeReferenceKey,
        index: usize,
    ) -> NativeFlowResult {
        let adapter = FlowProviderAdapter {
            provider,
            handles: self.handles,
        };
        let result = self
            .checker
            .native_iterator_result_with_status(&adapter, at, frame, index);
        NativeFlowResult {
            ty: self.handles.retain(result.ty),
            incomplete: result.incomplete,
            unresolved_auto: result.unresolved_auto,
        }
    }

    pub(super) fn flow_iterator_projection_native(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        projection: NativeIteratorProjection,
        ordinary: &Type,
    ) -> NativeFlowResult {
        let adapter = FlowProviderAdapter {
            provider,
            handles: self.handles,
        };
        let ordinary = import_type(self.checker, self.handles, ordinary);
        let result = self
            .checker
            .native_flow_iterator_projection(&adapter, at, projection, ordinary);
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
    use crate::{
        CallError, Engine, NativeCallEffects, NativeFlowError, NativeFlowNode, NativeGraphKey,
        NativeIteratorStage, NativePredicate, NativeValueKey, NativeWriteRelation, PackAlternative,
        PackFunctionType, PackSlot, Parameter, TypeQueries,
    };
    use std::cell::{Cell, RefCell};

    struct Graph {
        header: ValuePack,
        looping: bool,
        header_reads: Cell<usize>,
        errors: RefCell<Vec<CallError>>,
        origins: RefCell<Vec<NativeIteratorStage>>,
        flow_errors: RefCell<Vec<NativeFlowError>>,
        foreign_header: bool,
        foreign_input: bool,
        foreign_frame: bool,
        query_body: bool,
        incomplete_reads: Cell<usize>,
    }

    impl Graph {
        fn graph() -> NativeGraphKey {
            NativeGraphKey {
                snapshot: 913,
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
        fn reference(reference: u32) -> NativeReferenceKey {
            NativeReferenceKey {
                graph: Self::graph(),
                reference,
            }
        }
        fn new(header: ValuePack, looping: bool) -> Self {
            Self {
                header,
                looping,
                header_reads: Cell::new(0),
                errors: RefCell::new(Vec::new()),
                origins: RefCell::new(Vec::new()),
                flow_errors: RefCell::new(Vec::new()),
                foreign_header: false,
                foreign_input: false,
                foreign_frame: false,
                query_body: false,
                incomplete_reads: Cell::new(0),
            }
        }
    }

    impl NativeFlowProvider for Graph {
        fn contains_graph(&self, graph: NativeGraphKey) -> bool {
            graph == Self::graph()
        }
        fn contains_reference(&self, reference: NativeReferenceKey) -> bool {
            reference.graph == Self::graph() && reference.reference <= 2
        }
        fn node(&self, at: NativeFlowKey) -> Option<NativeFlowNode> {
            if at.graph != Self::graph() {
                return None;
            }
            Some(match at.node {
                0 => NativeFlowNode::Start,
                1 => {
                    let mut frame = Self::reference(0);
                    if self.foreign_frame {
                        frame.graph.snapshot += 1;
                    }
                    let mut input = Self::at(0);
                    if self.foreign_input {
                        input.graph.snapshot += 1;
                    }
                    NativeFlowNode::IteratorStart {
                        before: Self::at(0),
                        frame,
                        header: NativeValueKey {
                            snapshot: 913 + u64::from(self.foreign_header),
                            index: 0,
                        },
                        input,
                    }
                }
                2 => NativeFlowNode::Iteration {
                    before: Self::at(1),
                    frame: Self::reference(0),
                    stage: NativeIteratorStage::First,
                },
                3 | 6 => NativeFlowNode::Predicate {
                    before: Self::at(2),
                    predicate: NativePredicate::GenericIteration {
                        frame: Self::reference(0),
                        input: Self::at(2),
                    },
                    sense: at.node == 3,
                },
                4 => NativeFlowNode::Loop {
                    edges: if self.looping {
                        vec![Self::at(3), Self::at(9)]
                    } else {
                        vec![Self::at(3)]
                    },
                },
                5 if self.query_body => NativeFlowNode::Predicate {
                    before: Self::at(4),
                    predicate: NativePredicate::Truthy {
                        subject: Some(Self::reference(1)),
                        value: NativeValueKey {
                            snapshot: 913,
                            index: 2,
                        },
                        input: Self::at(4),
                    },
                    sense: true,
                },
                5 => NativeFlowNode::Write {
                    before: Self::at(4),
                    target: Self::reference(1),
                    value: NativeValueKey {
                        snapshot: 913,
                        index: 1,
                    },
                    input: Self::at(4),
                },
                7 => NativeFlowNode::Predicate {
                    before: Self::at(4),
                    predicate: NativePredicate::Truthy {
                        subject: Some(Self::reference(1)),
                        value: NativeValueKey {
                            snapshot: 913,
                            index: 2,
                        },
                        input: Self::at(4),
                    },
                    sense: true,
                },
                8 => NativeFlowNode::Iteration {
                    before: Self::at(5),
                    frame: Self::reference(0),
                    stage: NativeIteratorStage::Next,
                },
                9 | 10 => NativeFlowNode::Predicate {
                    before: Self::at(8),
                    predicate: NativePredicate::GenericIteration {
                        frame: Self::reference(0),
                        input: Self::at(8),
                    },
                    sense: at.node == 9,
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
            value: NativeValueKey,
            input: NativeFlowKey,
            queries: &mut dyn TypeQueries,
        ) -> Type {
            match value.index {
                1 => Type::String,
                2 => {
                    let result =
                        queries.iterator_result_with_status(self, input, Self::reference(0), 0);
                    if result.incomplete {
                        self.incomplete_reads.set(self.incomplete_reads.get() + 1);
                    }
                    result.ty
                }
                _ => panic!("header must retain its full result pack"),
            }
        }
        fn value_pack(
            &self,
            value: NativeValueKey,
            input: NativeFlowKey,
            _: &mut dyn TypeQueries,
        ) -> ValuePack {
            assert_eq!(value.index, 0);
            assert_eq!(input, Self::at(0));
            self.header_reads.set(self.header_reads.get() + 1);
            self.header.clone()
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
            self.flow_errors.borrow_mut().push(error);
        }
        fn iterator_error(
            &self,
            _: NativeFlowKey,
            stage: NativeIteratorStage,
            _: &Type,
            error: CallError,
            _: &mut dyn TypeQueries,
        ) {
            self.errors.borrow_mut().push(error);
            self.origins.borrow_mut().push(stage);
        }
        fn iterator_projection(
            &self,
            reference: NativeReferenceKey,
        ) -> Option<NativeIteratorProjection> {
            (reference == Self::reference(1)).then_some(NativeIteratorProjection {
                frame: Self::reference(0),
                index: 2,
            })
        }
    }

    fn row(types: Vec<Type>) -> PackAlternative {
        PackAlternative {
            fixed: types.into_iter().map(PackSlot::required).collect(),
            tail: None,
        }
    }
    fn function(
        queries: &mut dyn TypeQueries,
        state: Type,
        control: Type,
        returns: ValuePack,
    ) -> Type {
        queries.pack_function(&PackFunctionType {
            params: vec![
                Parameter {
                    name: "state".into(),
                    ty: state,
                    optional: false,
                },
                Parameter {
                    name: "control".into(),
                    ty: control,
                    optional: false,
                },
            ],
            rest: None,
            type_parameters: Vec::new(),
            returns,
        })
    }

    #[test]
    fn generic_iteration_enters_on_false_and_terminates_only_on_nil() {
        Engine::new().with_session(|queries| {
            let returns = ValuePack::Alternatives(vec![
                row(vec![Type::BooleanLiteral(false), Type::String]),
                row(vec![Type::Nil]),
            ]);
            let iterator = function(queries, Type::Nil, Type::Nil, returns);
            let graph = Graph::new(ValuePack::Alternatives(vec![row(vec![iterator])]), false);
            let first = queries.iterator_result(&graph, Graph::at(4), Graph::reference(0), 0);
            assert_eq!(queries.inspect_type(&first), Type::BooleanLiteral(false));
            let second = queries.iterator_result(&graph, Graph::at(4), Graph::reference(0), 1);
            assert_eq!(queries.inspect_type(&second), Type::String);
            assert!(queries.flow_reachable(&graph, Graph::at(4)));
            assert!(queries.flow_reachable(&graph, Graph::at(6)));
            assert!(
                graph.errors.borrow().is_empty(),
                "{:?}",
                graph.errors.borrow()
            );
            assert!(graph.flow_errors.borrow().is_empty());
        });
    }

    #[test]
    fn generic_iteration_zero_results_never_enter_body() {
        Engine::new().with_session(|queries| {
            let iterator = function(queries, Type::Nil, Type::Nil, ValuePack::empty());
            let graph = Graph::new(ValuePack::Alternatives(vec![row(vec![iterator])]), true);
            assert!(!queries.flow_reachable(&graph, Graph::at(4)));
            assert!(queries.flow_reachable(&graph, Graph::at(6)));
            assert!(graph.errors.borrow().is_empty());
        });
    }

    #[test]
    fn generic_iteration_keeps_header_iterator_state_and_result_rows_correlated() {
        Engine::new().with_session(|queries| {
            let left = function(
                queries,
                Type::String,
                Type::Nil,
                ValuePack::Alternatives(vec![row(vec![Type::BooleanLiteral(true), Type::Number])]),
            );
            let right = function(
                queries,
                Type::Number,
                Type::Nil,
                ValuePack::Alternatives(vec![row(vec![Type::BooleanLiteral(false), Type::String])]),
            );
            let graph = Graph::new(
                ValuePack::Alternatives(vec![
                    row(vec![left, Type::String, Type::Nil]),
                    row(vec![right, Type::Number, Type::Nil]),
                ]),
                false,
            );
            let second = queries.iterator_result(&graph, Graph::at(4), Graph::reference(0), 1);
            assert!(queries.is_assignable(&Type::Number, &second));
            assert!(queries.is_assignable(&Type::String, &second));
            assert!(
                graph.errors.borrow().is_empty(),
                "{:?}",
                graph.errors.borrow()
            );
            let narrowed = queries.flow_iterator_projection(
                &graph,
                Graph::at(7),
                NativeIteratorProjection {
                    frame: Graph::reference(0),
                    index: 3,
                },
                &second,
            );
            assert_eq!(queries.inspect_type(&narrowed.ty), Type::Number);
        });
    }

    #[test]
    fn generic_iteration_hidden_control_feedback_ignores_visible_body_writes() {
        Engine::new().with_session(|queries| {
            let iterator = function(
                queries,
                Type::Nil,
                Type::Number,
                ValuePack::Alternatives(vec![row(vec![Type::Number, Type::Boolean])]),
            );
            let graph = Graph::new(
                ValuePack::Alternatives(vec![row(vec![iterator, Type::Nil, Type::Number])]),
                true,
            );
            let control = queries.iterator_result(&graph, Graph::at(4), Graph::reference(0), 0);
            assert_eq!(queries.inspect_type(&control), Type::Number);
            let value = queries.iterator_result(&graph, Graph::at(4), Graph::reference(0), 1);
            assert_eq!(queries.inspect_type(&value), Type::Boolean);
            assert!(
                graph.errors.borrow().is_empty(),
                "{:?}",
                graph.errors.borrow()
            );
            assert!(
                graph.flow_errors.borrow().is_empty(),
                "{:?}",
                graph.flow_errors.borrow()
            );
        });
    }

    #[test]
    fn generic_iteration_checks_feedback_control_against_iterator_parameter() {
        Engine::new().with_session(|queries| {
            let iterator = function(
                queries,
                Type::Nil,
                Type::Nil,
                ValuePack::Alternatives(vec![row(vec![Type::Number])]),
            );
            let graph = Graph::new(ValuePack::Alternatives(vec![row(vec![iterator])]), true);
            let _ = queries.iterator_result(&graph, Graph::at(4), Graph::reference(0), 0);
            assert!(
                graph
                    .errors
                    .borrow()
                    .iter()
                    .any(|error| matches!(error, CallError::Argument { index: 1, .. })),
                "{:?}",
                graph.errors.borrow()
            );
            assert!(
                graph
                    .origins
                    .borrow()
                    .iter()
                    .all(|stage| *stage == NativeIteratorStage::Next)
            );
        });
    }

    #[test]
    fn generic_iteration_reports_non_callable_header_and_foreign_frame() {
        Engine::new().with_session(|queries| {
            let graph = Graph::new(ValuePack::single(Type::String), false);
            let _ = queries.iterator_result(&graph, Graph::at(4), Graph::reference(0), 0);
            assert!(
                graph
                    .errors
                    .borrow()
                    .iter()
                    .any(|error| matches!(error, CallError::NotCallable))
            );
            let mut foreign = Graph::reference(0);
            foreign.graph.snapshot += 1;
            let _ = queries.flow_iterator_pack(&graph, Graph::at(4), foreign);
            assert!(
                graph
                    .flow_errors
                    .borrow()
                    .iter()
                    .any(|error| matches!(error, NativeFlowError::InvalidReference(_)))
            );
        });
    }

    #[test]
    fn generic_iteration_never_returns_has_no_body_or_exit() {
        Engine::new().with_session(|queries| {
            let iterator = function(queries, Type::Nil, Type::Nil, ValuePack::never());
            let graph = Graph::new(ValuePack::Alternatives(vec![row(vec![iterator])]), true);
            assert!(!queries.flow_reachable(&graph, Graph::at(4)));
            assert!(!queries.flow_reachable(&graph, Graph::at(6)));
            assert!(graph.errors.borrow().is_empty());
        });
    }

    #[test]
    fn generic_iteration_variadic_returns_keep_both_nil_exit_and_non_nil_entry() {
        Engine::new().with_session(|queries| {
            let iterator = function(
                queries,
                Type::Nil,
                Type::Nil,
                ValuePack::variadic(Type::Number),
            );
            let graph = Graph::new(ValuePack::Alternatives(vec![row(vec![iterator])]), false);
            assert!(queries.flow_reachable(&graph, Graph::at(4)));
            assert!(queries.flow_reachable(&graph, Graph::at(6)));
            let exit = queries.flow_iterator_pack(&graph, Graph::at(6), Graph::reference(0));
            let exit_rows = queries.inspect_pack(&exit).unwrap();
            assert_eq!(exit_rows.len(), 1);
            assert_eq!(exit_rows[0].fixed.len(), 2);
            assert!(exit_rows[0].tail.is_none());
            let control = queries.iterator_result(&graph, Graph::at(4), Graph::reference(0), 0);
            assert_eq!(queries.inspect_type(&control), Type::Number);
            let next = queries.iterator_result(&graph, Graph::at(4), Graph::reference(0), 1);
            assert!(queries.is_assignable(&Type::Number, &next));
            assert!(queries.is_assignable(&Type::Nil, &next));
        });
    }

    #[test]
    fn generic_iteration_rejects_foreign_header_input_and_frame_before_source_demand() {
        Engine::new().with_session(|queries| {
            for field in 0..3 {
                let mut graph = Graph::new(ValuePack::empty(), false);
                match field {
                    0 => graph.foreign_header = true,
                    1 => graph.foreign_input = true,
                    _ => graph.foreign_frame = true,
                }
                let _ = queries.flow_iterator_pack(&graph, Graph::at(1), Graph::reference(0));
                assert_eq!(graph.header_reads.get(), 0);
                assert_eq!(graph.flow_errors.borrow().len(), 1);
            }
        });
    }

    #[test]
    fn generic_iteration_initial_argument_error_has_first_stage_origin() {
        Engine::new().with_session(|queries| {
            let iterator = function(queries, Type::Number, Type::Nil, ValuePack::empty());
            let graph = Graph::new(
                ValuePack::Alternatives(vec![row(vec![iterator, Type::String])]),
                false,
            );
            let _ = queries.iterator_result(&graph, Graph::at(4), Graph::reference(0), 0);
            assert!(
                graph
                    .errors
                    .borrow()
                    .iter()
                    .any(|error| matches!(error, CallError::Argument { index: 0, .. }))
            );
            assert!(
                graph
                    .origins
                    .borrow()
                    .iter()
                    .all(|stage| *stage == NativeIteratorStage::First)
            );
        });
    }

    #[test]
    fn generic_iteration_dynamic_result_count_keeps_hidden_iterator_and_state() {
        for dynamic_callee in [true, false] {
            // Every immutable graph identity belongs to one source snapshot;
            // use separate sessions for these two different source recipes.
            Engine::new().with_session(move |queries| {
                let dynamic = queries.call_pack(&Type::Any, &[], &[], None).return_pack;
                let known = function(queries, Type::String, Type::Any, dynamic);
                let iterator = if dynamic_callee { Type::Any } else { known };
                let graph = Graph::new(
                    ValuePack::Alternatives(vec![row(vec![
                        iterator.clone(),
                        Type::String,
                        Type::Nil,
                    ])]),
                    true,
                );
                let called = queries.flow_iterator_pack(&graph, Graph::at(2), Graph::reference(0));
                let called_rows = queries
                    .inspect_pack(&called)
                    .expect("known prefix and dynamic return count remain inspectable");
                assert_eq!(called_rows.len(), 1);
                assert_eq!(called_rows[0].fixed.len(), 2);
                assert!(called_rows[0].fixed.iter().all(|slot| !slot.optional));
                assert_eq!(
                    queries.inspect_type(called_rows[0].tail.as_ref().unwrap()),
                    Type::Any
                );
                for _ in 0..2 {
                    let result =
                        queries.iterator_result(&graph, Graph::at(4), Graph::reference(0), 0);
                    assert_eq!(queries.inspect_type(&result), Type::Any);
                    let frame =
                        queries.flow_iterator_pack(&graph, Graph::at(4), Graph::reference(0));
                    let rows = queries
                        .inspect_pack(&frame)
                        .expect("known hidden frame must survive a dynamic result count");
                    assert!(!rows.is_empty());
                    for row in rows {
                        assert_eq!(row.fixed.len(), 3);
                        assert!(row.fixed.iter().all(|slot| !slot.optional));
                        assert_eq!(queries.inspect_type(&row.fixed[1].ty), Type::String);
                        assert_eq!(queries.format(&row.fixed[0].ty), queries.format(&iterator));
                        assert!(row.tail.is_some());
                    }
                }
                assert!(queries.flow_reachable(&graph, Graph::at(4)));
                assert!(queries.flow_reachable(&graph, Graph::at(6)));
                assert!(
                    graph.errors.borrow().is_empty(),
                    "{:?}",
                    graph.errors.borrow()
                );
                assert!(
                    graph.flow_errors.borrow().is_empty(),
                    "{:?}",
                    graph.flow_errors.borrow()
                );
            });
        }
    }

    #[test]
    fn generic_iteration_provisional_body_queries_retain_incomplete_status() {
        Engine::new().with_session(|queries| {
            let iterator = function(
                queries,
                Type::Nil,
                Type::Number,
                ValuePack::Alternatives(vec![row(vec![Type::Number])]),
            );
            let mut graph = Graph::new(
                ValuePack::Alternatives(vec![row(vec![iterator, Type::Nil, Type::Number])]),
                true,
            );
            graph.query_body = true;
            let result =
                queries.iterator_result_with_status(&graph, Graph::at(4), Graph::reference(0), 0);
            assert_eq!(queries.inspect_type(&result.ty), Type::Number);
            assert!(
                graph.incomplete_reads.get() > 0,
                "body callback must see the in-progress loop result"
            );
            assert!(graph.flow_errors.borrow().is_empty());
        });
    }
}
