//! Written callable declarations retain original identity and signature-local generics.

use super::native_packs::import_pack;
use super::{
    Checker, NativeFunctionKey, NativeTypeDeclarationKey, NativeTypeProvider, ProviderAdapter,
    SessionQueries, Type, core_key, import_type,
};
use bun_sema::check::native_signatures::{
    NativeSignatureHeader, NativeSignatureOwner, NativeSignatureSite,
};
use bun_sema::types::{NativeSignatureId, SigParam};

/// Runtime implementation arity policy. Written public signature arity and
/// body parameter types are independent of this metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FunctionCallingConvention { TypeScript, Lua }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeSignatureKey {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeIntrinsicKind { Assert, Type, Require, SetMetatable }

/// Trusted operation identity attached to an original written signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeIntrinsicSite {
    pub binding: super::NativeGlobalBindingKey,
    pub declaration: super::NativeValueDeclarationKey,
    pub kind: NativeIntrinsicKind,
}

pub(super) fn source_intrinsic_site(site: bun_sema::check::native_intrinsics::NativeIntrinsicSite) -> NativeIntrinsicSite {
    NativeIntrinsicSite {
        binding: super::NativeGlobalBindingKey {
            environment: super::NativeEnvironmentKey { snapshot: site.binding.environment.snapshot, index: site.binding.environment.index },
            index: site.binding.index,
        },
        declaration: super::NativeValueDeclarationKey { snapshot: site.declaration.snapshot, index: site.declaration.index },
        kind: match site.kind {
            bun_sema::check::native_intrinsics::NativeIntrinsicKind::Assert => NativeIntrinsicKind::Assert,
            bun_sema::check::native_intrinsics::NativeIntrinsicKind::Type => NativeIntrinsicKind::Type,
            bun_sema::check::native_intrinsics::NativeIntrinsicKind::Require => NativeIntrinsicKind::Require,
            bun_sema::check::native_intrinsics::NativeIntrinsicKind::SetMetatable => NativeIntrinsicKind::SetMetatable,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SignatureOwner {
    Standalone,
    Function(NativeFunctionKey),
    NamedType(NativeTypeDeclarationKey),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SignatureSite {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SignatureHeader {
    pub owner: SignatureOwner,
    pub site: SignatureSite,
    pub parent: Option<NativeSignatureKey>,
    pub has_direct_literal_parameter: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CallError, Engine, InvalidSessionType, NativeBindingKey, NativeTypeParameterKey,
        PackAlternative, PackSlot, Parameter, TypeQueries, ValuePack,
    };
    use std::cell::Cell;

    #[derive(Clone)]
    struct Head {
        parameter: Type,
        returns: ValuePack,
        site: u32,
        literal: bool,
        generic: Option<u32>,
        constraint: Option<Type>,
        predicate: Option<Type>,
    }
    #[derive(Clone)]
    struct Provider {
        heads: Vec<Head>,
        parameter_reads: Cell<usize>,
        return_reads: Cell<usize>,
        predicate_reads: Cell<usize>,
        function_predicate_reads: Cell<usize>,
        implementation_reads: Cell<usize>,
        factory: bool,
        named: bool,
        contextual: bool,
        standalone: bool,
        contextual_checks: Cell<usize>,
        implementation_contract: Option<ValuePack>,
        implementation_pack: Option<ValuePack>,
        body_pack_reads: Cell<usize>,
        calling_convention: FunctionCallingConvention,
        parameterless_heads: Vec<u32>,
        implementation_parameter: Type,
    }
    impl Provider {
        fn key(index: u32) -> NativeSignatureKey {
            NativeSignatureKey {
                snapshot: 1219,
                index,
            }
        }
        fn function(index: u32) -> NativeFunctionKey {
            NativeFunctionKey {
                snapshot: 1219,
                index,
            }
        }
        fn parameter(index: u32) -> NativeTypeParameterKey {
            NativeTypeParameterKey {
                snapshot: 1219,
                index,
            }
        }
        fn binding() -> NativeBindingKey {
            NativeBindingKey {
                snapshot: 1219,
                index: 0,
            }
        }
        fn new(heads: Vec<Head>) -> Self {
            Self {
                heads,
                parameter_reads: Cell::new(0),
                return_reads: Cell::new(0),
                predicate_reads: Cell::new(0),
                function_predicate_reads: Cell::new(0),
                implementation_reads: Cell::new(0),
                factory: false,
                named: false,
                contextual: false,
                standalone: false,
                contextual_checks: Cell::new(0),
                implementation_contract: None,
                implementation_pack: None,
                body_pack_reads: Cell::new(0),
                calling_convention: FunctionCallingConvention::TypeScript,
                parameterless_heads: Vec::new(),
                implementation_parameter: Type::Any,
            }
        }
    }
    impl NativeTypeProvider for Provider {
        fn function_calling_convention(&self, _: NativeFunctionKey) -> FunctionCallingConvention {
            self.calling_convention
        }
        fn contains_signature(&self, key: NativeSignatureKey) -> bool {
            key.snapshot == 1219 && (key.index as usize) < self.heads.len()
        }
        fn signature_may_have_type_predicate(&self, key: NativeSignatureKey) -> bool {
            self.heads[key.index as usize].predicate.is_some()
        }
        fn function_may_have_type_predicate(&self, _: NativeFunctionKey) -> bool { false }
        fn function_type_predicate(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<crate::TypePredicate> {
            self.function_predicate_reads.set(self.function_predicate_reads.get() + 1);
            None
        }
        fn signature_header(&self, key: NativeSignatureKey) -> SignatureHeader {
            let head = &self.heads[key.index as usize];
            SignatureHeader {
                owner: if self.standalone {
                    SignatureOwner::Standalone
                } else if self.named {
                    SignatureOwner::NamedType(NativeTypeDeclarationKey {
                        snapshot: 1219,
                        index: 0,
                    })
                } else {
                    SignatureOwner::Function(Self::function(0))
                },
                site: SignatureSite {
                    snapshot: 1219,
                    index: head.site,
                },
                parent: None,
                has_direct_literal_parameter: head.literal,
            }
        }
        fn function_overload_signatures(&self, key: NativeFunctionKey) -> Vec<NativeSignatureKey> {
            if key.index == 0 {
                (0..self.heads.len())
                    .map(|index| Self::key(index as u32))
                    .collect()
            } else {
                Vec::new()
            }
        }
        fn signature_parameters(
            &self,
            key: NativeSignatureKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
            self.parameter_reads.set(self.parameter_reads.get() + 1);
            if self.parameterless_heads.contains(&key.index) { return Vec::new(); }
            vec![Parameter {
                name: "value".into(),
                ty: self.heads[key.index as usize].parameter.clone(),
                optional: false,
            }]
        }
        fn signature_type_predicate(&self, key: NativeSignatureKey, _: &mut dyn TypeQueries) -> Option<crate::TypePredicate> {
            self.predicate_reads.set(self.predicate_reads.get() + 1);
            self.heads[key.index as usize].predicate.clone().map(|ty| crate::TypePredicate { parameter: 0, ty: Some(ty), asserts: false })
        }
        fn signature_declared_return_pack(
            &self,
            key: NativeSignatureKey,
            _: &mut dyn TypeQueries,
        ) -> ValuePack {
            self.return_reads.set(self.return_reads.get() + 1);
            self.heads[key.index as usize].returns.clone()
        }
        fn signature_type_parameters(
            &self,
            key: NativeSignatureKey,
        ) -> Vec<NativeTypeParameterKey> {
            self.heads[key.index as usize]
                .generic
                .map(Self::parameter)
                .into_iter()
                .collect()
        }
        fn signature_outer_type_parameters(
            &self,
            _: NativeSignatureKey,
        ) -> Vec<NativeTypeParameterKey> {
            if self.factory || self.named {
                vec![Self::parameter(0)]
            } else {
                Vec::new()
            }
        }
        fn contains_function(&self, key: NativeFunctionKey) -> bool {
            key.snapshot == 1219
                && (key.index == 0
                    || self.factory && key.index == 1
                    || self.contextual && key.index == 2)
        }
        fn function_parameters(
            &self,
            key: NativeFunctionKey,
            queries: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
            vec![Parameter {
                name: "value".into(),
                ty: if key.index == 1 {
                    Type::NativeTypeParameter(Self::parameter(0))
                } else if key.index == 2 {
                    queries
                        .current_contextual_function()
                        .and_then(|context| queries.contextual_parameter_type(&context, 0))
                        .unwrap_or(Type::Any)
                } else {
                    self.implementation_parameter.clone()
                },
                optional: false,
            }]
        }
        fn function_declared_return(
            &self,
            key: NativeFunctionKey,
            queries: &mut dyn TypeQueries,
        ) -> Option<Type> {
            if key.index == 1 {
                Some(queries.captured_function(
                    Self::function(0),
                    &[(
                        Self::binding(),
                        Type::NativeTypeParameter(Self::parameter(0)),
                    )],
                ))
            } else {
                None
            }
        }
        fn function_inferred_return(
            &self,
            key: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Type {
            if key.index == 2 {
                return Type::String;
            }
            self.implementation_reads
                .set(self.implementation_reads.get() + 1);
            Type::Any
        }
        fn function_declared_scalar_contract(
            &self,
            key: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Option<ValuePack> {
            (key.index == 0)
                .then(|| self.implementation_contract.clone())
                .flatten()
        }
        fn function_has_result_pack(&self, key: NativeFunctionKey) -> bool {
            key.index == 0 && self.implementation_pack.is_some()
        }
        fn function_inferred_return_pack(
            &self,
            key: NativeFunctionKey,
            queries: &mut dyn TypeQueries,
        ) -> ValuePack {
            if key.index == 0
                && let Some(pack) = &self.implementation_pack
            {
                self.body_pack_reads.set(self.body_pack_reads.get() + 1);
                return pack.clone();
            }
            ValuePack::single(self.function_inferred_return(key, queries))
        }
        fn function_type_parameters(
            &self,
            key: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<NativeTypeParameterKey> {
            if key.index == 1 {
                vec![Self::parameter(0)]
            } else {
                Vec::new()
            }
        }
        fn function_outer_type_parameters(
            &self,
            key: NativeFunctionKey,
        ) -> Vec<NativeTypeParameterKey> {
            if self.factory && key.index == 0 {
                vec![Self::parameter(0)]
            } else {
                Vec::new()
            }
        }
        fn contains_binding(&self, key: NativeBindingKey) -> bool {
            key == Self::binding()
        }
        fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
            key.snapshot == 1219
                && ((self.factory || self.named) && key.index == 0
                    || self
                        .heads
                        .iter()
                        .any(|head| head.generic == Some(key.index)))
        }
        fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String {
            "T".into()
        }
        fn has_type_parameter_constraint(&self, key: NativeTypeParameterKey) -> bool {
            self.heads
                .iter()
                .any(|head| head.generic == Some(key.index) && head.constraint.is_some())
        }
        fn type_parameter_constraint(
            &self,
            key: NativeTypeParameterKey,
            _: &mut dyn TypeQueries,
        ) -> Option<Type> {
            self.heads
                .iter()
                .find(|head| head.generic == Some(key.index))
                .and_then(|head| head.constraint.clone())
        }
        fn type_parameter_owner(
            &self,
            key: NativeTypeParameterKey,
        ) -> Option<crate::TypeDeclarationOwner> {
            if self.named && key.index == 0 {
                Some(crate::TypeDeclarationOwner::NamedType(
                    NativeTypeDeclarationKey {
                        snapshot: 1219,
                        index: 0,
                    },
                ))
            } else if self.factory && key.index == 0 {
                Some(crate::TypeDeclarationOwner::Function(Self::function(1)))
            } else {
                self.heads
                    .iter()
                    .position(|head| head.generic == Some(key.index))
                    .map(|index| crate::TypeDeclarationOwner::Signature(Self::key(index as u32)))
            }
        }
        fn return_cycle(&self, _: NativeFunctionKey) {
            panic!("unexpected implementation cycle")
        }
        fn return_type_too_deep(&self, _: NativeFunctionKey) {
            panic!("unexpected implementation depth refusal")
        }
        fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool {
            self.named && key.snapshot == 1219 && key.index == 0
        }
        fn type_declaration_header(
            &self,
            _: NativeTypeDeclarationKey,
        ) -> crate::TypeDeclarationHeader {
            crate::TypeDeclarationHeader {
                kind: crate::TypeDeclarationKind::Shape,
                name: "Callable".into(),
                object_form: Some(crate::ObjectDeclarationForm::Record),
                parameters: vec![Self::parameter(0)],
                outer_parameters: Vec::new(),
            }
        }
        fn type_declaration_call_signatures(
            &self,
            _: NativeTypeDeclarationKey,
        ) -> Vec<NativeSignatureKey> {
            (0..self.heads.len())
                .map(|index| Self::key(index as u32))
                .collect()
        }
        fn contains_expression(&self, key: crate::NativeExpressionKey) -> bool {
            self.contextual && key.snapshot == 1219 && key.node == 0
        }
        fn is_context_sensitive(&self, key: crate::NativeExpressionKey) -> bool {
            self.contains_expression(key)
        }
        fn has_context_sensitive_parameters(&self, key: crate::NativeExpressionKey) -> bool {
            self.contains_expression(key)
        }
        fn contextual_parameter_indices(&self, key: NativeFunctionKey) -> Vec<usize> {
            if key.index == 2 { vec![0] } else { Vec::new() }
        }
        fn check_expression(
            &self,
            key: crate::NativeExpressionKey,
            expected: Option<&Type>,
            mode: crate::NativeCheckMode,
            queries: &mut dyn TypeQueries,
        ) -> Type {
            self.contextual_checks.set(self.contextual_checks.get() + 1);
            let function = Type::NativeFunction(Self::function(2));
            expected
                .map(|expected| queries.contextual_function_value(&function, key, expected, mode))
                .unwrap_or(function)
        }
    }
    fn head(parameter: Type, returns: Type) -> Head {
        Head {
            parameter,
            returns: ValuePack::single(returns),
            site: 0,
            literal: false,
            generic: None,
            constraint: None,
            predicate: None,
        }
    }

    fn with_provider<R: Send>(
        provider: Provider,
        work: impl FnOnce(&Provider, &mut dyn TypeQueries) -> R + Send,
    ) -> R {
        Engine::new().with_native_provider(move || provider, work)
    }

    #[test]
    fn native_type_queries_scope_scalar_and_pack_callbacks_without_poisoning_normal_checks() {
        #[derive(Default)]
        struct Modes(std::cell::RefCell<Vec<(u32, crate::NativeCheckMode)>>);
        impl NativeTypeProvider for Modes {
            fn contains_function(&self, _: NativeFunctionKey) -> bool { false }
            fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { unreachable!() }
            fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { unreachable!() }
            fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type { unreachable!() }
            fn return_cycle(&self, _: NativeFunctionKey) { unreachable!() }
            fn return_type_too_deep(&self, _: NativeFunctionKey) { unreachable!() }
            fn contains_expression(&self, key: crate::NativeExpressionKey) -> bool {
                key.snapshot == 7715 && key.node < 3
            }
            fn check_reusable_expression(&self, key: crate::NativeExpressionKey, _: Option<&Type>, mode: crate::NativeCheckMode, _: &mut dyn TypeQueries) -> (Type, bool) {
                self.0.borrow_mut().push((key.node, mode));
                (Type::String, true)
            }
            fn check_expression_pack(&self, key: crate::NativeExpressionKey, _: Option<&ValuePack>, mode: crate::NativeCheckMode, _: &mut dyn TypeQueries) -> ValuePack {
                self.0.borrow_mut().push((key.node, mode));
                ValuePack::single(Type::String)
            }
        }
        Engine::new().with_native_provider(Modes::default, |provider, queries| {
            let callable = Type::Function(crate::FunctionType {
                params: vec![Parameter { name: "value".into(), ty: Type::String, optional: false }],
                returns: Box::new(Type::String),
            });
            let expression = |node| crate::NativeExpressionKey { snapshot: 7715, node };
            let calls = |queries: &mut dyn TypeQueries| {
                let scalar = queries.call_native_arguments_scalar(&callable, None, &[crate::PackArgument::Expression(expression(0))], &[], None);
                assert!(scalar.errors.is_empty(), "{scalar:?}");
                let pack = queries.call_native_arguments(&callable, None, &[crate::PackArgument::ExpandExpression(expression(1))], &[], None);
                assert!(pack.errors.is_empty(), "{pack:?}");
            };
            queries.with_native_type_query(&mut |queries| {
                assert!(queries.native_type_query_mode().contains(crate::NativeCheckMode::TYPE_ONLY));
                queries.with_native_type_query(&mut |queries| calls(queries));
                calls(queries);
            });
            assert!(!provider.0.borrow().is_empty());
            assert!(provider.0.borrow().iter().all(|(_, mode)| mode.contains(crate::NativeCheckMode::TYPE_ONLY)));
            provider.0.borrow_mut().clear();
            assert!(queries.native_type_query_mode().is_empty());
            calls(queries);
            assert!(provider.0.borrow().iter().any(|(node, _)| *node == 0), "normal scalar check must not reuse the type-only argument cache");
            assert!(provider.0.borrow().iter().all(|(_, mode)| !mode.contains(crate::NativeCheckMode::TYPE_ONLY)));

            let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                queries.with_native_type_query(&mut |_| panic!("native type-query unwind fixture"));
            }));
            assert!(unwound.is_err());
            assert!(queries.native_type_query_mode().is_empty());
            provider.0.borrow_mut().clear();
            queries.call_native_arguments_scalar(&callable, None, &[crate::PackArgument::Expression(expression(2))], &[], None);
            assert!(!provider.0.borrow().is_empty());
            assert!(provider.0.borrow().iter().all(|(_, mode)| !mode.contains(crate::NativeCheckMode::TYPE_ONLY)));
        });
    }

    #[test]
    fn native_type_predicates_follow_the_selected_generic_overload_and_boolean_contract() {
        let mut number = head(Type::NativeTypeParameter(Provider::parameter(0)), Type::Boolean);
        number.generic = Some(0);
        number.constraint = Some(Type::Number);
        number.predicate = Some(Type::NativeTypeParameter(Provider::parameter(0)));
        let mut string = head(Type::String, Type::Boolean);
        string.predicate = Some(Type::String);
        with_provider(Provider::new(vec![number, string]), |provider, queries| {
            let callable = Type::NativeFunction(Provider::function(0));
            assert!(queries.may_have_type_predicate(&callable));
            assert_eq!(provider.predicate_reads.get(), 0);
            assert_eq!(provider.parameter_reads.get(), 0);
            assert_eq!(provider.return_reads.get(), 0);
            for ty in [Type::Number, Type::String] {
                let called = queries.call(&callable, &[ty.clone()]);
                assert!(called.errors.is_empty(), "{:?}", called.errors);
                assert_eq!(called.return_type, Type::Boolean);
                let predicate = called.predicate.expect("selected predicate");
                assert_eq!(predicate.parameter, 0);
                assert!(queries.is_assignable(predicate.ty.as_ref().unwrap(), &ty));
                assert!(queries.is_assignable(&ty, predicate.ty.as_ref().unwrap()));
                let packed = queries.call_native_arguments(&callable, None, &[crate::PackArgument::Resolved(ty.clone())], &[], None);
                assert!(packed.errors.is_empty());
                assert_eq!(packed.return_type, Type::Boolean);
                assert!(packed.predicate.is_some());
            }
            let failed = queries.call(&callable, &[Type::Boolean]);
            assert!(!failed.errors.is_empty());
            assert!(failed.predicate.is_none());
            assert_eq!(provider.implementation_reads.get(), 0);
        });
    }

    #[test]
    fn native_predicate_presence_skips_absent_metadata_without_resolving_contracts() {
        with_provider(Provider::new(vec![head(Type::Unknown, Type::Boolean)]), |provider, queries| {
            let signature = queries.signature_type(Provider::key(0));
            assert!(!queries.may_have_type_predicate(&signature));
            assert_eq!(provider.parameter_reads.get(), 0);
            assert_eq!(provider.return_reads.get(), 0);
            for _ in 0..3 {
                let result = queries.call(&signature, &[Type::String]);
                assert!(result.errors.is_empty());
                assert!(result.predicate.is_none());
            }
            assert_eq!(provider.predicate_reads.get(), 0);
        });
        with_provider(Provider::new(Vec::new()), |provider, queries| {
            let function = Type::NativeFunction(Provider::function(0));
            assert!(!queries.may_have_type_predicate(&function));
            assert!(queries.call(&function, &[Type::String]).predicate.is_none());
            assert_eq!(provider.function_predicate_reads.get(), 0);
        });
    }

    #[test]
    fn native_predicate_presence_preserves_union_and_intersection_candidates() {
        let mut predicate = head(Type::Unknown, Type::Boolean);
        predicate.predicate = Some(Type::String);
        let boolean = head(Type::Unknown, Type::Boolean);
        with_provider(Provider::new(vec![predicate, boolean]), |provider, queries| {
            let guard = queries.signature_type(Provider::key(0));
            let boolean = queries.signature_type(Provider::key(1));
            let union = Type::Union(vec![boolean.clone(), guard.clone()]);
            let intersection = Type::Intersection(vec![boolean, guard]);
            assert!(queries.may_have_type_predicate(&union));
            assert!(queries.may_have_type_predicate(&intersection));
            assert_eq!(provider.predicate_reads.get(), 0);
            assert_eq!(provider.parameter_reads.get(), 0);
            assert_eq!(provider.return_reads.get(), 0);
        });
    }

    #[test]
    fn native_written_type_predicate_metadata_is_cached_per_signature() {
        let mut predicate = head(Type::Unknown, Type::Boolean);
        predicate.predicate = Some(Type::String);
        with_provider(Provider::new(vec![predicate]), |provider, queries| {
            let signature = queries.signature_type(Provider::key(0));
            let first = queries.call(&signature, &[Type::String]);
            assert!(first.predicate.is_some());
            let initial_reads = provider.predicate_reads.get();
            assert!(initial_reads > 0);
            for _ in 0..3 {
                assert!(queries.call(&signature, &[Type::String]).predicate.is_some());
            }
            assert_eq!(provider.predicate_reads.get(), initial_reads);
        });
    }

    #[test]
    fn native_type_predicate_callable_relations_preserve_the_written_guard() {
        let mut predicate = head(Type::Unknown, Type::Boolean);
        predicate.predicate = Some(Type::String);
        let boolean = head(Type::Unknown, Type::Boolean);
        let mut number = head(Type::Unknown, Type::Boolean);
        number.predicate = Some(Type::Number);
        with_provider(Provider::new(vec![predicate, boolean, number]), |_, queries| {
            let guard = queries.signature_type(Provider::key(0));
            let boolean = queries.signature_type(Provider::key(1));
            let other = queries.signature_type(Provider::key(2));
            assert!(queries.is_assignable(&guard, &boolean));
            assert!(!queries.is_assignable(&boolean, &guard));
            assert!(!queries.is_assignable(&other, &guard));
        });
    }

    #[test]
    fn native_type_predicate_targets_preserve_captured_outer_instantiation() {
        let mut declaration = head(Type::Any, Type::Boolean);
        declaration.predicate = Some(Type::NativeTypeParameter(Provider::parameter(0)));
        let mut provider = Provider::new(vec![declaration]);
        provider.factory = true;
        with_provider(provider, |_, queries| {
            for outer in [Type::Number, Type::String] {
                let closure = queries.call(&Type::NativeFunction(Provider::function(1)), &[outer.clone()]).return_type;
                assert!(queries.may_have_type_predicate(&closure));
                let called = queries.call(&closure, &[outer.clone()]);
                assert!(called.errors.is_empty(), "{:?}", called.errors);
                assert_eq!(called.return_type, Type::Boolean);
                let predicate = called.predicate.expect("captured declaration predicate");
                assert!(queries.is_assignable(predicate.ty.as_ref().unwrap(), &outer));
                assert!(queries.is_assignable(&outer, predicate.ty.as_ref().unwrap()));
            }
        });
    }

    #[test]
    fn native_signature_identity_is_lazy_and_has_no_runtime_identity_from_owner() {
        let provider = Provider::new(vec![head(Type::Number, Type::String)]);
        with_provider(provider.clone(), |_provider, queries| {
            let annotation = queries.signature_type(Provider::key(0));
            assert_eq!(queries.native_signature_key(&annotation), Some(Provider::key(0)));
            assert_eq!(_provider.parameter_reads.get(), 0);
            assert_eq!(_provider.return_reads.get(), 0);
            assert_eq!(queries.native_function_key(&annotation), None);
            let called = queries.call(&annotation, &[Type::Number]);
            assert!(called.errors.is_empty());
            assert_eq!(called.return_type, Type::String);
            assert_eq!(_provider.implementation_reads.get(), 0);
        });
    }

    #[test]
    fn native_overloads_preserve_source_order_and_later_declaration_site_precedence() {
        for later in [false, true] {
            let mut heads = vec![head(Type::Any, Type::Number), head(Type::Any, Type::String)];
            heads[1].site = u32::from(later);
            let provider = Provider::new(heads);
            with_provider(provider.clone(), |_provider, queries| {
                let called = queries.call(
                    &Type::NativeFunction(Provider::function(0)),
                    &[Type::Number],
                );
                assert!(called.errors.is_empty());
                assert_eq!(
                    called.return_type,
                    if later { Type::String } else { Type::Number }
                );
                assert_eq!(_provider.implementation_reads.get(), 0);
            });
        }
    }

    #[test]
    fn native_overloads_prioritize_only_direct_literal_parameter_heads() {
        for direct in [false, true] {
            let mut heads = vec![
                head(Type::String, Type::String),
                head(Type::StringLiteral(b"event".to_vec()), Type::Number),
            ];
            heads[1].literal = direct;
            let provider = Provider::new(heads);
            with_provider(provider.clone(), |_provider, queries| {
                let called = queries.call(
                    &Type::NativeFunction(Provider::function(0)),
                    &[Type::StringLiteral(b"event".to_vec())],
                );
                assert!(called.errors.is_empty());
                assert_eq!(
                    called.return_type,
                    if direct { Type::Number } else { Type::String }
                );
            });
        }
    }

    #[test]
    fn native_overloads_use_subtype_pass_before_assignable_for_any_arguments() {
        let provider = Provider::new(vec![
            head(Type::Number, Type::Number),
            head(Type::Any, Type::String),
        ]);
        with_provider(provider.clone(), |_provider, queries| {
            let called = queries.call(&Type::NativeFunction(Provider::function(0)), &[Type::Any]);
            assert!(called.errors.is_empty());
            assert_eq!(called.return_type, Type::String);
        });
    }

    #[test]
    fn native_overloads_infer_distinct_signature_local_generics_and_check_constraints() {
        let mut heads = vec![
            head(
                Type::NativeTypeParameter(Provider::parameter(0)),
                Type::NativeTypeParameter(Provider::parameter(0)),
            ),
            head(
                Type::NativeTypeParameter(Provider::parameter(1)),
                Type::NativeTypeParameter(Provider::parameter(1)),
            ),
        ];
        heads[0].generic = Some(0);
        heads[0].constraint = Some(Type::Number);
        heads[1].generic = Some(1);
        heads[1].constraint = Some(Type::String);
        let provider = Provider::new(heads);
        with_provider(provider.clone(), |_provider, queries| {
            for ty in [Type::Number, Type::String] {
                let called =
                    queries.call(&Type::NativeFunction(Provider::function(0)), &[ty.clone()]);
                assert!(called.errors.is_empty(), "{:?}", called.errors);
                assert_eq!(called.return_type, ty);
            }
            let invalid = queries.call(
                &Type::NativeFunction(Provider::function(0)),
                &[Type::Boolean],
            );
            assert!(matches!(
                invalid.errors.as_slice(),
                [CallError::Argument { .. }]
            ));
        });
    }

    #[test]
    fn native_overloaded_returned_closures_preserve_outer_mapper_and_runtime_capture() {
        let mut provider = Provider::new(vec![
            head(
                Type::Number,
                Type::NativeTypeParameter(Provider::parameter(0)),
            ),
            head(
                Type::String,
                Type::NativeTypeParameter(Provider::parameter(0)),
            ),
        ]);
        provider.factory = true;
        let retained = with_provider(provider.clone(), |_provider, queries| {
            let called = queries.call(
                &Type::NativeFunction(Provider::function(1)),
                &[Type::Number],
            );
            assert!(called.errors.is_empty());
            let closure = called.return_type;
            assert_eq!(
                queries.native_function_key(&closure),
                Some(Provider::function(0))
            );
            for arg in [Type::Number, Type::String] {
                let called = queries.call(&closure, &[arg]);
                assert!(called.errors.is_empty(), "{:?}", called.errors);
                assert_eq!(called.return_type, Type::Number);
            }
            assert_eq!(_provider.implementation_reads.get(), 0);
            closure
        });
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Engine::new().with_session(|queries| queries.format(&retained))
        }))
        .expect_err("captured overload capability cannot cross epochs");
        assert!(failure.downcast::<InvalidSessionType>().is_ok());
    }

    #[test]
    fn native_standalone_generic_signature_preserves_whole_return_packs_and_foreign_keys() {
        let mut generic = head(Type::NativeTypeParameter(Provider::parameter(0)), Type::Nil);
        generic.generic = Some(0);
        generic.returns = ValuePack::Alternatives(vec![PackAlternative {
            fixed: vec![
                PackSlot::required(Type::NativeTypeParameter(Provider::parameter(0))),
                PackSlot::required(Type::Number),
            ],
            tail: None,
        }]);
        let mut provider = Provider::new(vec![generic]);
        provider.standalone = true;
        with_provider(provider.clone(), |_provider, queries| {
            let called = queries.call_pack(
                &Type::NativeSignature(Provider::key(0)),
                &[Type::String],
                &[],
                None,
            );
            assert!(called.errors.is_empty());
            assert_eq!(queries.pack_slot(&called.return_pack, 0), Type::String);
            assert_eq!(queries.pack_slot(&called.return_pack, 1), Type::Number);
        });
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_provider(provider.clone(), |_provider, queries| {
                queries.signature_type(NativeSignatureKey {
                    snapshot: 1220,
                    index: 0,
                });
            })
        }))
        .expect_err("foreign signature snapshot must be rejected");
        assert!(failure.downcast::<InvalidNativeSignatureKey>().is_ok());
    }

    #[test]
    fn native_named_callable_heads_use_named_mapper_without_runtime_identity() {
        let mut provider = Provider::new(vec![
            head(
                Type::Number,
                Type::NativeTypeParameter(Provider::parameter(0)),
            ),
            head(
                Type::String,
                Type::NativeTypeParameter(Provider::parameter(0)),
            ),
        ]);
        provider.named = true;
        with_provider(provider, |_, queries| {
            let callable = queries
                .named_type_reference(
                    NativeTypeDeclarationKey {
                        snapshot: 1219,
                        index: 0,
                    },
                    &[Type::Boolean],
                )
                .unwrap();
            assert_eq!(queries.native_function_key(&callable), None);
            assert!(queries.function_implementation_type(&callable).is_none());
            for argument in [Type::Number, Type::String] {
                let called = queries.call(&callable, &[argument]);
                assert!(called.errors.is_empty(), "{:?}", called.errors);
                assert_eq!(called.return_type, Type::Boolean);
            }
            let expected = queries.pack_function(&crate::PackFunctionType {
                params: vec![Parameter {
                    name: "value".into(),
                    ty: Type::Number,
                    optional: false,
                }],
                rest: None,
                type_parameters: Vec::new(),
                returns: ValuePack::single(Type::Boolean),
            });
            assert!(
                queries.is_assignable(&callable, &expected),
                "named callable signatures must participate in recursive type relations"
            );
        });
    }

    #[test]
    fn native_overload_body_compatibility_checks_complete_packs_and_ignored_returns() {
        let mut declaration = head(Type::Number, Type::Number);
        declaration.returns = ValuePack::Alternatives(vec![PackAlternative {
            fixed: vec![
                PackSlot::required(Type::Number),
                PackSlot::required(Type::String),
            ],
            tail: None,
        }]);
        let provider = Provider::new(vec![
            declaration,
            head(Type::Number, Type::Number),
            Head {
                returns: ValuePack::empty(),
                ..head(Type::Number, Type::Nil)
            },
        ]);
        with_provider(provider, |_, queries| {
            let params = vec![Parameter {
                name: "value".into(),
                ty: Type::Union(vec![Type::Number, Type::String]),
                optional: false,
            }];
            let short = queries.pack_function(&crate::PackFunctionType {
                params: params.clone(),
                rest: None,
                type_parameters: Vec::new(),
                returns: ValuePack::single(Type::Number),
            });
            assert!(!queries.signature_implementation_compatible(Provider::key(0), &short));
            let full = queries.pack_function(&crate::PackFunctionType {
                params: params.clone(),
                rest: None,
                type_parameters: Vec::new(),
                returns: ValuePack::Alternatives(vec![PackAlternative {
                    fixed: vec![
                        PackSlot::required(Type::Number),
                        PackSlot::required(Type::String),
                    ],
                    tail: None,
                }]),
            });
            assert!(queries.signature_implementation_compatible(Provider::key(0), &full));
            let wider = queries.pack_function(&crate::PackFunctionType {
                params: params.clone(),
                rest: None,
                type_parameters: Vec::new(),
                returns: ValuePack::Alternatives(vec![
                    PackAlternative {
                        fixed: vec![PackSlot::required(Type::Number)],
                        tail: None,
                    },
                    PackAlternative {
                        fixed: vec![PackSlot::required(Type::String)],
                        tail: None,
                    },
                ]),
            });
            assert!(queries.signature_implementation_compatible(Provider::key(1), &wider));
            assert!(queries.signature_implementation_compatible(Provider::key(2), &full));
            let wrong_parameter = queries.pack_function(&crate::PackFunctionType {
                params: vec![Parameter {
                    name: "value".into(),
                    ty: Type::String,
                    optional: false,
                }],
                rest: None,
                type_parameters: Vec::new(),
                returns: ValuePack::single(Type::Number),
            });
            assert!(
                !queries.signature_implementation_compatible(Provider::key(1), &wrong_parameter)
            );
        });
    }

    #[test]
    fn lua_physical_nil_arguments_require_the_implementation_contract_to_admit_nil() {
        for convention in [FunctionCallingConvention::TypeScript, FunctionCallingConvention::Lua] {
            let mut provider = Provider::new(vec![
                head(Type::String, Type::String),
                head(Type::Number, Type::String),
                head(Type::String, Type::Number),
            ]);
            provider.calling_convention = convention;
            provider.parameterless_heads.push(0);
            provider.implementation_parameter = Type::Union(vec![Type::String, Type::Nil]);
            provider.implementation_contract = Some(ValuePack::single(Type::String));
            with_provider(provider, |_, queries| {
                let implementation = queries.function_implementation_type(&Type::NativeFunction(Provider::function(0))).unwrap();
                assert_eq!(queries.signature_implementation_compatible(Provider::key(0), &implementation), convention == FunctionCallingConvention::Lua);
                assert!(!queries.signature_implementation_compatible(Provider::key(1), &implementation), "wrong parameter types still fail");
                assert!(!queries.signature_implementation_compatible(Provider::key(2), &implementation), "wrong result types still fail");
                assert!(!queries.call(&implementation, &[]).errors.is_empty(), "the compatibility projection does not change the actual implementation signature");
                assert!(queries.call(&implementation, &[Type::StringLiteral(b"value".to_vec())]).errors.is_empty());
                assert!(queries.call(&Type::NativeFunction(Provider::function(0)), &[]).errors.is_empty(), "the original public zero-argument head remains callable");
            });
            let mut nonnullable = Provider::new(vec![head(Type::String, Type::String)]);
            nonnullable.calling_convention = convention;
            nonnullable.parameterless_heads.push(0);
            nonnullable.implementation_parameter = Type::String;
            nonnullable.implementation_contract = Some(ValuePack::single(Type::String));
            with_provider(nonnullable, |_, queries| {
                let implementation = queries.function_implementation_type(&Type::NativeFunction(Provider::function(0))).unwrap();
                assert!(!queries.signature_implementation_compatible(Provider::key(0), &implementation), "an omitted argument is nil, not an arity waiver");
            });
        }
    }

    #[test]
    fn native_overloaded_generic_callback_candidates_use_the_shared_contextual_inference_loop() {
        let callback = |parameter| {
            Type::Function(crate::FunctionType {
                params: vec![Parameter {
                    name: "value".into(),
                    ty: Type::NativeTypeParameter(Provider::parameter(parameter)),
                    optional: false,
                }],
                returns: Box::new(Type::NativeTypeParameter(Provider::parameter(parameter))),
            })
        };
        let mut heads = vec![
            head(
                callback(0),
                Type::NativeTypeParameter(Provider::parameter(0)),
            ),
            head(
                callback(1),
                Type::NativeTypeParameter(Provider::parameter(1)),
            ),
        ];
        heads[0].generic = Some(0);
        heads[0].constraint = Some(Type::Number);
        heads[1].generic = Some(1);
        heads[1].constraint = Some(Type::String);
        let mut provider = Provider::new(heads);
        provider.contextual = true;
        with_provider(provider, |provider, queries| {
            let called = queries.call_native_arguments(
                &Type::NativeFunction(Provider::function(0)),
                None,
                &[crate::PackArgument::Expression(
                    crate::NativeExpressionKey {
                        snapshot: 1219,
                        node: 0,
                    },
                )],
                &[],
                None,
            );
            assert!(called.errors.is_empty(), "{:?}", called.errors);
            assert_eq!(called.return_type, Type::String);
            assert!(provider.contextual_checks.get() > 0);
            assert_eq!(provider.implementation_reads.get(), 0);
        });
    }

    #[test]
    fn native_overload_type_argument_errors_use_the_failed_generic_head() {
        let mut generic = head(
            Type::NativeTypeParameter(Provider::parameter(0)),
            Type::NativeTypeParameter(Provider::parameter(0)),
        );
        generic.generic = Some(0);
        generic.constraint = Some(Type::String);
        let provider = Provider::new(vec![head(Type::Number, Type::Number), generic]);
        with_provider(provider, |_, queries| {
            let invalid = queries.call_with_type_arguments(
                &Type::NativeFunction(Provider::function(0)),
                &[Type::String],
                &[Type::Number],
                None,
            );
            assert!(
                matches!(
                    invalid.errors.as_slice(),
                    [CallError::TypeArgument {
                        source: Type::Number,
                        target: Type::String,
                        ..
                    }]
                ),
                "{:?}",
                invalid.errors
            );
        });
    }
    #[test]
    fn native_public_signature_descriptions_keep_heads_and_captured_mapper_separate_from_implementation()
     {
        let mut provider = Provider::new(vec![
            head(
                Type::Number,
                Type::NativeTypeParameter(Provider::parameter(0)),
            ),
            head(
                Type::String,
                Type::NativeTypeParameter(Provider::parameter(0)),
            ),
        ]);
        provider.factory = true;
        with_provider(provider, |provider, queries| {
            let closure = queries
                .call(
                    &Type::NativeFunction(Provider::function(1)),
                    &[Type::Boolean],
                )
                .return_type;
            let public = queries.call_signature_types(&closure);
            assert_eq!(public.len(), 2);
            for ((head, parameter), index) in public.iter().zip([Type::Number, Type::String]).zip(0..) {
                assert_eq!(queries.native_signature_key(head), Some(Provider::key(index)));
                let signature = queries.function_signature(head).unwrap();
                assert_eq!(signature.params[0].ty, parameter);
                assert_eq!(signature.returns.as_ref(), &Type::Boolean);
                let pack = queries.function_result_pack(head).unwrap();
                assert_eq!(queries.pack_slot(&pack, 0), Type::Boolean);
            }
            assert_eq!(provider.implementation_reads.get(), 0);
            let implementation = queries.function_implementation_type(&closure).unwrap();
            assert_eq!(queries.call_signature_types(&implementation).len(), 1);
            let signature = queries.function_signature(&implementation).unwrap();
            assert_eq!(signature.params[0].ty, Type::Any);
        });
    }

    #[test]
    fn native_overload_compatibility_uses_written_pack_contract_without_demanding_body_count() {
        for written in [true, false] {
            let one = head(Type::Number, Type::Number);
            let mut two = head(Type::String, Type::Number);
            two.returns = ValuePack::Alternatives(vec![PackAlternative {
                fixed: vec![
                    PackSlot::required(Type::Number),
                    PackSlot::required(Type::Number),
                ],
                tail: None,
            }]);
            let mut provider = Provider::new(vec![one, two]);
            provider.implementation_pack = Some(ValuePack::Alternatives(vec![PackAlternative {
                fixed: vec![
                    PackSlot::required(Type::Number),
                    PackSlot::required(Type::Number),
                ],
                tail: None,
            }]));
            if written {
                provider.implementation_contract = Some(ValuePack::variadic(Type::Number));
            }
            with_provider(provider, |provider, queries| {
                let implementation = queries
                    .function_implementation_type(&Type::NativeFunction(Provider::function(0)))
                    .unwrap();
                assert_eq!(
                    queries.signature_implementation_compatible(Provider::key(0), &implementation),
                    written
                );
                assert!(
                    queries.signature_implementation_compatible(Provider::key(1), &implementation)
                );
                if written {
                    assert_eq!(
                        provider.body_pack_reads.get(),
                        0,
                        "written overload compatibility must not inspect body cardinality"
                    );
                } else {
                    assert!(
                        provider.body_pack_reads.get() > 0,
                        "unannotated compatibility must inspect physical returns"
                    );
                }
            });
        }
    }
    #[test]
    fn native_callback_relations_use_written_contract_without_erasing_physical_termination_rows() {
        for written in [true, false] {
            let successful = PackAlternative {
                fixed: vec![
                    PackSlot::required(Type::Number),
                    PackSlot::required(Type::String),
                ],
                tail: None,
            };
            let contract = ValuePack::Alternatives(vec![
                PackAlternative {
                    fixed: vec![PackSlot::required(Type::Nil)],
                    tail: None,
                },
                successful.clone(),
            ]);
            let physical = ValuePack::Alternatives(vec![
                PackAlternative {
                    fixed: Vec::new(),
                    tail: None,
                },
                successful,
            ]);
            let mut declaration = head(Type::Number, Type::Nil);
            declaration.returns = contract.clone();
            let mut provider = Provider::new(vec![declaration]);
            if written {
                provider.implementation_contract = Some(contract);
            }
            provider.implementation_pack = Some(physical);
            with_provider(provider, |provider, queries| {
                let source = queries
                    .function_implementation_type(&Type::NativeFunction(Provider::function(0)))
                    .unwrap();
                let target = queries.signature_type(Provider::key(0));
                assert!(queries.is_assignable(&source, &target));
                if written {
                    assert_eq!(provider.body_pack_reads.get(), 0);
                } else {
                    assert!(provider.body_pack_reads.get() > 0);
                }
                let results = queries.function_result_pack(&source).unwrap();
                let rows = queries.inspect_pack(&results).unwrap();
                assert!(
                    rows.iter()
                        .any(|row| row.fixed.is_empty() && row.tail.is_none())
                );
                assert!(
                    !rows
                        .iter()
                        .any(|row| row.fixed.len() == 1 && row.fixed[0].ty == Type::Nil)
                );
                assert!(rows.iter().any(|row| row.fixed.len() == 2
                    && row.fixed[0].ty == Type::Number
                    && row.fixed[1].ty == Type::String));
                let strict_two = queries.pack_function(&crate::PackFunctionType {
                    params: vec![Parameter {
                        name: "value".into(),
                        ty: Type::Number,
                        optional: false,
                    }],
                    rest: None,
                    type_parameters: Vec::new(),
                    returns: ValuePack::Alternatives(vec![PackAlternative {
                        fixed: vec![
                            PackSlot::required(Type::Number),
                            PackSlot::required(Type::String),
                        ],
                        tail: None,
                    }]),
                });
                assert!(
                    !queries.is_assignable(&source, &strict_two),
                    "termination cannot satisfy two required successful results"
                );
            });
        }
    }
    #[test]
    fn native_nil_annotation_callable_contract_does_not_guarantee_an_expanded_argument() {
        with_provider(
            Provider::new(vec![head(Type::Number, Type::Nil)]),
            |_, queries| {
                let declaration = queries.signature_type(Provider::key(0));
                let returned = queries.call_pack(&declaration, &[Type::Number], &[], None);
                assert!(returned.errors.is_empty());
                let rows = queries.inspect_pack(&returned.return_pack).unwrap();
                assert!(rows.iter().any(|row| row.fixed.is_empty()));
                assert!(
                    rows.iter()
                        .any(|row| row.fixed.len() == 1 && row.fixed[0].ty == Type::Nil)
                );
                let consume = queries.pack_function(&crate::PackFunctionType {
                    params: vec![Parameter {
                        name: "value".into(),
                        ty: Type::Nil,
                        optional: false,
                    }],
                    rest: None,
                    type_parameters: Vec::new(),
                    returns: ValuePack::empty(),
                });
                let expanded = queries.call_native_arguments(
                    &consume,
                    None,
                    &[crate::PackArgument::Expand(returned.return_pack.clone())],
                    &[],
                    None,
                );
                assert!(matches!(
                    expanded.errors.as_slice(),
                    [CallError::PackArity {
                        actual_minimum: 0,
                        actual_maximum: Some(1),
                        minimum: 1,
                        maximum: Some(1),
                        ..
                    }]
                ));
                let scalar = queries.pack_slot(&returned.return_pack, 0);
                assert!(queries.call(&consume, &[scalar]).errors.is_empty());
            },
        );
    }

    #[test]
    fn native_declaration_signature_group_exposes_all_heads_without_runtime_fallback() {
        with_provider(
            Provider::new(vec![
                head(Type::Number, Type::String),
                head(Type::String, Type::Number),
            ]),
            |provider, queries| {
                let callable = queries.signature_group_type(&[Provider::key(0), Provider::key(1)]);
                assert_eq!(queries.native_signature_key(&callable), None);
                assert_eq!(provider.parameter_reads.get(), 0);
                assert_eq!(provider.return_reads.get(), 0);
                assert_eq!(queries.native_function_key(&callable), None);
                assert_eq!(queries.function_implementation_type(&callable), None);
                for (argument, expected) in
                    [(Type::Number, Type::String), (Type::String, Type::Number)]
                {
                    let called = queries.call(&callable, &[argument]);
                    assert!(called.errors.is_empty());
                    assert_eq!(called.return_type, expected);
                }
                assert!(!queries.call(&callable, &[Type::Boolean]).errors.is_empty());
                let public = queries.call_signature_types(&callable);
                assert_eq!(public.len(), 2);
                for (index, signature) in public.into_iter().enumerate() {
                    assert_eq!(queries.native_signature_key(&signature), Some(Provider::key(index as u32)));
                    assert_eq!(queries.native_function_key(&signature), None);
                    assert_eq!(queries.function_implementation_type(&signature), None);
                }
                assert_eq!(provider.implementation_reads.get(), 0);
                assert_eq!(provider.body_pack_reads.get(), 0);
            },
        );
    }

    #[test]
    fn native_declaration_signature_group_keeps_signature_local_generic_inference() {
        let parameter = Type::NativeTypeParameter(Provider::parameter(0));
        let mut generic = head(parameter.clone(), parameter);
        generic.generic = Some(0);
        with_provider(Provider::new(vec![generic]), |provider, queries| {
            let callable = queries.signature_group_type(&[Provider::key(0)]);
            for argument in [Type::Number, Type::String] {
                let called = queries.call(&callable, &[argument.clone()]);
                assert!(called.errors.is_empty());
                assert_eq!(called.return_type, argument);
            }
            assert_eq!(queries.function_implementation_type(&callable), None);
            assert_eq!(provider.implementation_reads.get(), 0);
        });
    }

    #[test]
    fn native_declaration_signature_groups_validate_all_original_keys_and_session_ownership() {
        let provider = Provider::new(vec![head(Type::Number, Type::String)]);
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_provider(provider.clone(), |provider, queries| {
                let foreign = NativeSignatureKey {
                    snapshot: 1220,
                    index: 0,
                };
                let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    queries.signature_group_type(&[Provider::key(0), foreign]);
                }))
                .expect_err("foreign declaration head must be rejected");
                assert_eq!(
                    *failure.downcast::<InvalidNativeSignatureKey>().unwrap(),
                    InvalidNativeSignatureKey(foreign)
                );
                assert_eq!(provider.parameter_reads.get(), 0);
                assert_eq!(provider.return_reads.get(), 0);
                queries.signature_group_type(&[]);
            });
        }))
        .expect_err("an empty group cannot manufacture an implementation fallback");
        assert!(failure.is::<String>() || failure.is::<&str>());
        let escaped = with_provider(provider.clone(), |_, queries| {
            queries.signature_group_type(&[Provider::key(0)])
        });
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_provider(provider, |_, queries| {
                queries.call_signature_types(&escaped)
            });
        }))
        .expect_err("declaration group is an owned session type");
        assert!(failure.downcast::<InvalidSessionType>().is_ok());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidNativeSignatureKey(pub NativeSignatureKey);

