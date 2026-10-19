use luanalysis::{check, CheckReport};

fn clean(source: &str) -> CheckReport {
    let report = check(source);
    assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    report
}
fn rejects(source: &str, expression: &str) {
    let report = check(source);
    assert!(report.diagnostics.iter().any(|error| error.code == "TS2322"
        && &source[error.span.start..error.span.end] == expression),
        "{source}\n{:#?}", report.diagnostics);
    assert!(report.diagnostics.iter().all(|error| error.code != "TS2589" && error.code != "LUA2031"), "{:#?}", report.diagnostics);
}

#[test]
fn a_mutable_generic_alias_cannot_change_a_literal_through_a_wider_view() {
    let source = r#"---@class Box<T:string>
---@field value T

---@param narrow Box<"literal">
local function mutate(narrow)
 ---@type Box<string>
 local wide=narrow
 wide.value="different"
 ---@type "literal"
 local assumed=narrow.value
 return assumed
end
"#;
    rejects(source, "narrow");
    clean(&source.replace("Box<string>", "Box<\"literal\">").replace("wide.value=\"different\"", "wide.value=\"literal\""));
}

#[test]
fn mapped_inherited_arguments_are_invariant_only_when_the_ancestor_stores_t() {
    let prefix = "---@class Base<T>\n---@field value T\n\n---@class Child<U>:Base<U>\n\n";
    let wrong = format!("{prefix}---@param value Child<1>\n---@return Base<number>\nlocal function widen(value) return value end\n");
    rejects(&wrong, "value");
    clean(&wrong.replace("Base<number>", "Base<1>"));
    clean("---@class Base<T>\n\n---@class Child<U>:Base<U>\n\n---@param value Child<1>\n---@return Base<number>\nlocal function widen(value) return value end\n");
}

#[test]
fn bounds_do_not_erase_the_rigid_argument_of_writable_generic_self() {
    let source = "---@class Box<T:string>\n---@field value T\nlocal Box={}\n---@return self\nfunction Box:identity()\n ---@type Box<string>\n local rejected=self\n return self\nend\n";
    rejects(source, "self");
    clean(&source.replace("---@type Box<string>\n local rejected=self", "---@type self\n local accepted=self"));
}

#[test]
fn nested_arrays_records_and_nominal_storage_cannot_hide_the_writable_parameter() {
    for field in ["T[]", "{nested:T}", "Inner<T>"] {
        let source = format!("---@class Inner<U>\n---@field value U\n\n---@class Outer<T>\n---@field value {field}\n\n---@param value Outer<1>\n---@return Outer<number>\nlocal function widen(value) return value end\n");
        rejects(&source, "value");
        clean(&source.replace("Outer<number>", "Outer<1>"));
    }
    clean("---@class Phantom<U>\n\n---@class Outer<T>\n---@field value Phantom<T>\n\n---@param value Outer<1>\n---@return Outer<number>\nlocal function widen(value) return value end\n");
}

#[test]
fn writable_callback_and_method_slots_are_invariant_but_function_values_keep_variance() {
    let callback = "---@class Box<T>\n---@field callback fun():T\n\n---@param value Box<1>\n---@return Box<number>\nlocal function widen(value) return value end\n";
    rejects(callback, "value");
    clean(&callback.replace("Box<number>", "Box<1>"));
    let method = "---@class Box<T>\nlocal Box={}\n---@return T\nfunction Box.get() return --[[---@type any]] nil end\n\n---@param value Box<1>\n---@return Box<number>\nlocal function widen(value) return value end\n";
    rejects(method, "value");
    clean(&method.replace("Box<number>", "Box<1>"));
    clean("---@param value fun():1\n---@return fun():number\nlocal function widen(value) return value end\n");
    clean("---@param value fun(value:number):void\n---@return fun(value:1):void\nlocal function narrow(value) return value end\n");
    clean("---@class Factory<T>\n---@overload fun():T\n\n---@param value Factory<1>\n---@return Factory<number>\nlocal function widen(value) return value end\n");
}

#[test]
fn recursive_growing_phantoms_remain_unobserved_and_swapped_parameters_do_not() {
    clean("---@class Phantom<T>\n---@field next Phantom<T[]>\n\n---@param value Phantom<number>\n---@return Phantom<string>\nlocal function change(value) return value end\n");
    let swap = "---@class Flip<A,B>\n---@field value A\n---@field next Flip<B,A>\n\n---@param value Flip<number,1>\n---@return Flip<number,number>\nlocal function widen(value) return value end\n";
    rejects(swap, "value");
    clean(&swap.replace("Flip<number,number>", "Flip<number,1>"));
    clean("---@alias Grow<T> fun():Grow<T[]>\n---@class Phantom<T>\n---@field get Grow<T>\n\n---@param value Phantom<number>\n---@return Phantom<string>\nlocal function change(value) return value end\n");
}

