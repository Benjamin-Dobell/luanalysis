//! Packed signatures exercise the same call, relation, and inference workers
//! as source functions. No test infers a result by solving individual slots.

use crate::{
    CallError, Engine, NativeCheckMode, NativeExpressionKey, NativeFunctionKey,
    NativeTypeParameterKey, NativeTypeProvider, PackAlternative, PackArgument, PackFunctionType,
    PackSlot, Parameter, Type, TypeQueries, ValuePack,
};

fn fixed(types: Vec<Type>) -> ValuePack {
    ValuePack::Alternatives(vec![PackAlternative {
        fixed: types.into_iter().map(PackSlot::required).collect(),
        tail: None,
    }])
}

fn parameter(name: &str, ty: Type) -> Parameter {
    Parameter {
        name: name.to_owned(),
        ty,
        optional: false,
    }
}

fn packed_function(
    queries: &mut dyn TypeQueries,
    params: Vec<Parameter>,
    returns: ValuePack,
) -> Type {
    queries.pack_function(&PackFunctionType {
        params,
        rest: None,
        type_parameters: Vec::new(),
        returns,
    })
}

fn packed_signature_text(queries: &mut dyn TypeQueries, function: &Type) -> String {
    let signature = queries.format(function);
    let Some(pack) = queries.function_result_pack(function) else {
        return signature;
    };
    let first = queries.pack_slot(&pack, 0);
    let second = queries.pack_slot(&pack, 1);
    format!(
        "{signature}; result slots ({}, {})",
        queries.format(&first),
        queries.format(&second)
    )
}

#[derive(Default)]
struct SourceFunctions {
    scalar_demands: std::cell::Cell<usize>,
    pack_contexts: std::cell::RefCell<Vec<Option<ValuePack>>>,
}

impl SourceFunctions {
    const SNAPSHOT: u64 = 9_234;

    fn function(index: u32) -> NativeFunctionKey {
        NativeFunctionKey {
            snapshot: Self::SNAPSHOT,
            index,
        }
    }

    fn type_parameter(index: u32) -> NativeTypeParameterKey {
        NativeTypeParameterKey {
            snapshot: Self::SNAPSHOT,
            index,
        }
    }

    fn ty(index: u32) -> Type {
        Type::NativeTypeParameter(Self::type_parameter(index))
    }

    fn expression(node: u32) -> NativeExpressionKey {
        NativeExpressionKey {
            snapshot: Self::SNAPSHOT,
            node,
        }
    }
}

