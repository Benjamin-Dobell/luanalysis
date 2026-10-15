//! Scalar annotation reads must not accidentally demand physical return counts.

use crate::*;
use std::cell::Cell;

fn fixed(types: Vec<Type>) -> ValuePack {
    ValuePack::Alternatives(vec![PackAlternative {
        fixed: types.into_iter().map(PackSlot::required).collect(),
        tail: None,
    }])
}

#[derive(Default)]
struct Contracts {
    counts: Cell<usize>,
    cycles: Cell<usize>,
}

impl Contracts {
    fn function(index: u32) -> NativeFunctionKey {
        NativeFunctionKey {
            snapshot: 913,
            index,
        }
    }
    fn variable() -> Type {
        Type::NativeTypeParameter(NativeTypeParameterKey {
            snapshot: 913,
            index: 0,
        })
    }
}

impl NativeTypeProvider for Contracts {
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        key.snapshot == 913 && key.index < 8
    }
    fn function_has_result_pack(&self, _: NativeFunctionKey) -> bool {
        true
    }
    fn function_parameters(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        if key.index == 4 {
            vec![Parameter {
                name: "value".into(),
                ty: Self::variable(),
                optional: false,
            }]
        } else {
            Vec::new()
        }
    }
    fn function_declared_scalar_contract(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        match key.index {
            0 | 2 => Some(ValuePack::single(Type::Number)),
            1 => Some(ValuePack::empty()),
            4 | 5 => Some(ValuePack::single(Self::variable())),
            6 => Some(ValuePack::single(Type::Nil)),
            7 => Some(ValuePack::single(Type::Union(vec![
                Type::Number,
                Type::Void,
            ]))),
            _ => None,
        }
    }
    fn function_declared_return_pack(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        self.counts.set(self.counts.get() + 1);
        match key.index {
            0 => {
                let scalar = queries.call_native_arguments_scalar(
                    &Type::NativeFunction(key),
                    None,
                    &[],
                    &[],
                    None,
                );
                assert!(scalar.errors.is_empty());
                assert_eq!(scalar.return_type, Type::Number);
                Some(ValuePack::single(Type::Number))
            }
            1 => Some(ValuePack::empty()),
            2 | 6 | 7 => Some(
                queries
                    .function_result_pack(&Type::NativeFunction(key))
                    .unwrap(),
            ),
            4 | 5 => Some(ValuePack::single(Self::variable())),
            _ => None,
        }
    }
    fn function_inferred_return_pack(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        assert_eq!(key.index, 3);
        queries
            .function_result_pack(&Type::NativeFunction(key))
            .unwrap()
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        panic!("scalar legacy metadata must not replace a written pack")
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("source pack inference owns its body")
    }
    fn function_type_parameters(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<NativeTypeParameterKey> {
        if key.index == 4 {
            vec![NativeTypeParameterKey {
                snapshot: 913,
                index: 0,
            }]
        } else {
            Vec::new()
        }
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        key.snapshot == 913 && key.index < 2
    }
    fn type_parameter_name(&self, key: NativeTypeParameterKey) -> String {
        if key.index == 0 { "T" } else { "U" }.into()
    }
    fn function_outer_type_parameters(
        &self,
        key: NativeFunctionKey,
    ) -> Vec<NativeTypeParameterKey> {
        if key.index == 5 {
            (0..2)
                .map(|index| NativeTypeParameterKey {
                    snapshot: 913,
                    index,
                })
                .collect()
        } else {
            Vec::new()
        }
    }
    fn contains_binding(&self, key: NativeBindingKey) -> bool {
        key.snapshot == 913 && key.index < 2
    }
    fn contains_expression(&self, key: NativeExpressionKey) -> bool {
        key.snapshot == 913 && key.node == 0
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        self.cycles.set(self.cycles.get() + 1);
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("finite fixture")
    }
}

