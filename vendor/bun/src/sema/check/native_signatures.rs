//! Original written callable heads and their independent runtime implementations.

use super::{Checker, NativeFunctionProvider};
use super::native_named::NativeTypeDeclarationId;
use crate::types::{MapperId, NativeFunctionId, NativeSignatureId, Shape, SigData, SigId, TypeId};

/// Physical runtime implementation arity is separate from written public
/// callable contracts. Lua fills omitted fixed arguments with nil.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum NativeCallingConvention { TypeScript, Lua }

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum NativeSignatureOwner {
    Standalone,
    Function(NativeFunctionId),
    NamedType(NativeTypeDeclarationId),
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeSignatureSite {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct NativeSignatureHeader {
    pub owner: NativeSignatureOwner,
    pub site: NativeSignatureSite,
    pub parent: Option<NativeSignatureId>,
    /// Matches TS's direct, unparenthesized literal parameter syntax flag.
    pub has_direct_literal_parameter: bool,
}

impl<'p, 's> Checker<'p, 's> {
    /// A pure written scalar contract, sufficient for nonreturning call flow.
    /// Multiple public heads establish that effect only when every one is never.
    pub fn native_callable_declared_scalar_return(&mut self, callable: TypeId) -> Option<TypeId> {
        let signatures = self.signatures(callable, false).to_vec();
        let first = self.sig_declared_scalar_return(*signatures.first()?)?;
        if signatures.len() == 1 { return Some(first); }
        if first != TypeId::NEVER { return None; }
        for signature in signatures.into_iter().skip(1) {
            if self.sig_declared_scalar_return(signature)? != TypeId::NEVER { return None; }
        }
        Some(TypeId::NEVER)
    }

    pub(super) fn native_signature_provider_for(&self, declaration: NativeSignatureId) -> &'p dyn NativeFunctionProvider {
        let provider = self.native_function_provider.expect("native signature without a provider");
        assert!(provider.contains_signature(declaration), "foreign native signature identity");
        provider
    }

    pub fn native_signature_type(&mut self, declaration: NativeSignatureId) -> Option<TypeId> {
        if !self.native_function_provider?.contains_signature(declaration) { return None; }
        let provider = self.native_signature_provider_for(declaration);
        let outer = provider.signature_outer_type_parameters(declaration).into_iter().map(|id| {
            let param = self.cloned_native_type_param(id, MapperId::IDENTITY);
            (param, param)
        }).collect();
        let sig = self.types().intern_sig(SigData::NativeDeclarationSignature {
            declaration, mapper: self.types().mapper(outer), implementation: None,
        });
        let sig = self.native_signature_with_outer_instances(sig);
        Some(self.type_of_signature(sig, false))
    }

    /// Seed the exact free-instance domain before object flags and mapper
    /// projection are computed. Resolving this original metadata never asks
    /// for parameters, results or the function body.
    pub(super) fn native_signature_with_outer_instances(&mut self, signature: SigId) -> SigId {
        let (instances, mapper) = match *self.types().sig(signature) {
            SigData::Native { declaration, mapper } => (self.native_function_provider.expect("native function provider").outer_type_parameter_instances(self, declaration), mapper),
            SigData::NativeDeclarationSignature { declaration, mapper, .. } => (self.native_signature_provider_for(declaration).signature_outer_type_parameter_instances(self, declaration), mapper),
            _ => return signature,
        };
        if instances.is_empty() { return signature; }
        let own = self.sig_type_params(signature).into_vec();
        let mut mapping = self.types().mapping(mapper).to_vec();
        for parameter in instances {
            let Some((declaration, _)) = self.native_type_parameter_decl(parameter) else { panic!("native outer instance must be a type parameter") };
            assert!(!own.iter().any(|&own| self.native_type_parameter_decl(own).is_some_and(|(id, _)| id == declaration)), "native outer instance must not bind a callable's own parameter");
            if !mapping.iter().any(|pair| pair.0 == parameter) { mapping.push((parameter, parameter)); }
        }
        let mapper = self.types().mapper(mapping);
        let data = match *self.types().sig(signature) {
            SigData::Native { declaration, .. } => SigData::Native { declaration, mapper },
            SigData::NativeDeclarationSignature { declaration, implementation, .. } => SigData::NativeDeclarationSignature { declaration, mapper, implementation },
            _ => unreachable!(),
        };
        self.types().intern_sig(data)
    }

    /// A declaration-only callable set. Original heads retain their own generic
    /// scopes and source ordering, without borrowing any runtime implementation.
    pub fn native_signature_group_type(&mut self, declarations: &[NativeSignatureId]) -> Result<TypeId, NativeSignatureId> {
        assert!(!declarations.is_empty(), "native signature group requires an original declaration");
        let Some(provider) = self.native_function_provider else { return Err(declarations[0]); };
        if let Some(&invalid) = declarations.iter().find(|&&key| !provider.contains_signature(key)) { return Err(invalid); }
        let mut shape = Shape::new_in(self.arena);
        for &declaration in declarations {
            let ty = self.native_signature_type(declaration).expect("validated original signature");
            shape.call.push(self.single_call_signature(ty).expect("one original declaration head"));
        }
        Ok(self.synth(shape))
    }

    pub(super) fn native_function_overload_type(&self, implementation: SigId) -> TypeId {
        let Some((function, mapper)) = self.sig_native_decl(implementation) else { return self.type_of_signature(implementation, false) };
        let declarations = self.native_function_provider.expect("native function provider").overload_signatures(function);
        if declarations.is_empty() { return self.type_of_signature(implementation, false); }
        let mut shape = Shape::new_in(self.arena);
        for declaration in declarations {
            self.native_signature_provider_for(declaration);
            shape.call.push(self.types().intern_sig(SigData::NativeDeclarationSignature { declaration, mapper, implementation: Some(implementation) }));
        }
        if self.native_function_provider.expect("native function provider").exposes_implementation(function) {
            shape.call.push(implementation);
        }
        self.synth(shape)
    }

    pub fn native_implementation_signature(&mut self, ty: TypeId) -> Option<SigId> {
        let signatures = self.signatures(ty, false);
        let mut implementation = None;
        for &signature in signatures.iter() {
            let signature = match *self.types().sig(signature) {
                SigData::NativeDeclarationSignature { implementation: Some(runtime), .. } => runtime,
                SigData::NativeDeclarationSignature { implementation: None, .. } => return None,
                _ if self.sig_native_decl(signature).is_some() => signature,
                _ => return None,
            };
            if implementation.is_some_and(|prior| prior != signature) { return None; }
            implementation = Some(signature);
        }
        implementation
    }

    pub fn native_implementation_type(&mut self, ty: TypeId) -> Option<TypeId> {
        let implementation = self.native_implementation_signature(ty)?;
        Some(self.type_of_signature(implementation, false))
    }

    pub fn native_call_signature_types(&mut self, ty: TypeId) -> Vec<TypeId> {
        let signatures = self.signatures(ty, false);
        signatures.iter().map(|&sig| self.type_of_signature(sig, false)).collect()
    }

    /// Original doc signatures retain the absence of a result contract through
    /// mapping/wrapping. Runtime functions always retain their inferred result.
    pub(super) fn sig_has_unconstrained_native_return(&self, sig: SigId) -> bool {
        self.sig_native_signature_decl(sig).is_some_and(|(declaration, _)| {
            self.native_signature_provider_for(declaration).signature_has_unconstrained_return(declaration)
        })
    }

    pub fn sig_native_signature_decl(&self, sig: SigId) -> Option<(NativeSignatureId, MapperId)> {
        match *self.types().sig(sig) {
            SigData::NativeDeclarationSignature { declaration, mapper, .. } => Some((declaration, mapper)),
            SigData::WithReturn { sig: inner, .. } | SigData::WithNativeResultPack { sig: inner, .. } => self.sig_native_signature_decl(inner),
            _ => None,
        }
    }

    pub fn native_signature_implementation_compatible(&mut self, declaration: NativeSignatureId, implementation: TypeId) -> bool {
        let Some(overload) = self.native_signature_type(declaration).and_then(|ty| self.single_call_signature(ty)) else { return false; };
        let Some(implementation) = self.native_implementation_signature(implementation).or_else(|| self.single_call_signature(implementation)) else { return false; };
        self.is_implementation_compatible_with_overload(implementation, overload)
    }
}