impl NativeTypeProvider for SourceFunctions {
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        key.snapshot == Self::SNAPSHOT && key.index < 7
    }

    fn contains_expression(&self, key: NativeExpressionKey) -> bool {
        key.snapshot == Self::SNAPSHOT && key.node < 4
    }

    fn check_expression(
        &self,
        key: NativeExpressionKey,
        expected: Option<&Type>,
        _: NativeCheckMode,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        assert_eq!(key, Self::expression(0));
        self.scalar_demands.set(self.scalar_demands.get() + 1);
        let expected = expected.map(|ty| ValuePack::single(ty.clone()));
        queries
            .call_pack(
                &Type::NativeFunction(Self::function(1)),
                &[],
                &[],
                expected.as_ref(),
            )
            .return_type
    }

    fn check_expression_pack(
        &self,
        key: NativeExpressionKey,
        expected: Option<&ValuePack>,
        _: NativeCheckMode,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        assert!(matches!(key.node, 0 | 2 | 3));
        if let Some(expected) = expected {
            assert!(matches!(expected, ValuePack::Session(_)));
        }
        self.pack_contexts.borrow_mut().push(expected.cloned());
        let function = match key.node {
            0 => 1,
            2 => 5,
            3 => 6,
            _ => unreachable!("validated pack expression demand"),
        };
        let called = queries.call_pack(
            &Type::NativeFunction(Self::function(function)),
            &[],
            &[],
            expected,
        );
        assert!(called.errors.is_empty(), "{:?}", called.errors);
        called.return_pack
    }

    fn function_has_result_pack(&self, _: NativeFunctionKey) -> bool {
        true
    }

    fn function_parameters(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        match key.index {
            // pair<T, U>(left: T, right: U): (T, U)
            0 => vec![
                parameter("left", Self::ty(0)),
                parameter("right", Self::ty(1)),
            ],
            // factory<T>(): (string, T): only the second result mentions T.
            1 => Vec::new(),
            // apply<T, U>(producer: () -> (T, U), consumer: (T, U) -> ()): (T, U)
            2 => {
                let producer =
                    packed_function(queries, Vec::new(), fixed(vec![Self::ty(3), Self::ty(4)]));
                let consumer = packed_function(
                    queries,
                    vec![
                        parameter("left", Self::ty(3)),
                        parameter("right", Self::ty(4)),
                    ],
                    ValuePack::empty(),
                );
                vec![
                    parameter("producer", producer),
                    parameter("consumer", consumer),
                ]
            }
            3 => vec![parameter("head", Type::Number)],
            // A base declaration body returns its declaration-space parameters.
            4 => vec![
                parameter("left", Self::ty(5)),
                parameter("right", Self::ty(6)),
            ],
            5 => Vec::new(),
            6 => Vec::new(),
            _ => unreachable!("validated source function identity"),
        }
    }

    fn function_declared_return_pack(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        if key.index == 4 {
            return None;
        }
        if key.index == 5 {
            // A written producer contract gives its returned callback a real
            // generic parameter type before callers provide any context.
            let callback = packed_function(
                queries,
                vec![parameter("value", Self::ty(7))],
                ValuePack::single(Self::ty(7)),
            );
            return Some(ValuePack::single(callback));
        }
        Some(match key.index {
            0 => fixed(vec![Self::ty(0), Self::ty(1)]),
            1 => fixed(vec![Type::String, Self::ty(2)]),
            2 => fixed(vec![Self::ty(3), Self::ty(4)]),
            3 => fixed(vec![Type::Number, Type::String]),
            6 => ValuePack::variadic(Self::ty(8)),
            _ => unreachable!("validated source function identity"),
        })
    }

    fn function_inferred_return_pack(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> ValuePack {
        match key.index {
            4 => fixed(vec![Self::ty(5), Self::ty(6)]),
            _ => panic!("written packed return avoids the body demand"),
        }
    }

    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        None
    }

    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("native packed signature must not use a scalar provider fallback")
    }

    fn function_type_parameters(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<NativeTypeParameterKey> {
        match key.index {
            0 => vec![Self::type_parameter(0), Self::type_parameter(1)],
            1 => vec![Self::type_parameter(2)],
            2 => vec![Self::type_parameter(3), Self::type_parameter(4)],
            3 => Vec::new(),
            4 => vec![Self::type_parameter(5), Self::type_parameter(6)],
            5 => vec![Self::type_parameter(7)],
            6 => vec![Self::type_parameter(8)],
            _ => unreachable!("validated source function identity"),
        }
    }

    fn function_rest_pack(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        (key.index == 3).then(|| ValuePack::variadic(Type::String))
    }

    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        key.snapshot == Self::SNAPSHOT && key.index < 9
    }

    fn type_parameter_name(&self, key: NativeTypeParameterKey) -> String {
        if matches!(key.index, 1 | 4 | 6) {
            "U"
        } else {
            "T"
        }
        .to_owned()
    }

    fn type_parameter_order(&self, key: NativeTypeParameterKey) -> u32 {
        match key.index {
            1 | 4 | 6 => 1,
            _ => 0,
        }
    }

    fn type_parameter_siblings(&self, key: NativeTypeParameterKey) -> Vec<NativeTypeParameterKey> {
        match key.index {
            0 | 1 => vec![Self::type_parameter(0), Self::type_parameter(1)],
            2 => vec![Self::type_parameter(2)],
            3 | 4 => vec![Self::type_parameter(3), Self::type_parameter(4)],
            5 | 6 => vec![Self::type_parameter(5), Self::type_parameter(6)],
            7 => vec![Self::type_parameter(7)],
            8 => vec![Self::type_parameter(8)],
            _ => unreachable!("validated source type parameter identity"),
        }
    }

    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("acyclic packed signature")
    }

    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("shallow packed signature")
    }
}

#[test]
fn inferred_generic_base_pack_applies_each_call_mapper_to_both_results() {
    Engine::new().with_native_provider(SourceFunctions::default, |_, queries| {
        let function = Type::NativeFunction(SourceFunctions::function(4));
        for (left, right) in [
            (Type::Number, Type::String),
            (Type::Boolean, Type::Number),
            (Type::String, Type::Boolean),
            (Type::Number, Type::String),
        ] {
            let called = queries.call_pack(&function, &[left.clone(), right.clone()], &[], None);
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            assert!(matches!(called.return_pack, ValuePack::Session(_)));
            assert_eq!(called.return_type, left);
            assert_eq!(queries.pack_slot(&called.return_pack, 0), left);
            assert_eq!(queries.pack_slot(&called.return_pack, 1), right);
            assert_eq!(queries.pack_slot(&called.return_pack, 2), Type::Nil);
            assert!(queries.pack_assignable(&called.return_pack, &fixed(vec![left, right])));
        }
    });
}

#[test]
fn generic_source_pair_preserves_the_second_result_and_independent_mappers() {
    Engine::new().with_native_provider(SourceFunctions::default, |_, queries| {
        let function = Type::NativeFunction(SourceFunctions::function(0));
        for (left, right) in [
            (Type::Number, Type::String),
            (Type::Boolean, Type::Number),
            (Type::Number, Type::String),
        ] {
            let called = queries.call_pack(&function, &[left.clone(), right.clone()], &[], None);
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            assert!(matches!(called.return_pack, ValuePack::Session(_)));
            assert_eq!(called.return_type, left);
            assert_eq!(queries.pack_slot(&called.return_pack, 1), right);
            assert_eq!(queries.pack_slot(&called.return_pack, 2), Type::Nil);
            assert!(queries.pack_assignable(&called.return_pack, &fixed(vec![left, right])));
        }
        let explicitly_instantiated = queries.call_pack(
            &function,
            &[Type::Number, Type::String],
            &[Type::Number, Type::String],
            None,
        );
        assert!(explicitly_instantiated.errors.is_empty());
        assert_eq!(
            queries.pack_slot(&explicitly_instantiated.return_pack, 1),
            Type::String
        );
        let wrong_second = queries.call_pack(
            &function,
            &[Type::Number, Type::Number],
            &[Type::Number, Type::String],
            None,
        );
        assert!(matches!(
            wrong_second.errors.as_slice(),
            [CallError::Argument { index: 1, .. }]
        ));
    });
}

