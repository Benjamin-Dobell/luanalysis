use super::*;

fn signature(index: u32) -> NativeSignatureKey {
    NativeSignatureKey {
        snapshot: 11_231,
        index,
    }
}
fn parameter(index: u32) -> NativeTypeParameterKey {
    NativeTypeParameterKey {
        snapshot: 11_231,
        index,
    }
}
fn fixed(values: &[Type]) -> ValuePack {
    ValuePack::Alternatives(vec![PackAlternative {
        fixed: values.iter().cloned().map(PackSlot::required).collect(),
        tail: None,
    }])
}
struct Generic;
impl NativeTypeProvider for Generic {
    fn contains_signature(&self, key: NativeSignatureKey) -> bool {
        key.snapshot == 11_231 && key.index < 2
    }
    fn signature_header(&self, key: NativeSignatureKey) -> SignatureHeader {
        SignatureHeader {
            owner: SignatureOwner::Standalone,
            site: SignatureSite {
                snapshot: 11_231,
                index: key.index,
            },
            parent: None,
            has_direct_literal_parameter: false,
        }
    }
    fn signature_type_parameters(&self, key: NativeSignatureKey) -> Vec<NativeTypeParameterKey> {
        if key.index == 0 {
            vec![parameter(0)]
        } else {
            vec![parameter(2), parameter(3)]
        }
    }
    fn signature_parameters(
        &self,
        key: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        if key.index == 0 {
            vec![Parameter {
                name: "value".into(),
                ty: Type::NativeTypeParameter(parameter(0)),
                optional: true,
            }]
        } else {
            vec![
                Parameter {
                    name: "first".into(),
                    ty: Type::NativeTypeParameter(parameter(2)),
                    optional: false,
                },
                Parameter {
                    name: "second".into(),
                    ty: Type::NativeTypeParameter(parameter(3)),
                    optional: false,
                },
            ]
        }
    }
    fn signature_rest_pack(
        &self,
        _: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        Some(ValuePack::variadic(Type::Unknown))
    }
    fn signature_declared_return_pack(
        &self,
        key: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> ValuePack {
        if key.index == 0 {
            ValuePack::single(Type::Union(vec![
                Type::NativeTypeParameter(parameter(0)),
                Type::Nil,
            ]))
        } else {
            fixed(&[
                Type::NativeTypeParameter(parameter(2)),
                Type::NativeTypeParameter(parameter(3)),
            ])
        }
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        key.snapshot == 11_231 && key.index < 4
    }
    fn type_parameter_name(&self, key: NativeTypeParameterKey) -> String {
        format!("T{}", key.index)
    }
    fn type_parameter_owner(&self, key: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> {
        Some(TypeDeclarationOwner::Signature(signature(u32::from(
            key.index >= 2,
        ))))
    }
    fn type_parameter_siblings(&self, key: NativeTypeParameterKey) -> Vec<NativeTypeParameterKey> {
        if key.index < 2 {
            vec![parameter(0)]
        } else {
            vec![parameter(2), parameter(3)]
        }
    }
    fn contains_function(&self, _: NativeFunctionKey) -> bool {
        false
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        panic!("original written signature only")
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        panic!("original written signature only")
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("original written signature only")
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("finite original written signature")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("finite original written signature")
    }
}

#[test]
fn native_correlated_expansion_infers_one_union_candidate_for_each_fixed_parameter() {
    Engine::new().with_native_provider(
        || Generic,
        |_, queries| {
            let callee = queries.signature_type(signature(0));
            let input = queries.join_packs(&[
                fixed(&[Type::BooleanLiteral(false), Type::String]),
                fixed(&[Type::Number, Type::Boolean]),
                fixed(&[Type::Nil]),
                ValuePack::empty(),
            ]);
            let result = queries.call_native_arguments(
                &callee,
                None,
                &[PackArgument::Expand(input)],
                &[],
                None,
            );
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            for value in [Type::BooleanLiteral(false), Type::Number, Type::Nil] {
                assert!(
                    queries.is_assignable(&value, &result.return_type),
                    "{:?}",
                    result.return_type
                );
            }
            assert!(!queries.is_assignable(&Type::String, &result.return_type));

            let two = queries.signature_type(signature(1));
            let input = queries.join_packs(&[
                fixed(&[Type::BooleanLiteral(false), Type::String]),
                fixed(&[Type::Number, Type::Boolean]),
            ]);
            let result = queries.call_native_arguments(
                &two,
                None,
                &[PackArgument::Expand(input)],
                &[],
                None,
            );
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            let first = queries.pack_slot(&result.return_pack, 0);
            let second = queries.pack_slot(&result.return_pack, 1);
            assert!(queries.is_assignable(&Type::BooleanLiteral(false), &first));
            assert!(queries.is_assignable(&Type::Number, &first));
            assert!(!queries.is_assignable(&Type::String, &first));
            assert!(queries.is_assignable(&Type::Boolean, &second));
            assert!(queries.is_assignable(&Type::String, &second));
            assert!(!queries.is_assignable(&Type::Number, &second));
        },
    );
}

#[test]
fn fixed_parameter_projection_does_not_invent_missing_required_arguments() {
    Engine::new().with_native_provider(
        || Generic,
        |_, queries| {
            let callee = queries.signature_type(signature(1));
            let input =
                queries.join_packs(&[fixed(&[Type::Number, Type::String]), ValuePack::empty()]);
            let result = queries.call_native_arguments(
                &callee,
                None,
                &[PackArgument::Expand(input)],
                &[],
                None,
            );
            assert!(
                matches!(result.errors.as_slice(), [CallError::PackArity { .. }]),
                "{:?}",
                result.errors
            );
        },
    );
}
