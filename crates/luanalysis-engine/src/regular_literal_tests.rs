use super::*;

#[test]
fn written_number_string_boolean_and_union_literals_keep_regularness_across_scalar_queries() {
    Engine::new().with_session(|q| {
        for (literal, widened) in [
            (Type::NumberLiteral(1.0), Type::Number),
            (Type::StringLiteral(b"written".to_vec()), Type::String),
            (Type::BooleanLiteral(false), Type::Boolean),
        ] {
            assert_eq!(q.widen(&literal), widened, "runtime literal descriptors remain fresh");
            let regular = q.regular_type(&literal);
            assert!(matches!(regular, Type::SessionType(_)));
            assert_eq!(q.inspect_type(&regular), literal);
            let stored = q.widen(&regular);
            assert_eq!(q.format(&stored), q.format(&literal));
            assert!(!q.is_assignable(&widened, &stored));
            let retained = q.retain_pack(&ValuePack::single(stored));
            let projected = q.pack_slot(&retained, 0);
            let copied = q.widen(&projected);
            assert_eq!(q.inspect_type(&copied), literal);
        }
        let union = Type::Union(vec![Type::NumberLiteral(1.0), Type::NumberLiteral(2.0)]);
        let regular = q.regular_type(&union);
        let first = q.widen(&regular);
        let reduced = q.union_reduced(&[first]);
        let copied = q.widen(&reduced);
        assert!(q.is_assignable(&Type::NumberLiteral(2.0), &copied));
        assert!(!q.is_assignable(&Type::NumberLiteral(3.0), &copied));
        assert!(!q.is_assignable(&Type::Number, &copied));
        assert!(matches!(q.inspect_type(&copied), Type::Union(parts) if parts.iter().all(|part| matches!(part, Type::NumberLiteral(_)))));
    });
}

#[test]
fn declared_result_packs_and_call_results_keep_regular_literals_during_copy_inference() {
    Engine::new().with_session(|q| {
        let literal = q.regular_type(&Type::NumberLiteral(1.0));
        let function = q.pack_function(&PackFunctionType { params: vec![], rest: None, type_parameters: vec![], returns: ValuePack::single(literal) });
        let result = q.call_pack(&function, &[], &[], None);
        assert!(result.errors.is_empty());
        let scalar = q.pack_slot(&result.return_pack, 0);
        let copied = q.widen(&scalar);
        assert_eq!(q.format(&copied), "1");
        assert!(!q.is_assignable(&Type::NumberLiteral(2.0), &copied));
        let displayed = q.inspect_type(&copied);
        assert_eq!(displayed, Type::NumberLiteral(1.0));
    });
}

struct Capture;
fn function() -> NativeFunctionKey { NativeFunctionKey { snapshot: 95001, index: 0 } }
fn binding() -> NativeBindingKey { NativeBindingKey { snapshot: 95001, index: 0 } }
impl NativeTypeProvider for Capture {
    fn contains_function(&self, key: NativeFunctionKey) -> bool { key == function() }
    fn contains_binding(&self, key: NativeBindingKey) -> bool { key == binding() }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { vec![] }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { None }
    fn function_inferred_return(&self, _: NativeFunctionKey, q: &mut dyn TypeQueries) -> Type {
        let capture = q.current_captured_function().expect("actual native capture frame");
        q.captured_binding(&capture, binding()).unwrap()
    }
    fn return_cycle(&self, _: NativeFunctionKey) { panic!("acyclic capture") }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { panic!("shallow capture") }
}

#[test]
fn captured_regular_literals_keep_native_capture_identity_and_copy_contracts() {
    Engine::new().with_native_provider(|| Capture, |_, q| {
        let literal = q.regular_type(&Type::StringLiteral(b"captured".to_vec()));
        let captured = q.captured_function(function(), &[(binding(), literal)]);
        let result = q.call(&captured, &[]);
        assert!(result.errors.is_empty());
        let copied = q.widen(&result.return_type);
        assert_eq!(q.inspect_type(&copied), Type::StringLiteral(b"captured".to_vec()));
        assert!(!q.is_assignable(&Type::StringLiteral(b"changed".to_vec()), &copied));
    });
}

#[test]
fn regular_literal_capabilities_obey_epoch_guards_and_display_descriptors_remain_portable() {
    let regular = Engine::new().with_session(|q| q.regular_type(&Type::BooleanLiteral(false)));
    let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Engine::new().widen(&regular))).unwrap_err();
    assert!(error.downcast::<InvalidSessionType>().is_ok());
    let descriptor = Engine::new().with_session(|q| {
        let regular = q.regular_type(&Type::NumberLiteral(1.0));
        q.inspect_type(&regular)
    });
    assert_eq!(descriptor, Type::NumberLiteral(1.0));
    assert_eq!(Engine::new().format(&descriptor), "1");
    assert_eq!(Engine::new().widen(&Type::NumberLiteral(1.0)), Type::Number);
    let result = Engine::new().call(&Type::Function(FunctionType { params: vec![], returns: Box::new(Type::NumberLiteral(1.0)) }), &[]);
    assert!(result.errors.is_empty());
    assert_eq!(result.return_type, Type::NumberLiteral(1.0));
    assert_eq!(Engine::new().format(&result.return_type), "1");
}