#[test]
fn scalar_declared_calls_do_not_enter_body_count_queries_and_void_projects_to_nil() {
    Engine::new().with_native_provider(Contracts::default, |provider, queries| {
        for (index, expected) in [(0, Type::Number), (1, Type::Nil)] {
            let function = Type::NativeFunction(Contracts::function(index));
            assert_eq!(
                queries.function_declared_scalar_return(&function),
                Some(expected.clone())
            );
            let called = queries.call_native_arguments_scalar(&function, None, &[], &[], None);
            assert!(called.errors.is_empty());
            assert_eq!(called.return_type, expected);
        }
        assert_eq!(provider.counts.get(), 0);
        assert_eq!(provider.cycles.get(), 0);
        let body = queries
            .function_result_pack(&Type::NativeFunction(Contracts::function(0)))
            .unwrap();
        assert_eq!(queries.pack_slot(&body, 0), Type::Number);
        assert_eq!(provider.counts.get(), 1);
        assert_eq!(provider.cycles.get(), 0);
    });
}

#[test]
fn annotated_count_cycles_recover_the_written_contract_without_scalar_cycle_diagnostics() {
    Engine::new().with_native_provider(Contracts::default, |provider, queries| {
        let annotated = Type::NativeFunction(Contracts::function(2));
        let pack = queries.function_result_pack(&annotated).unwrap();
        assert!(queries.pack_assignable(&pack, &ValuePack::single(Type::Number)));
        assert_eq!(queries.pack_slot(&pack, 0), Type::Number);
        assert_eq!(provider.cycles.get(), 0);
        let unannotated = Type::NativeFunction(Contracts::function(3));
        assert_eq!(queries.function_declared_scalar_return(&unannotated), None);
        let _ = queries.function_result_pack(&unannotated);
        assert_eq!(provider.cycles.get(), 1);
    });
}

#[test]
fn raw_generic_scalar_contracts_use_the_selected_native_signature_mapper() {
    Engine::new().with_native_provider(Contracts::default, |provider, queries| {
        let function = Type::NativeFunction(Contracts::function(4));
        for ty in [Type::Number, Type::String, Type::Boolean] {
            let called = queries.call_native_arguments_scalar(
                &function,
                None,
                &[PackArgument::Resolved(ty.clone())],
                &[],
                None,
            );
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            assert_eq!(called.return_type, ty);
        }
        assert_eq!(provider.counts.get(), 0);
    });
}

#[test]
fn universal_any_suffix_preserves_guaranteed_prefix_arity_and_prefix_type_checks() {
    Engine::new().with_session(|queries| {
        let any = queries.call_pack(&Type::Any, &[], &[], None).return_pack;
        let zero = queries.pack_function(&PackFunctionType {
            params: Vec::new(),
            rest: None,
            type_parameters: Vec::new(),
            returns: ValuePack::empty(),
        });
        let unknown_only = queries.call_native_arguments(
            &zero,
            None,
            &[PackArgument::Expand(any.clone())],
            &[],
            None,
        );
        assert!(unknown_only.errors.is_empty());
        let too_many = queries.call_native_arguments(
            &zero,
            None,
            &[
                PackArgument::Resolved(Type::Number),
                PackArgument::Expand(any.clone()),
            ],
            &[],
            None,
        );
        assert!(matches!(
            too_many.errors.as_slice(),
            [CallError::PackArity {
                actual_minimum: 1,
                minimum: 0,
                maximum: Some(0),
                ..
            }]
        ));
        let one_string = queries.pack_function(&PackFunctionType {
            params: vec![Parameter {
                name: "value".into(),
                ty: Type::String,
                optional: false,
            }],
            rest: None,
            type_parameters: Vec::new(),
            returns: ValuePack::empty(),
        });
        let mismatch = queries.call_native_arguments(
            &one_string,
            None,
            &[
                PackArgument::Resolved(Type::Number),
                PackArgument::Expand(any),
            ],
            &[],
            None,
        );
        assert!(matches!(
            mismatch.errors.as_slice(),
            [CallError::Argument { index: 0, .. }]
        ));
    });
}