pub(super) fn core_signature_key(key: NativeSignatureKey) -> NativeSignatureId {
    NativeSignatureId {
        snapshot: key.snapshot,
        index: key.index,
    }
}
pub(super) fn source_signature_key(key: NativeSignatureId) -> NativeSignatureKey {
    NativeSignatureKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}

impl SessionQueries<'_, '_, '_> {
    pub(super) fn signature_type_native(&mut self, declaration: NativeSignatureKey) -> Type {
        let ty = self
            .checker
            .native_signature_type(core_signature_key(declaration))
            .unwrap_or_else(|| std::panic::panic_any(InvalidNativeSignatureKey(declaration)));
        self.handles.retain(ty)
    }

    pub(super) fn signature_group_type_native(
        &mut self,
        declarations: &[NativeSignatureKey],
    ) -> Type {
        let declarations: Vec<_> = declarations
            .iter()
            .copied()
            .map(core_signature_key)
            .collect();
        let ty = self
            .checker
            .native_signature_group_type(&declarations)
            .unwrap_or_else(|key| {
                std::panic::panic_any(InvalidNativeSignatureKey(source_signature_key(key)))
            });
        self.handles.retain(ty)
    }
}

impl<P: NativeTypeProvider> ProviderAdapter<'_, P> {
    pub(super) fn signature_header_core(
        &self,
        declaration: NativeSignatureId,
    ) -> NativeSignatureHeader {
        let header = self
            .provider
            .signature_header(source_signature_key(declaration));
        let owner = match header.owner {
            SignatureOwner::Standalone => NativeSignatureOwner::Standalone,
            SignatureOwner::Function(key) => NativeSignatureOwner::Function(core_key(key)),
            SignatureOwner::NamedType(key) => NativeSignatureOwner::NamedType(
                bun_sema::check::native_named::NativeTypeDeclarationId {
                    snapshot: key.snapshot,
                    index: key.index,
                },
            ),
        };
        NativeSignatureHeader {
            owner,
            site: NativeSignatureSite {
                snapshot: header.site.snapshot,
                index: header.site.index,
            },
            parent: header.parent.map(core_signature_key),
            has_direct_literal_parameter: header.has_direct_literal_parameter,
        }
    }

    pub(super) fn signature_parameters_core(
        &self,
        checker: &mut Checker<'_, '_>,
        declaration: NativeSignatureId,
    ) -> Vec<SigParam> {
        let key = source_signature_key(declaration);
        let parameters = self.provider.signature_parameters(
            key,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        let mut params = parameters
            .into_iter()
            .map(|parameter| {
                let mut ty = import_type(checker, self.handles, &parameter.ty);
                if parameter.optional {
                    ty = checker.optional(ty);
                }
                SigParam {
                    name: checker.atoms().intern(parameter.name.as_bytes()),
                    ty,
                    optional: parameter.optional,
                    rest: false,
                    is_required_rest: false,
                    declaration: None,
                }
            })
            .collect::<Vec<_>>();
        let rest = self.provider.signature_rest_pack(
            key,
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        if let Some(rest) = rest {
            let pack = import_pack(checker, self.handles, &rest);
            params.push(SigParam {
                name: checker.atoms().intern(b"..."),
                ty: checker.native_pack_type(pack),
                optional: false,
                rest: true,
                is_required_rest: false,
                declaration: None,
            });
        }
        params
    }

    pub(super) fn signature_return_pack_core(
        &self,
        checker: &mut Checker<'_, '_>,
        declaration: NativeSignatureId,
    ) -> bun_sema::check::NativePackResult {
        let pack = self.provider.signature_declared_return_pack(
            source_signature_key(declaration),
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        let pack = super::native_packs::import_declared_pack(checker, self.handles, &pack);
        bun_sema::check::NativePackResult::Declaration(
            checker.native_pack_with_implicit_empty(pack),
        )
    }
}