#[test]
fn target_pack_inference_uses_a_type_parameter_in_the_second_result_only() {
    Engine::new().with_native_provider(SourceFunctions::default, |_, queries| {
        let function = Type::NativeFunction(SourceFunctions::function(1));
        for payload in [Type::Number, Type::Boolean, Type::String] {
            let expected = fixed(vec![Type::String, payload.clone()]);
            let called = queries.call_pack(&function, &[], &[], Some(&expected));
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            assert_eq!(called.return_type, Type::String);
            assert_eq!(queries.pack_slot(&called.return_pack, 1), payload);
            assert!(queries.pack_assignable(&called.return_pack, &expected));
        }
        let signature = queries.function_result_pack(&function).unwrap();
        assert!(matches!(signature, ValuePack::Session(_)));
    });
}

#[test]
fn target_variadic_pack_infers_a_zero_argument_generic_factory_tail() {
    Engine::new().with_native_provider(SourceFunctions::default, |_, queries| {
        let function = Type::NativeFunction(SourceFunctions::function(6));
        for element in [Type::Number, Type::String, Type::Boolean] {
            let expected = ValuePack::variadic(element.clone());
            let called = queries.call_pack(&function, &[], &[], Some(&expected));
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            let alternatives = queries.inspect_pack(&called.return_pack).unwrap();
            assert_eq!(alternatives.len(), 1);
            assert!(alternatives[0].fixed.is_empty());
            assert_eq!(alternatives[0].tail.as_deref(), Some(&element));
            assert!(queries.pack_assignable(&called.return_pack, &expected));
            let projected = queries.pack_slot(&called.return_pack, 4);
            let nullable = Type::Union(vec![element.clone(), Type::Nil]);
            assert!(queries.is_assignable(&projected, &nullable));
            assert!(queries.is_assignable(&element, &projected));
            assert!(queries.is_assignable(&Type::Nil, &projected));
            assert!(!queries.is_assignable(&projected, &element));
            assert!(queries.is_assignable(&called.return_type, &nullable));
        }
    });
}

#[test]
fn target_variadic_pack_reaches_a_delayed_factory_after_a_scalar_prefix() {
    Engine::new().with_native_provider(SourceFunctions::default, |provider, queries| {
        for element in [Type::Number, Type::String, Type::Boolean] {
            let consumer = queries.pack_function(&PackFunctionType {
                params: vec![parameter("prefix", Type::Boolean)],
                rest: Some(ValuePack::variadic(element.clone())),
                type_parameters: Vec::new(),
                returns: ValuePack::empty(),
            });
            let called = queries.call_native_arguments(
                &consumer,
                Some(SourceFunctions::expression(1)),
                &[
                    PackArgument::Resolved(Type::Boolean),
                    PackArgument::ExpandExpression(SourceFunctions::expression(3)),
                ],
                &[],
                None,
            );
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            let contexts = provider.pack_contexts.borrow().clone();
            assert!(contexts.iter().any(Option::is_none));
            let expected = contexts.into_iter().flatten().last().unwrap();
            let alternatives = queries.inspect_pack(&expected).unwrap();
            assert_eq!(alternatives.len(), 1);
            assert!(alternatives[0].fixed.is_empty());
            assert_eq!(alternatives[0].tail.as_deref(), Some(&element));
        }
    });
}