#[test]
fn recursive_captured_native_closures_keep_their_original_t_and_bounded_storage_walk() {
    let source = "---@generic T\n---@param value T\nlocal function make(value)\n return function() return make(value) end\nend\n\n---@class Holder<T>\nlocal Holder={\n ---@param value T\n get=function(value) return make(value) end,\n}\n\n---@param value Holder<1>\n---@return Holder<number>\nlocal function widen(value) return value end\n";
    rejects(source, "value");
    clean(&source.replace("Holder<number>", "Holder<1>"));
    // The recursive declaration has its own generic quantifier: its next call
    // can instantiate T with T[]. The captured body's original T remains rigid.
    let growing = source.replace("return function() return make(value) end", "return function() return make({value}) end")
        .replace("Holder<number>", "Holder<1>");
    clean(&growing);
    let rigid = source.replace(
        "return function() return make(value) end",
        "return function()\n---@type T\nlocal bad={value}\nreturn make(value)\nend",
    ).replace("Holder<number>", "Holder<1>");
    let report = check(&rigid);
    assert_eq!(report.diagnostics.len(), 1, "{:#?}", report.diagnostics);
    let error = &report.diagnostics[0];
    assert_eq!(error.code, "TS2322");
    assert_eq!(&rigid[error.span.start..error.span.end], "{value}");
}

mod readonly {
    use luanalysis_engine::*;
    use std::cell::Cell;