#[test]
fn concatenating_unknown_dynamic_results_preserves_the_known_first_value() {
    Engine::new().with_session(|queries| {
        let any = queries.call_pack(&Type::Any, &[], &[], None).return_pack;
        let prefixed = queries.concat_pack(&[Type::Number], &any);
        assert_eq!(queries.pack_slot(&prefixed, 0), Type::Number);
        assert_eq!(queries.pack_slot(&prefixed, 1), Type::Any);
        let target = queries.pack_function(&PackFunctionType {
            params: Vec::new(),
            rest: None,
            type_parameters: Vec::new(),
            returns: ValuePack::empty(),
        });
        let called = queries.call_native_arguments(
            &target,
            None,
            &[PackArgument::Expand(prefixed)],
            &[],
            None,
        );
        assert!(matches!(
            called.errors.as_slice(),
            [CallError::PackArity {
                actual_minimum: 1,
                maximum: Some(0),
                ..
            }]
        ));
    });
}

#[test]
fn written_void_unions_normalize_to_zero_results_without_erasing_nil_slots() {
    Engine::new().with_session(|queries| {
        let written = ValuePack::single(Type::Union(vec![Type::Number, Type::Void]));
        let normalized = queries.declared_result_pack(&written);
        assert!(queries.pack_assignable(&ValuePack::empty(), &normalized));
        assert!(queries.pack_assignable(&ValuePack::single(Type::Number), &normalized));
        assert!(!queries.pack_assignable(&fixed(vec![Type::Number, Type::String]), &normalized));
        let nil = queries.declared_result_pack(&ValuePack::single(Type::Nil));
        assert!(!queries.pack_assignable(&ValuePack::empty(), &nil));
        assert_eq!(queries.pack_slot(&nil, 0), Type::Nil);
        let void = queries.declared_result_pack(&ValuePack::single(Type::Void));
        assert!(queries.pack_assignable(&ValuePack::empty(), &void));
        assert_eq!(queries.pack_slot(&void, 0), Type::Nil);
        assert_eq!(queries.inspect_pack(&void).unwrap()[0].fixed.len(), 0);
    });
}

#[test]
fn native_return_contract_types_retain_complete_actual_cardinality_rows() {
    Engine::new().with_session(|queries| {
        let declared = queries.declared_result_pack(&ValuePack::single(Type::Union(vec![
            Type::Number,
            Type::Void,
        ])));
        let empty = queries.return_pack_with_cardinality(&declared, &ValuePack::empty());
        assert_eq!(queries.inspect_pack(&empty).unwrap()[0].fixed.len(), 0);
        assert_eq!(queries.pack_slot(&empty, 0), Type::Nil);
        let value = queries
            .return_pack_with_cardinality(&declared, &ValuePack::single(Type::NumberLiteral(1.0)));
        assert_eq!(queries.pack_slot(&value, 0), Type::Number);
        assert!(!queries.pack_assignable(&ValuePack::empty(), &value));
        let mixed = queries.return_pack_with_cardinality(
            &declared,
            &ValuePack::Alternatives(vec![
                PackAlternative {
                    fixed: Vec::new(),
                    tail: None,
                },
                PackAlternative {
                    fixed: vec![PackSlot::required(Type::NumberLiteral(1.0))],
                    tail: None,
                },
            ]),
        );
        assert_eq!(queries.inspect_pack(&mixed).unwrap().len(), 2);
        assert!(queries.pack_assignable(&ValuePack::empty(), &mixed));
        assert!(queries.pack_assignable(&ValuePack::single(Type::Number), &mixed));
        assert!(!queries.pack_assignable(&fixed(vec![Type::Number, Type::String]), &mixed));
    });
}