#[test]
fn public_packed_rest_signatures_use_native_arity_and_argument_checks() {
    Engine::new().with_session(|queries| {
        let function = queries.pack_function(&PackFunctionType {
            params: vec![parameter("head", Type::Boolean)],
            rest: Some(fixed(vec![Type::Number, Type::String])),
            type_parameters: Vec::new(),
            returns: ValuePack::empty(),
        });
        assert!(matches!(function, Type::SessionType(_)));
        let called = queries.call_pack(
            &function,
            &[Type::Boolean, Type::Number, Type::String],
            &[],
            None,
        );
        assert!(called.errors.is_empty(), "{:?}", called.errors);
        assert_eq!(called.return_type, Type::Nil);
        let missing = queries.call_pack(&function, &[Type::Boolean], &[], None);
        assert!(matches!(
            missing.errors.as_slice(),
            [CallError::Arity {
                actual: 1,
                minimum: 3,
                maximum: Some(3)
            }]
        ));
        let excessive = queries.call_pack(
            &function,
            &[Type::Boolean, Type::Number, Type::String, Type::Boolean],
            &[],
            None,
        );
        assert!(matches!(
            excessive.errors.as_slice(),
            [CallError::Arity {
                actual: 4,
                minimum: 3,
                maximum: Some(3)
            }]
        ));
        let wrong = queries.call_pack(
            &function,
            &[Type::Boolean, Type::Number, Type::Number],
            &[],
            None,
        );
        assert!(matches!(
            wrong.errors.as_slice(),
            [CallError::Argument { index: 2, .. }]
        ));

        let optional = queries.pack_function(&PackFunctionType {
            params: vec![parameter("head", Type::Boolean)],
            rest: Some(ValuePack::Alternatives(vec![PackAlternative {
                fixed: vec![
                    PackSlot::required(Type::Number),
                    PackSlot {
                        ty: Type::String,
                        optional: true,
                    },
                ],
                tail: None,
            }])),
            type_parameters: Vec::new(),
            returns: ValuePack::empty(),
        });
        for arguments in [
            vec![Type::Boolean, Type::Number],
            vec![Type::Boolean, Type::Number, Type::String],
        ] {
            let called = queries.call_pack(&optional, &arguments, &[], None);
            assert!(called.errors.is_empty(), "{:?}", called.errors);
        }
        let rest = queries.signature_rest_pack(&optional).unwrap();
        let last = queries.pack_slot(&rest, 1);
        assert!(queries.is_assignable(&Type::String, &last));
        assert!(queries.is_assignable(&Type::Nil, &last));
    });
}

#[test]
fn variadic_packed_calls_check_every_argument_and_scalarize_possible_absence_to_nil() {
    Engine::new().with_session(|queries| {
        let function = queries.pack_function(&PackFunctionType {
            params: vec![parameter("head", Type::Boolean)],
            rest: Some(ValuePack::variadic(Type::Number)),
            type_parameters: Vec::new(),
            returns: ValuePack::variadic(Type::Number),
        });
        for arguments in [
            vec![Type::Boolean],
            vec![Type::Boolean, Type::Number, Type::Number],
        ] {
            let called = queries.call_pack(&function, &arguments, &[], None);
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            assert!(queries.is_assignable(&Type::Number, &called.return_type));
            assert!(queries.is_assignable(&Type::Nil, &called.return_type));
            let later = queries.pack_slot(&called.return_pack, 8);
            assert!(queries.is_assignable(&Type::Number, &later));
            assert!(queries.is_assignable(&Type::Nil, &later));
        }
        let wrong = queries.call_pack(
            &function,
            &[Type::Boolean, Type::Number, Type::String],
            &[],
            None,
        );
        assert!(matches!(
            wrong.errors.as_slice(),
            [CallError::Argument { index: 2, .. }]
        ));
        let prefixed = packed_function(
            queries,
            Vec::new(),
            ValuePack::Alternatives(vec![PackAlternative {
                fixed: vec![PackSlot::required(Type::String)],
                tail: Some(Box::new(Type::Number)),
            }]),
        );
        let called = queries.call_pack(&prefixed, &[], &[], None);
        assert!(called.errors.is_empty());
        assert_eq!(called.return_type, Type::String);
        let tail = queries.pack_slot(&called.return_pack, 1);
        assert!(queries.is_assignable(&Type::Nil, &tail));
    });
}

#[test]
fn source_provider_rest_packs_use_the_same_call_scheduler_as_built_signatures() {
    Engine::new().with_native_provider(SourceFunctions::default, |_, queries| {
        let function = Type::NativeFunction(SourceFunctions::function(3));
        for arguments in [
            vec![Type::Number],
            vec![Type::Number, Type::String, Type::String],
        ] {
            let called = queries.call_pack(&function, &arguments, &[], None);
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            assert_eq!(called.return_type, Type::Number);
            assert_eq!(queries.pack_slot(&called.return_pack, 1), Type::String);
        }
        let wrong = queries.call_pack(
            &function,
            &[Type::Number, Type::String, Type::Number],
            &[],
            None,
        );
        assert!(matches!(
            wrong.errors.as_slice(),
            [CallError::Argument { index: 2, .. }]
        ));
        let missing = queries.call_pack(&function, &[], &[], None);
        assert!(matches!(
            missing.errors.as_slice(),
            [CallError::Arity {
                actual: 0,
                minimum: 1,
                maximum: None
            }]
        ));
        let rest = queries.signature_rest_pack(&function).unwrap();
        let projected = queries.pack_slot(&rest, 0);
        assert!(queries.is_assignable(&Type::String, &projected));
        assert!(queries.is_assignable(&Type::Nil, &projected));
    });
}

#[test]
fn packed_producer_relations_preserve_payloads_and_whole_return_alternatives() {
    Engine::new().with_session(|queries| {
        let producer =
            packed_function(queries, Vec::new(), fixed(vec![Type::Number, Type::String]));
        let matching =
            packed_function(queries, Vec::new(), fixed(vec![Type::Number, Type::String]));
        let wrong_payload =
            packed_function(queries, Vec::new(), fixed(vec![Type::Number, Type::Number]));
        assert!(queries.is_assignable(&producer, &matching));
        assert!(!queries.is_assignable(&producer, &wrong_payload));
        let correlated = packed_function(
            queries,
            Vec::new(),
            ValuePack::Alternatives(vec![
                PackAlternative {
                    fixed: vec![
                        PackSlot::required(Type::BooleanLiteral(false)),
                        PackSlot::required(Type::String),
                    ],
                    tail: None,
                },
                PackAlternative {
                    fixed: vec![
                        PackSlot::required(Type::BooleanLiteral(true)),
                        PackSlot::required(Type::Number),
                    ],
                    tail: None,
                },
            ]),
        );
        let crossed = packed_function(
            queries,
            Vec::new(),
            fixed(vec![Type::BooleanLiteral(false), Type::Number]),
        );
        assert!(!queries.is_assignable(&crossed, &correlated));
    });
}

