use crate::*;
use std::cell::Cell;

fn owner() -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey { snapshot: 8017, index: 0 }
}
fn member(index: u32) -> NativeTypeMemberKey {
    NativeTypeMemberKey { snapshot: 8017, index }
}
fn signature(index: u32) -> NativeSignatureKey {
    NativeSignatureKey { snapshot: 8017, index }
}

struct WrittenContracts {
    field_demands: Cell<usize>,
    signature_demands: Cell<usize>,
}
impl NativeTypeProvider for WrittenContracts {
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool { key == owner() }
    fn type_declaration_header(&self, _: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        TypeDeclarationHeader {
            kind: TypeDeclarationKind::Shape,
            name: "Written".into(),
            object_form: Some(ObjectDeclarationForm::Record),
            parameters: vec![],
            outer_parameters: vec![],
        }
    }
    fn type_declaration_members(&self, _: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        (0..4).map(member).collect()
    }
    fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool {
        key.snapshot == 8017 && key.index < 4
    }
    fn type_member_header(&self, key: NativeTypeMemberKey) -> TypeMemberHeader {
        TypeMemberHeader {
            owner: TypeMemberOwner::Declared(owner()),
            key: TypeMemberKeyRecipe::Fixed(LuaKey::String(
                [b"count".as_slice(), b"callback", b"method", b"unrelated"][key.index as usize].to_vec(),
            )),
            visibility: TypeMemberVisibility::Public,
            optional: false,
            readonly: false,
        }
    }
    fn type_member_origin(&self, key: NativeTypeMemberKey) -> TableFieldOrigin {
        if key.index == 3 { TableFieldOrigin::Inferred } else { TableFieldOrigin::Declared }
    }
    fn type_member_signatures(&self, key: NativeTypeMemberKey) -> Option<Vec<NativeSignatureKey>> {
        (key.index == 2).then(|| vec![signature(1)])
    }
    fn type_member_type(&self, key: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        self.field_demands.set(self.field_demands.get() + 1);
        match key.index {
            0 => Type::Number,
            1 => Type::NativeSignature(signature(0)),
            2 => panic!("bodyless @function must use its original signature"),
            _ => panic!("reading written contracts must not infer an unrelated member"),
        }
    }
    fn contains_signature(&self, key: NativeSignatureKey) -> bool {
        key.snapshot == 8017 && key.index < 2
    }
    fn signature_header(&self, key: NativeSignatureKey) -> SignatureHeader {
        SignatureHeader {
            owner: SignatureOwner::NamedType(owner()),
            site: SignatureSite { snapshot: key.snapshot, index: key.index },
            parent: None,
            has_direct_literal_parameter: false,
        }
    }
    fn signature_parameters(&self, _: NativeSignatureKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        vec![Parameter { name: "value".into(), ty: Type::Number, optional: false }]
    }
    fn signature_declared_return_pack(&self, _: NativeSignatureKey, _: &mut dyn TypeQueries) -> ValuePack {
        self.signature_demands.set(self.signature_demands.get() + 1);
        ValuePack::single(Type::String)
    }
    fn runtime_table_seed(&self, _: NativeTypeDeclarationKey, _: &mut dyn TypeQueries) -> Option<Type> {
        panic!("written declarations do not have an evolving allocation seed")
    }
    fn runtime_store_receiver(&self, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Option<Type> {
        panic!("written declarations must not trace evolving store origins")
    }
    fn contextual_type_owner(&self, _: NativeFunctionKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> ContextualTypeOwner {
        panic!("written members do not need a contextual runtime owner")
    }
    fn contains_function(&self, _: NativeFunctionKey) -> bool {
        panic!("written signatures do not manufacture runtime functions")
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        panic!("written signatures have their own parameters")
    }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> {
        panic!("written signatures have their own result pack")
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("written signatures must not infer a runtime body")
    }
    fn return_cycle(&self, _: NativeFunctionKey) { panic!("no runtime return query") }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { panic!("no runtime return query") }
}

#[test]
fn written_fields_and_bodyless_functions_need_no_evolving_member_or_runtime_origin_demand() {
    Engine::new().with_native_provider(
        || WrittenContracts { field_demands: Cell::new(0), signature_demands: Cell::new(0) },
        |provider, queries| {
            let written = queries.named_declared_type(owner()).unwrap();
            assert_eq!(queries.property_type(&written, "count"), Some(Type::Number));
            for name in ["callback", "method"] {
                let callable = queries.property_type(&written, name).unwrap();
                assert_eq!(queries.function_implementation_type(&callable), None);
                let result = queries.call_pack(&callable, &[Type::Number], &[], None);
                assert!(result.errors.is_empty(), "{:#?}", result.errors);
                assert_eq!(queries.pack_slot(&result.return_pack, 0), Type::String);
                let wrong = queries.call_pack(&callable, &[Type::Boolean], &[], None);
                assert!(!wrong.errors.is_empty(), "written parameters still reject wrong arguments");
            }
            assert_eq!(provider.field_demands.get(), 2, "only the two written @field contracts were read");
            assert_eq!(provider.signature_demands.get(), 2, "both original written signatures were resolved");
        },
    );
}