#[test]
fn captured_and_contextual_pure_scalar_contracts_map_annotations_exactly_once() {
    crate::with_program(|program, handles| {
        let provider = Contracts::default();
        let adapter = crate::ProviderAdapter {
            provider: &provider,
            handles,
        };
        let mut checker = program.checker_with_native_provider(&adapter);
        checker.enable_native_arrays();
        let t = Contracts::variable();
        let u = Type::NativeTypeParameter(NativeTypeParameterKey {
            snapshot: 913,
            index: 1,
        });
        let captured = crate::SessionQueries {
            checker: &mut checker,
            handles,
        }
        .captured_function(
            Contracts::function(5),
            &[
                (
                    NativeBindingKey {
                        snapshot: 913,
                        index: 0,
                    },
                    t.clone(),
                ),
                (
                    NativeBindingKey {
                        snapshot: 913,
                        index: 1,
                    },
                    u.clone(),
                ),
            ],
        );
        let raw = crate::import_type(&mut checker, handles, &captured);
        let native_t = crate::import_type(&mut checker, handles, &t);
        let native_u = crate::import_type(&mut checker, handles, &u);
        let mapper = checker.types().mapper(vec![
            (native_t, native_u),
            (native_u, bun_sema::types::TypeId::NUMBER),
        ]);
        let mapped = checker.instantiate(raw, mapper);
        let mapped = crate::export_type(&mut checker, handles, mapped);
        let mut queries = crate::SessionQueries {
            checker: &mut checker,
            handles,
        };
        assert_eq!(
            queries.function_declared_scalar_return(&mapped),
            Some(u.clone())
        );
        let expected = Type::Function(FunctionType {
            params: Vec::new(),
            returns: Box::new(u.clone()),
        });
        let contextual = queries.contextual_function_value(
            &mapped,
            NativeExpressionKey {
                snapshot: 913,
                node: 0,
            },
            &expected,
            NativeCheckMode::empty(),
        );
        assert_eq!(
            queries.function_declared_scalar_return(&contextual),
            Some(u)
        );
        assert_eq!(provider.counts.get(), 0);
        assert_eq!(provider.cycles.get(), 0);
    });
}

#[test]
fn scalar_queries_reject_escaped_context_handles_and_cleanup_for_later_batches() {
    let escaped = Engine::new().with_native_provider(Contracts::default, |_, queries| {
        queries.captured_function(
            Contracts::function(5),
            &[(
                NativeBindingKey {
                    snapshot: 913,
                    index: 0,
                },
                Type::Number,
            )],
        )
    });
    let panic = std::panic::catch_unwind(|| {
        Engine::new().with_native_provider(Contracts::default, |_, queries| {
            queries.function_declared_scalar_return(&escaped)
        })
    })
    .unwrap_err();
    assert!(panic.is::<InvalidSessionType>());
    let value = Engine::new().with_native_provider(Contracts::default, |_, queries| {
        queries
            .call_native_arguments_scalar(
                &Type::NativeFunction(Contracts::function(0)),
                None,
                &[],
                &[],
                None,
            )
            .return_type
    });
    assert_eq!(value, Type::Number);
}

#[test]
fn dynamic_any_tail_assignment_keeps_fixed_prefix_and_unknown_suffix_values() {
    Engine::new().with_session(|queries| {
        let any = queries.call_pack(&Type::Any, &[], &[], None).return_pack;
        let packed = queries.concat_pack(&[Type::Number, Type::String], &any);
        let adjusted = queries.adjust_pack(&packed, 4).unwrap();
        assert_eq!(queries.pack_slot(&adjusted, 0), Type::Number);
        assert_eq!(queries.pack_slot(&adjusted, 1), Type::String);
        assert_eq!(queries.pack_slot(&adjusted, 2), Type::Any);
        assert_eq!(queries.pack_slot(&adjusted, 3), Type::Any);
        assert_eq!(queries.inspect_pack(&adjusted).unwrap().len(), 1);
        // Runtime arity was not rewritten by adjustment: the original pack
        // still guarantees two values and has an unknown number after them.
        let zero = queries.pack_function(&PackFunctionType {
            params: Vec::new(),
            rest: None,
            type_parameters: Vec::new(),
            returns: ValuePack::empty(),
        });
        let called =
            queries.call_native_arguments(&zero, None, &[PackArgument::Expand(packed)], &[], None);
        assert!(matches!(
            called.errors.as_slice(),
            [CallError::PackArity {
                actual_minimum: 2,
                actual_maximum: None,
                maximum: Some(0),
                ..
            }]
        ));
        let finite = queries
            .adjust_pack(&fixed(vec![Type::Number, Type::String]), 4)
            .unwrap();
        assert_eq!(queries.pack_slot(&finite, 2), Type::Nil);
        let numeric = queries.concat_pack(
            &[Type::Number, Type::String],
            &ValuePack::variadic(Type::Number),
        );
        let numeric = queries.adjust_pack(&numeric, 4).unwrap();
        let third = queries.pack_slot(&numeric, 2);
        assert!(queries.is_assignable(&Type::Number, &third));
        assert!(queries.is_assignable(&Type::Nil, &third));
        assert!(queries.inspect_pack(&numeric).unwrap().len() > 1);
    });
}