#[test]
fn higher_order_source_calls_infer_packed_producers_and_check_consumers() {
    Engine::new().with_native_provider(SourceFunctions::default, |_, queries| {
        let source = Type::NativeFunction(SourceFunctions::function(2));
        let producer =
            packed_function(queries, Vec::new(), fixed(vec![Type::Number, Type::String]));
        let consumer = packed_function(
            queries,
            vec![
                parameter("left", Type::Number),
                parameter("right", Type::String),
            ],
            ValuePack::empty(),
        );
        let called = queries.call_pack(&source, &[producer.clone(), consumer], &[], None);
        let relation_errors: Vec<_> = called
            .errors
            .iter()
            .map(|error| match error {
                CallError::Argument {
                    index,
                    source,
                    target,
                    ..
                } => format!(
                    "argument {index}: {} -> {}",
                    packed_signature_text(queries, source),
                    packed_signature_text(queries, target)
                ),
                other => format!("{other:?}"),
            })
            .collect();
        assert!(called.errors.is_empty(), "{relation_errors:#?}");
        assert_eq!(called.return_type, Type::Number);
        assert_eq!(queries.pack_slot(&called.return_pack, 1), Type::String);
        let ignores_arguments = packed_function(queries, Vec::new(), ValuePack::empty());
        let ignored = queries.call_pack(&source, &[producer.clone(), ignores_arguments], &[], None);
        assert!(ignored.errors.is_empty(), "{:?}", ignored.errors);
        assert_eq!(queries.pack_slot(&ignored.return_pack, 1), Type::String);
        let wrong_consumer = packed_function(
            queries,
            vec![
                parameter("left", Type::Number),
                parameter("right", Type::Number),
            ],
            ValuePack::empty(),
        );
        let wrong = queries.call_pack(
            &source,
            &[producer.clone(), wrong_consumer.clone()],
            &[],
            None,
        );
        assert!(matches!(
            wrong.errors.as_slice(),
            [CallError::Argument { .. }]
        ));
        let explicitly_instantiated = queries.call_pack(
            &source,
            &[producer, wrong_consumer],
            &[Type::Number, Type::String],
            None,
        );
        assert!(matches!(
            explicitly_instantiated.errors.as_slice(),
            [CallError::Argument { index: 1, .. }]
        ));
    });
}

#[test]
fn returning_a_packed_callable_keeps_its_exact_signature_in_the_same_session() {
    Engine::new().with_session(|queries| {
        let inner = packed_function(queries, Vec::new(), fixed(vec![Type::Number, Type::String]));
        let outer = packed_function(queries, Vec::new(), ValuePack::single(inner));
        let returned = queries.call_pack(&outer, &[], &[], None);
        assert!(returned.errors.is_empty());
        assert!(matches!(returned.return_type, Type::SessionType(_)));
        let called = queries.call_pack(&returned.return_type, &[], &[], None);
        assert!(called.errors.is_empty());
        assert_eq!(called.return_type, Type::Number);
        assert_eq!(queries.pack_slot(&called.return_pack, 1), Type::String);
        let from_pack = queries.pack_slot(&returned.return_pack, 0);
        assert!(matches!(from_pack, Type::SessionType(_)));
        assert!(queries.is_assignable(&from_pack, &returned.return_type));
    });
}

#[test]
fn expanded_static_generic_calls_infer_both_parameters_from_one_argument_pack() {
    Engine::new().with_native_provider(SourceFunctions::default, |_, queries| {
        let function = Type::NativeFunction(SourceFunctions::function(0));
        for (left, right) in [(Type::Number, Type::String), (Type::Boolean, Type::Number)] {
            let called = queries.call_native_arguments(
                &function,
                None,
                &[PackArgument::Expand(fixed(vec![
                    left.clone(),
                    right.clone(),
                ]))],
                &[],
                None,
            );
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            assert_eq!(called.return_type, left);
            assert_eq!(queries.pack_slot(&called.return_pack, 1), right);
            let forwarded = queries.call_native_arguments(
                &function,
                None,
                &[PackArgument::Expand(called.return_pack)],
                &[],
                None,
            );
            assert!(forwarded.errors.is_empty(), "{:?}", forwarded.errors);
            assert_eq!(forwarded.return_type, left);
            assert_eq!(queries.pack_slot(&forwarded.return_pack, 1), right);
        }
        let prefixed = queries.call_native_arguments(
            &function,
            None,
            &[
                PackArgument::Resolved(Type::Number),
                PackArgument::Expand(ValuePack::single(Type::String)),
            ],
            &[],
            None,
        );
        assert!(prefixed.errors.is_empty(), "{:?}", prefixed.errors);
        assert_eq!(queries.pack_slot(&prefixed.return_pack, 1), Type::String);
    });
}

