use super::*;

fn signature(index: u32) -> NativeSignatureKey {
    NativeSignatureKey {
        snapshot: 11_119,
        index,
    }
}
struct Overloads;
impl NativeTypeProvider for Overloads {
    fn contains_signature(&self, key: NativeSignatureKey) -> bool {
        key.snapshot == 11_119 && key.index < 2
    }
    fn signature_header(&self, key: NativeSignatureKey) -> SignatureHeader {
        SignatureHeader {
            owner: SignatureOwner::Standalone,
            site: SignatureSite {
                snapshot: 11_119,
                index: key.index,
            },
            parent: None,
            has_direct_literal_parameter: false,
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
                ty: Type::Any,
                optional: false,
            }]
        } else {
            vec![
                Parameter {
                    name: "value".into(),
                    ty: Type::String,
                    optional: false,
                },
                Parameter {
                    name: "base".into(),
                    ty: Type::Number,
                    optional: false,
                },
            ]
        }
    }
    fn signature_declared_return_pack(
        &self,
        _: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> ValuePack {
        ValuePack::single(Type::Union(vec![Type::Number, Type::Nil]))
    }
    fn contains_function(&self, _: NativeFunctionKey) -> bool {
        false
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        panic!("written heads have no runtime implementation")
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        panic!("written heads have no runtime implementation")
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("written heads have no runtime implementation")
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("written heads are finite")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("written heads are finite")
    }
}

#[test]
fn overload_failure_diagnostics_keep_each_actual_heads_parameter_and_arity_correlation() {
    for order in [[signature(0), signature(1)], [signature(1), signature(0)]] {
        Engine::new().with_native_provider(
            || Overloads,
            |_, queries| {
                let function = queries.signature_group_type(&order);
                for valid in [vec![Type::Number], vec![Type::String, Type::Number]] {
                    assert!(queries.call(&function, &valid).errors.is_empty());
                }
                for invalid_first in [Type::Number, Type::Shape(vec![])] {
                    let invalid = queries.call(&function, &[invalid_first, Type::Number]);
                    assert!(
                        matches!(
                            invalid.errors.as_slice(),
                            [CallError::Argument { index: 0, .. }]
                        ),
                        "head order {order:?}: {:?}",
                        invalid.errors
                    );
                }
                assert!(matches!(
                    queries
                        .call(&function, &[Type::String, Type::String])
                        .errors
                        .as_slice(),
                    [CallError::Argument { index: 1, .. }]
                ));
                assert!(matches!(
                    queries
                        .call(&function, &[Type::String, Type::Number, Type::Number])
                        .errors
                        .as_slice(),
                    [CallError::Arity { .. }]
                ));
            },
        );
    }
}