#[test]
fn annotated_nil_and_void_union_count_cycles_keep_written_alternative_cardinality() {
    Engine::new().with_native_provider(Contracts::default, |provider, queries| {
        for (index, expected_slot) in [(6, Type::Nil), (7, Type::Number)] {
            let function = Type::NativeFunction(Contracts::function(index));
            let pack = queries.function_result_pack(&function).unwrap();
            let alternatives = queries.inspect_pack(&pack).unwrap();
            assert_eq!(alternatives.len(), 2);
            assert!(
                alternatives
                    .iter()
                    .any(|row| row.fixed.is_empty() && row.tail.is_none())
            );
            let written = alternatives
                .iter()
                .find(|row| row.fixed.len() == 1)
                .unwrap();
            assert_eq!(written.fixed[0].ty, expected_slot);
            assert!(!written.fixed[0].optional);
            assert!(written.tail.is_none());
            assert!(queries.pack_assignable(&ValuePack::single(expected_slot), &pack));
            assert!(queries.pack_assignable(&ValuePack::empty(), &pack));
            assert!(!queries.pack_assignable(&fixed(vec![Type::Number, Type::Number]), &pack));
        }
        assert_eq!(provider.cycles.get(), 0);
    });
}

struct WrittenHeads;
impl WrittenHeads {
    fn key(index: u32) -> NativeSignatureKey {
        NativeSignatureKey {
            snapshot: 4917,
            index,
        }
    }
}
impl NativeTypeProvider for WrittenHeads {
    fn contains_function(&self, _: NativeFunctionKey) -> bool {
        false
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        panic!("no function body exists")
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        panic!("no function body exists")
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("no function body exists")
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("no function body exists")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("no function body exists")
    }
    fn contains_signature(&self, key: NativeSignatureKey) -> bool {
        key.snapshot == 4917 && key.index < 3
    }
    fn signature_header(&self, key: NativeSignatureKey) -> SignatureHeader {
        SignatureHeader {
            owner: SignatureOwner::Standalone,
            site: SignatureSite {
                snapshot: key.snapshot,
                index: key.index,
            },
            parent: None,
            has_direct_literal_parameter: false,
        }
    }
    fn signature_parameters(
        &self,
        _: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        panic!("a pure written return query must not check arguments or bodies")
    }
    fn signature_declared_return_pack(
        &self,
        key: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> ValuePack {
        if key.index == 2 {
            ValuePack::single(Type::Number)
        } else {
            ValuePack::never()
        }
    }
}

#[test]
fn original_written_never_head_has_a_pure_scalar_contract_without_argument_or_body_demands() {
    Engine::new().with_native_provider(
        || WrittenHeads,
        |_, queries| {
            let head = queries.signature_type(WrittenHeads::key(0));
            assert_eq!(
                queries.function_declared_scalar_return(&head),
                Some(Type::Never)
            );
        },
    );
}

#[test]
fn all_original_public_heads_must_be_never_before_a_pure_overload_flow_effect_is_claimed() {
    Engine::new().with_native_provider(
        || WrittenHeads,
        |_, queries| {
            let all_never =
                queries.signature_group_type(&[WrittenHeads::key(0), WrittenHeads::key(1)]);
            assert_eq!(
                queries.function_declared_scalar_return(&all_never),
                Some(Type::Never)
            );
            let mixed = queries.signature_group_type(&[WrittenHeads::key(0), WrittenHeads::key(2)]);
            assert_eq!(queries.function_declared_scalar_return(&mixed), None);
            let opposite =
                queries.signature_group_type(&[WrittenHeads::key(2), WrittenHeads::key(0)]);
            assert_eq!(queries.function_declared_scalar_return(&opposite), None);
        },
    );
}