#[test]
fn expanded_correlated_rest_arguments_compare_whole_pairs() {
    Engine::new().with_session(|queries| {
        let alternatives = ValuePack::Alternatives(vec![
            PackAlternative {
                fixed: vec![
                    PackSlot::required(Type::BooleanLiteral(false)),
                    PackSlot::required(Type::String),
                ],
                tail: None,
            },
            PackAlternative {
                fixed: vec![
                    PackSlot::required(Type::BooleanLiteral(true)),
                    PackSlot::required(Type::Number),
                ],
                tail: None,
            },
        ]);
        let consumer = queries.pack_function(&PackFunctionType {
            params: Vec::new(),
            rest: Some(alternatives.clone()),
            type_parameters: Vec::new(),
            returns: ValuePack::empty(),
        });
        let valid = queries.call_native_arguments(
            &consumer,
            None,
            &[PackArgument::Expand(alternatives)],
            &[],
            None,
        );
        assert!(valid.errors.is_empty(), "{:?}", valid.errors);
        let crossed = queries.call_native_arguments(
            &consumer,
            None,
            &[PackArgument::Expand(fixed(vec![
                Type::BooleanLiteral(false),
                Type::Number,
            ]))],
            &[],
            None,
        );
        assert!(matches!(
            crossed.errors.as_slice(),
            [CallError::Argument { index: 0, .. }]
        ));
        let mixed = ValuePack::Alternatives(vec![
            PackAlternative {
                fixed: vec![
                    PackSlot::required(Type::BooleanLiteral(false)),
                    PackSlot::required(Type::String),
                ],
                tail: None,
            },
            PackAlternative {
                fixed: vec![
                    PackSlot::required(Type::BooleanLiteral(true)),
                    PackSlot::required(Type::String),
                ],
                tail: None,
            },
        ]);
        let invalid_alternative = queries.call_native_arguments(
            &consumer,
            None,
            &[PackArgument::Expand(mixed)],
            &[],
            None,
        );
        assert!(matches!(
            invalid_alternative.errors.as_slice(),
            [CallError::Argument { .. }]
        ));
    });
}

#[test]
fn expanded_pack_arity_preserves_zero_one_optional_and_variadic_counts() {
    Engine::new().with_session(|queries| {
        let nil_consumer = packed_function(
            queries,
            vec![parameter("value", Type::Nil)],
            ValuePack::empty(),
        );
        let zero_results = queries.call_native_arguments(
            &nil_consumer,
            None,
            &[PackArgument::Expand(ValuePack::empty())],
            &[],
            None,
        );
        assert!(matches!(
            zero_results.errors.as_slice(),
            [CallError::PackArity {
                actual_minimum: 0,
                actual_maximum: Some(0),
                minimum: 1,
                maximum: Some(1),
                ..
            }]
        ));
        let explicit_nil = queries.call_native_arguments(
            &nil_consumer,
            None,
            &[PackArgument::Expand(ValuePack::single(Type::Nil))],
            &[],
            None,
        );
        assert!(explicit_nil.errors.is_empty());
        let scalar_nil = queries.pack_slot(&ValuePack::empty(), 0);
        let scalarized = queries.call_native_arguments(
            &nil_consumer,
            None,
            &[PackArgument::Resolved(scalar_nil)],
            &[],
            None,
        );
        assert!(scalarized.errors.is_empty());
        let strict = packed_function(
            queries,
            vec![
                parameter("number", Type::Number),
                parameter("text", Type::String),
            ],
            ValuePack::empty(),
        );
        let optional = ValuePack::Alternatives(vec![PackAlternative {
            fixed: vec![
                PackSlot::required(Type::Number),
                PackSlot {
                    ty: Type::String,
                    optional: true,
                },
            ],
            tail: None,
        }]);
        let maybe_missing = queries.call_native_arguments(
            &strict,
            None,
            &[PackArgument::Expand(optional.clone())],
            &[],
            None,
        );
        assert!(matches!(
            maybe_missing.errors.as_slice(),
            [CallError::PackArity {
                actual_minimum: 1,
                actual_maximum: Some(2),
                minimum: 2,
                maximum: Some(2),
                ..
            }]
        ));
        let mut text = parameter("text", Type::String);
        text.optional = true;
        let optional_consumer = packed_function(
            queries,
            vec![parameter("number", Type::Number), text],
            ValuePack::empty(),
        );
        let accepts_optional = queries.call_native_arguments(
            &optional_consumer,
            None,
            &[PackArgument::Expand(optional)],
            &[],
            None,
        );
        assert!(
            accepts_optional.errors.is_empty(),
            "{:?}",
            accepts_optional.errors
        );
        let variadic = queries.pack_function(&PackFunctionType {
            params: Vec::new(),
            rest: Some(ValuePack::variadic(Type::Number)),
            type_parameters: Vec::new(),
            returns: ValuePack::empty(),
        });
        let tail = queries.call_native_arguments(
            &variadic,
            None,
            &[PackArgument::Expand(ValuePack::variadic(Type::Number))],
            &[],
            None,
        );
        assert!(tail.errors.is_empty(), "{:?}", tail.errors);
        let zero_consumer = packed_function(queries, Vec::new(), ValuePack::empty());
        let excessive_tail = queries.call_native_arguments(
            &zero_consumer,
            None,
            &[PackArgument::Expand(ValuePack::variadic(Type::Number))],
            &[],
            None,
        );
        // Lua drops values past the parameters: a variadic tail is no excess.
        assert!(excessive_tail.errors.is_empty(), "{:?}", excessive_tail.errors);
    });
}

