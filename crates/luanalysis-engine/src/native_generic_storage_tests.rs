//! Internal wrapper controls: source annotations do not expose return overrides.
use super::*;
use std::cell::RefCell;
use bun_sema::types::{Shape, SigData};

fn signature() -> NativeSignatureKey { NativeSignatureKey { snapshot: 91_001, index: 0 } }
struct WrappedProvider { base: Provider, value: RefCell<Type> }
impl NativeTypeProvider for WrappedProvider {
    fn contains_function(&self, _: NativeFunctionKey) -> bool { false }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { unreachable!() }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { unreachable!() }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type { unreachable!() }
    fn return_cycle(&self, _: NativeFunctionKey) { unreachable!() }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { unreachable!() }
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool { self.base.contains_type_declaration(key) }
    fn type_declaration_header(&self, key: NativeTypeDeclarationKey) -> TypeDeclarationHeader { self.base.type_declaration_header(key) }
    fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool { self.base.contains_type_member(key) }
    fn type_declaration_members(&self, key: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> { self.base.type_declaration_members(key) }
    fn type_member_header(&self, key: NativeTypeMemberKey) -> TypeMemberHeader { self.base.type_member_header(key) }
    fn type_member_type(&self, _: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type { self.value.borrow().clone() }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool { self.base.contains_type_parameter(key) }
    fn type_parameter_name(&self, key: NativeTypeParameterKey) -> String { self.base.type_parameter_name(key) }
    fn type_parameter_owner(&self, key: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> { self.base.type_parameter_owner(key) }
    fn type_declaration_error(&self, error: NamedTypeError) { self.base.type_declaration_error(error) }
    fn signature_return_cycle(&self, _: NativeSignatureKey) { panic!("wrapper walk must not create a return cycle") }
    fn signature_return_type_too_deep(&self, _: NativeSignatureKey) { panic!("wrapper walk must not exceed return depth") }
    fn contains_signature(&self, key: NativeSignatureKey) -> bool { key == signature() }
    fn signature_header(&self, _: NativeSignatureKey) -> SignatureHeader {
        SignatureHeader { owner: SignatureOwner::NamedType(key(0)), site: SignatureSite { snapshot: 91_001, index: 0 }, parent: None, has_direct_literal_parameter: false }
    }
    fn signature_parameters(&self, _: NativeSignatureKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { vec![] }
    fn signature_outer_type_parameters(&self, _: NativeSignatureKey) -> Vec<NativeTypeParameterKey> { vec![parameter()] }
    fn signature_declared_return_pack(&self, _: NativeSignatureKey, _: &mut dyn TypeQueries) -> ValuePack { ValuePack::single(Type::NativeTypeParameter(parameter())) }
}
#[derive(Clone, Copy)]
enum Override { Erased, Observed, Recursive }
fn check_wrapper(override_kind: Override, physical_pack: bool) -> bool {
    with_program(|program, handles| {
        let mut declaration = Decl::class(0, vec![Type::Any]);
        declaration.generic = true;
        let provider = WrappedProvider { base: Provider { declarations: vec![declaration], ..Default::default() }, value: RefCell::new(Type::Any) };
        let adapter = ProviderAdapter { provider: &provider, handles };
        let mut checker = program.checker_with_native_provider(&adapter);
        checker.set_native_type_declaration_provider(&adapter);
        checker.enable_native_arrays();
        let formal = checker.native_type_parameter(core_parameter_key(parameter())).unwrap();
        let base = checker.native_signature_type(native_signatures::core_signature_key(signature())).unwrap();
        let base = checker.signatures(base, false)[0];
        let returned = match override_kind {
            Override::Erased => TypeId::NUMBER,
            Override::Observed => formal,
            Override::Recursive => {
                // A nested instance of the same wrapped origin/profile crosses
                // the deliberate conservative proof boundary, even though this
                // particular finite nesting ultimately erases T.
                let inner = if physical_pack {
                    let pack = checker.native_scalar_pack(TypeId::NUMBER);
                    checker.types().intern_sig(SigData::WithNativeResultPack { sig: base, pack })
                } else {
                    checker.types().intern_sig(SigData::WithReturn { sig: base, ret: TypeId::NUMBER })
                };
                let mut shape = Shape::new_in(checker.arena);
                shape.call.push(inner);
                checker.synth(shape)
            }
        };
        let wrapper = if physical_pack {
            let pack = checker.native_scalar_pack(returned);
            checker.types().intern_sig(SigData::WithNativeResultPack { sig: base, pack })
        } else {
            checker.types().intern_sig(SigData::WithReturn { sig: base, ret: returned })
        };
        let mut shape = Shape::new_in(checker.arena);
        shape.call.push(wrapper);
        let ty = checker.synth(shape);
        *provider.value.borrow_mut() = export_type(&mut checker, handles, ty);
        let mut queries = SessionQueries { checker: &mut checker, handles };
        let narrow = queries.named_type_reference(key(0), &[Type::NumberLiteral(1.0)]).unwrap();
        let wide = queries.named_type_reference(key(0), &[Type::Number]).unwrap();
        let related = queries.is_assignable(&narrow, &wide);
        assert!(provider.base.errors.borrow().is_empty(), "{:?}", provider.base.errors.borrow());
        related
    })
}
#[test]
fn finite_return_overrides_preserve_erasure_and_actual_t_observability() {
    for physical_pack in [false, true] {
        assert!(check_wrapper(Override::Erased, physical_pack));
        assert!(!check_wrapper(Override::Observed, physical_pack));
    }
}
#[test]
fn repeated_wrapped_native_origins_fail_closed_with_bounded_analysis() {
    // Conservative proof boundary: even nested finite instances of the same
    // wrapped native origin are not assumed to preserve erasure. Ordinary
    // finite wrappers above retain their actual overridden contracts.
    for physical_pack in [false, true] { assert!(!check_wrapper(Override::Recursive, physical_pack)); }
}