    const SNAPSHOT: u64 = 95_811;
    fn declaration() -> NativeTypeDeclarationKey { NativeTypeDeclarationKey { snapshot: SNAPSHOT, index: 0 } }
    fn parameter() -> NativeTypeParameterKey { NativeTypeParameterKey { snapshot: SNAPSHOT, index: 0 } }
    fn member() -> NativeTypeMemberKey { NativeTypeMemberKey { snapshot: SNAPSHOT, index: 0 } }
    #[derive(Clone, Copy)]
    enum Value {
        Scalar, NestedMutable, NestedReadonly, Producer, Consumer,
        ProducerMutable, ProducerArray, ProducerReadonly,
        ConsumerMutable, ConsumerArray, ConsumerReadonly,
    }
    fn function() -> NativeFunctionKey { NativeFunctionKey { snapshot: SNAPSHOT, index: 0 } }
    struct Provider { readonly: bool, value: Value, pending: Cell<bool>, captured: bool }
    impl NativeTypeProvider for Provider {
        fn contains_function(&self, key: NativeFunctionKey) -> bool { self.captured && key == function() }
        fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { vec![Parameter { name: "value".into(), ty: Type::NativeTypeParameter(parameter()), optional: false }] }
        fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { Some(Type::NativeNamedReference { declaration: declaration(), arguments: vec![] }) }
        fn function_type_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<NativeTypeParameterKey> { vec![parameter()] }
        fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type { unreachable!() }
        fn return_cycle(&self, _: NativeFunctionKey) { unreachable!() }
        fn return_type_too_deep(&self, _: NativeFunctionKey) { unreachable!() }
        fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool { key == declaration() }
        fn type_declaration_header(&self, _: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
            TypeDeclarationHeader { kind: TypeDeclarationKind::Class, name: "Box".into(),
                object_form: Some(ObjectDeclarationForm::Record), parameters: if self.captured { vec![] } else { vec![parameter()] }, outer_parameters: if self.captured { vec![parameter()] } else { vec![] } }
        }
        fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool { key == member() }
        fn type_member_activation(&self, _: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> TypeMemberActivation { if self.pending.get() { TypeMemberActivation::Pending } else { TypeMemberActivation::Active } }
        fn type_declaration_members(&self, _: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> { vec![member()] }
        fn type_member_header(&self, _: NativeTypeMemberKey) -> TypeMemberHeader {
            TypeMemberHeader { owner: TypeMemberOwner::Declared(declaration()), key: TypeMemberKeyRecipe::Fixed(LuaKey::String(b"value".to_vec())),
                visibility: TypeMemberVisibility::Public, optional: false, readonly: self.readonly }
        }
        fn type_member_type(&self, _: NativeTypeMemberKey, queries: &mut dyn TypeQueries) -> Type {
            let formal = Type::NativeTypeParameter(parameter());
            match self.value {
                Value::Scalar => formal,
                Value::NestedMutable | Value::NestedReadonly => {
                    let mut field = TableField::declared(LuaKey::String(b"nested".to_vec()), formal);
                    field.readonly = matches!(self.value, Value::NestedReadonly);
                    Type::Table(TableType { fields: vec![field], ..Default::default() })
                }
                Value::Producer => Type::Function(FunctionType { params: vec![], returns: Box::new(formal) }),
                Value::Consumer => Type::Function(FunctionType { params: vec![Parameter { name: "value".into(), ty: formal, optional: false }], returns: Box::new(Type::Void) }),
                Value::ProducerMutable | Value::ProducerArray | Value::ProducerReadonly
                | Value::ConsumerMutable | Value::ConsumerArray | Value::ConsumerReadonly => {
                    let storage = if matches!(self.value, Value::ProducerArray | Value::ConsumerArray) {
                        queries.array_type(&formal)
                    } else {
                        let mut field = TableField::declared(LuaKey::String(b"nested".to_vec()), formal);
                        field.readonly = matches!(self.value, Value::ProducerReadonly | Value::ConsumerReadonly);
                        Type::Table(TableType { fields: vec![field], ..Default::default() })
                    };
                    if matches!(self.value, Value::ConsumerMutable | Value::ConsumerArray | Value::ConsumerReadonly) {
                        Type::Function(FunctionType { params: vec![Parameter { name: "value".into(), ty: storage, optional: false }], returns: Box::new(Type::Void) })
                    } else {
                        Type::Function(FunctionType { params: vec![], returns: Box::new(storage) })
                    }
                }
            }
        }
        fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool { key == parameter() }
        fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String { "T".into() }
        fn type_parameter_owner(&self, _: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> { Some(if self.captured { TypeDeclarationOwner::Function(function()) } else { TypeDeclarationOwner::NamedType(declaration()) }) }
        fn has_type_parameter_constraint(&self, _: NativeTypeParameterKey) -> bool { self.captured }
        fn type_parameter_constraint(&self, _: NativeTypeParameterKey, _: &mut dyn TypeQueries) -> Option<Type> { self.captured.then_some(Type::Number) }
    }
    fn relation(value: Value, readonly: bool) -> (bool, bool) {
        Engine::new().with_native_provider(|| Provider { readonly, value, pending: Cell::new(false), captured: false }, |_, queries| {
            let narrow = queries.named_type_reference(declaration(), &[Type::NumberLiteral(1.0)]).unwrap();
            let wide = queries.named_type_reference(declaration(), &[Type::Number]).unwrap();
            (queries.is_assignable(&narrow, &wide), queries.is_assignable(&wide, &narrow))
        })
    }
    #[test]
    fn readonly_scalar_and_producer_covariance_remains() {
        assert_eq!(relation(Value::Scalar, true), (true, false));
        assert_eq!(relation(Value::Producer, true), (true, false));
        assert_eq!(relation(Value::Consumer, true), (false, true));
    }
    #[test]
    fn readonly_outer_members_still_expose_inner_mutation() {
        assert_eq!(relation(Value::NestedMutable, true), (false, false));
        assert_eq!(relation(Value::NestedReadonly, true), (true, false));
    }
    #[test]
    fn readonly_producers_cannot_expose_mutable_storage_through_a_wider_return_view() {
        for value in [Value::ProducerMutable, Value::ProducerArray] {
            // A producer may return the same captured table on every call. A
            // widened caller must not write number into storage promised as 1.
            assert_eq!(relation(value, true), (false, false));
        }
    }
    #[test]
    fn readonly_consumers_cannot_accept_narrow_storage_through_a_wider_writer() {
        for value in [Value::ConsumerMutable, Value::ConsumerArray] {
            // A consumer of mutable number storage can write 2; it cannot be
            // exposed as a consumer accepting storage whose value is exactly 1.
            assert_eq!(relation(value, true), (false, false));
        }
    }
    #[test]
    fn readonly_nested_callable_storage_keeps_ordinary_function_variance() {
        assert_eq!(relation(Value::ProducerReadonly, true), (true, false));
        assert_eq!(relation(Value::ConsumerReadonly, true), (false, true));
    }
    #[test]
    fn writable_callbacks_are_invariant_in_both_signature_directions() {
        assert_eq!(relation(Value::Producer, false), (false, false));
        assert_eq!(relation(Value::Consumer, false), (false, false));
    }
    #[test]
    fn pending_observability_is_not_a_successful_public_assignment_or_a_cached_mask() {
        Engine::new().with_native_provider(
            || Provider { readonly: true, value: Value::Scalar, pending: Cell::new(true), captured: false },
            |provider, queries| {
                let narrow = queries.named_type_reference(declaration(), &[Type::NumberLiteral(1.0)]).unwrap();
                let wide = queries.named_type_reference(declaration(), &[Type::Number]).unwrap();
                assert!(!queries.is_assignable(&narrow, &wide));
                provider.pending.set(false);
                assert!(queries.is_assignable(&narrow, &wide));
            },
        );
    }
    #[test]
    fn captured_outer_arguments_keep_their_original_function_parameter_identity() {
        Engine::new().with_native_provider(
            || Provider { readonly: false, value: Value::Scalar, pending: Cell::new(false), captured: true },
            |_, queries| {
                let narrow = queries.call(&Type::NativeFunction(function()), &[Type::NumberLiteral(1.0)]);
                let wide = queries.call(&Type::NativeFunction(function()), &[Type::Number]);
                assert!(narrow.errors.is_empty() && wide.errors.is_empty(), "{narrow:?} {wide:?}");
                let literal = queries.property_type(&narrow.return_type, "value").unwrap();
                assert_eq!(queries.format(&literal), "1");
                assert_eq!(queries.property_type(&wide.return_type, "value"), Some(Type::Number));
                assert!(!queries.is_assignable(&narrow.return_type, &wide.return_type));
                assert!(!queries.is_assignable(&wide.return_type, &narrow.return_type));
            },
        );
    }

}