#[test]
fn expanded_lazy_producer_receives_the_whole_remaining_parameter_context() {
    Engine::new().with_native_provider(SourceFunctions::default, |provider, queries| {
        for payload in [Type::Number, Type::Boolean, Type::String] {
            let consumer = packed_function(
                queries,
                vec![
                    parameter("prefix", Type::Boolean),
                    parameter("text", Type::String),
                    parameter("payload", payload.clone()),
                ],
                ValuePack::empty(),
            );
            let called = queries.call_native_arguments(
                &consumer,
                Some(SourceFunctions::expression(1)),
                &[
                    PackArgument::Resolved(Type::Boolean),
                    PackArgument::ExpandExpression(SourceFunctions::expression(0)),
                ],
                &[],
                None,
            );
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            let contexts = provider.pack_contexts.borrow().clone();
            assert!(contexts.iter().any(Option::is_none));
            let remaining = contexts.into_iter().flatten().last().unwrap();
            assert!(matches!(remaining, ValuePack::Session(_)));
            assert_eq!(queries.pack_slot(&remaining, 0), Type::String);
            assert_eq!(queries.pack_slot(&remaining, 1), payload);
            assert_eq!(queries.pack_slot(&remaining, 2), Type::Nil);
        }
        assert_eq!(provider.scalar_demands.get(), 0);
    });
}

#[test]
fn expanded_expression_mode_does_not_expand_an_ordinary_scalar_argument() {
    Engine::new().with_native_provider(SourceFunctions::default, |provider, queries| {
        let single = packed_function(
            queries,
            vec![parameter("text", Type::String)],
            ValuePack::empty(),
        );
        let scalar = queries.call_native_arguments(
            &single,
            Some(SourceFunctions::expression(1)),
            &[PackArgument::Expression(SourceFunctions::expression(0))],
            &[],
            None,
        );
        assert!(scalar.errors.is_empty(), "{:?}", scalar.errors);
        assert!(provider.scalar_demands.get() > 0);
        assert!(provider.pack_contexts.borrow().is_empty());
        let pair = packed_function(
            queries,
            vec![
                parameter("text", Type::String),
                parameter("payload", Type::Number),
            ],
            ValuePack::empty(),
        );
        let truncated = queries.call_native_arguments(
            &pair,
            None,
            &[PackArgument::Expression(SourceFunctions::expression(0))],
            &[],
            None,
        );
        assert!(matches!(
            truncated.errors.as_slice(),
            [CallError::Arity {
                actual: 1,
                minimum: 2,
                maximum: Some(2)
            }]
        ));
        let expanded = queries.call_native_arguments(
            &pair,
            None,
            &[PackArgument::ExpandExpression(SourceFunctions::expression(
                0,
            ))],
            &[],
            None,
        );
        assert!(expanded.errors.is_empty(), "{:?}", expanded.errors);
    });
}

#[test]
fn expanded_written_callback_producer_keeps_contextual_generic_return_metadata() {
    Engine::new().with_native_provider(SourceFunctions::default, |provider, queries| {
        for ty in [Type::Number, Type::String] {
            let callback = packed_function(
                queries,
                vec![parameter("value", ty.clone())],
                ValuePack::single(ty.clone()),
            );
            let consumer = packed_function(
                queries,
                vec![
                    parameter("prefix", Type::Boolean),
                    parameter("callback", callback),
                ],
                ValuePack::empty(),
            );
            let called = queries.call_native_arguments(
                &consumer,
                Some(SourceFunctions::expression(1)),
                &[
                    PackArgument::Resolved(Type::Boolean),
                    PackArgument::ExpandExpression(SourceFunctions::expression(2)),
                ],
                &[],
                None,
            );
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            let context = provider
                .pack_contexts
                .borrow()
                .iter()
                .flatten()
                .last()
                .cloned()
                .unwrap();
            let factory = queries.call_pack(
                &Type::NativeFunction(SourceFunctions::function(5)),
                &[],
                &[],
                Some(&context),
            );
            assert!(factory.errors.is_empty(), "{:?}", factory.errors);
            assert!(matches!(factory.return_type, Type::SessionType(_)));
            let invoke = queries.call_pack(&factory.return_type, &[ty.clone()], &[], None);
            assert!(invoke.errors.is_empty(), "{:?}", invoke.errors);
            assert_eq!(invoke.return_type, ty);
        }
    });
}

#[test]
fn native_callable_unions_accept_gradual_any_without_erasing_other_parameters() {
    Engine::new().with_session(|queries| {
        let mut members = Vec::new();
        for ty in [Type::Number, Type::String] {
            members.push(packed_function(
                queries,
                vec![
                    parameter("value", ty.clone()),
                    parameter("flag", Type::Boolean),
                ],
                fixed(vec![ty]),
            ));
        }
        for member in &members {
            let direct = queries.call_pack(member, &[Type::Any, Type::Boolean], &[], None);
            assert!(direct.errors.is_empty(), "{:?}", direct.errors);
        }
        let callable = Type::Union(members);
        let accepted = queries.call_pack(&callable, &[Type::Any, Type::Boolean], &[], None);
        assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
        for ty in [Type::Number, Type::String] {
            assert!(queries.is_assignable(&ty, &accepted.return_type));
        }
        assert!(!queries.is_assignable(&Type::Boolean, &accepted.return_type));
        for ty in [Type::Number, Type::Unknown, Type::NumberLiteral(1.0)] {
            let rejected = queries.call_pack(&callable, &[ty, Type::Boolean], &[], None);
            assert!(matches!(
                rejected.errors.as_slice(),
                [CallError::Argument { index: 0, .. }]
            ));
        }
        let wrong_second = queries.call_pack(&callable, &[Type::Any, Type::String], &[], None);
        assert!(matches!(
            wrong_second.errors.as_slice(),
            [CallError::Argument { index: 1, .. }]
        ));
        for args in [
            vec![Type::Any],
            vec![Type::Any, Type::Boolean, Type::Boolean],
        ] {
            let rejected = queries.call_pack(&callable, &args, &[], None);
            assert!(matches!(
                rejected.errors.as_slice(),
                [CallError::Arity { .. }]
            ));
        }
        let forbidden = packed_function(
            queries,
            vec![
                parameter("value", Type::Never),
                parameter("flag", Type::Boolean),
            ],
            ValuePack::empty(),
        );
        let explicit_never = Type::Union(vec![callable.clone(), forbidden]);
        let rejected = queries.call_pack(&explicit_never, &[Type::Any, Type::Boolean], &[], None);
        assert!(matches!(
            rejected.errors.as_slice(),
            [CallError::Argument { index: 0, .. }]
        ));
        let corrected = queries.call_pack(&callable, &[Type::Any, Type::Boolean], &[], None);
        assert!(corrected.errors.is_empty());
    });
}

#[test]
fn native_union_any_expansions_and_tuple_rests_preserve_result_rows() {
    Engine::new().with_session(|queries| {
        let returns = [
            fixed(vec![Type::Number, Type::String]),
            fixed(vec![Type::String, Type::Number]),
        ];
        for tuple_rest in [false, true] {
            let mut members = Vec::new();
            for (index, ty) in [Type::Number, Type::String].into_iter().enumerate() {
                members.push(queries.pack_function(&PackFunctionType {
                    params: if tuple_rest {
                        Vec::new()
                    } else {
                        vec![
                            parameter("value", ty.clone()),
                            parameter("flag", Type::Boolean),
                        ]
                    },
                    rest: tuple_rest.then(|| fixed(vec![ty, Type::Boolean])),
                    type_parameters: Vec::new(),
                    returns: returns[index].clone(),
                }));
            }
            let callable = Type::Union(members);
            let expected = ValuePack::Alternatives(vec![
                PackAlternative {
                    fixed: vec![
                        PackSlot::required(Type::Number),
                        PackSlot::required(Type::String),
                    ],
                    tail: None,
                },
                PackAlternative {
                    fixed: vec![
                        PackSlot::required(Type::String),
                        PackSlot::required(Type::Number),
                    ],
                    tail: None,
                },
            ]);
            for args in [
                vec![
                    PackArgument::Resolved(Type::Any),
                    PackArgument::Resolved(Type::Boolean),
                ],
                vec![PackArgument::Expand(fixed(vec![Type::Any, Type::Boolean]))],
            ] {
                let accepted = queries.call_native_arguments(&callable, None, &args, &[], None);
                assert!(
                    accepted.errors.is_empty(),
                    "tuple_rest={tuple_rest}: {:?}",
                    accepted.errors
                );
                assert!(queries.pack_assignable(&accepted.return_pack, &expected));
                assert!(queries.pack_assignable(&expected, &accepted.return_pack));
                assert!(!queries.pack_assignable(
                    &accepted.return_pack,
                    &fixed(vec![Type::Number, Type::Number])
                ));
            }
            let wrong = queries.call_native_arguments(
                &callable,
                None,
                &[PackArgument::Expand(fixed(vec![Type::Any, Type::String]))],
                &[],
                None,
            );
            assert!(matches!(
                wrong.errors.as_slice(),
                [CallError::Argument { .. }]
            ));
            let unknown = queries.call_native_arguments(
                &callable,
                None,
                &[PackArgument::Expand(fixed(vec![
                    Type::Unknown,
                    Type::Boolean,
                ]))],
                &[],
                None,
            );
            assert!(!unknown.errors.is_empty());
        }
    });
}
