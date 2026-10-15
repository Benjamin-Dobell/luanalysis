//! Trusted original written operations. Ordinary signatures still select and
//! validate calls; only their successful argument-dependent result is replaced.
use super::{CheckMode, Checker, ResolvedTypeCallError};
use super::call::Arg;
use super::native_globals::{NativeGlobalBindingId, NativeGlobalQueryError, NativeGlobalSourceRole, NativeValueDeclarationId};
use super::native_packs::{NativePackAlternative, NativePackSlot};
use crate::types::{ElemFlags, NativeResultPack, SigId, TypeData, TypeId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeIntrinsicKind { Assert, Type, Require, SetMetatable }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeIntrinsicSite {
    pub binding: NativeGlobalBindingId,
    pub declaration: NativeValueDeclarationId,
    pub kind: NativeIntrinsicKind,
}

impl<'p, 's> Checker<'p, 's> {
    /// Every public head must retain the same trusted original value identity.
    /// Signature-shaped values and ambiguous unions acquire no intrinsic effect.
    pub fn native_function_intrinsic(&mut self, function: TypeId) -> Option<NativeIntrinsicSite> {
        if self.is_union(function) {
            let parts = self.parts(function).to_vec();
            let mut selected = None;
            for part in parts {
                let site = self.native_function_intrinsic(part)?;
                if selected.is_some_and(|prior| prior != site) { return None }
                selected = Some(site);
            }
            return selected;
        }
        // A callable table or an uninstantiated parameter constrained by a
        // signature is not evidence of the original runtime function value.
        if self.native_runtime_kind(function) != Some(crate::types::NativeRuntimeKind::Function) { return None }
        let signatures = self.signatures(function, false);
        if signatures.is_empty() { return None }
        let mut selected = None;
        for &signature in signatures.iter() {
            let site = self.native_intrinsic_site(signature)?;
            if selected.is_some_and(|prior| prior != site) { return None }
            selected = Some(site);
        }
        selected
    }

    pub(super) fn native_intrinsic_site(&mut self, signature: SigId) -> Option<NativeIntrinsicSite> {
        let (original, _) = self.sig_native_signature_decl(signature)?;
        let functions = self.native_function_provider?;
        if !functions.contains_signature(original) { return None }
        let site = functions.signature_intrinsic(original)?;
        let globals = self.native_globals_provider?;
        if !globals.contains_global_environment(site.binding.environment) {
            self.report_native_global(NativeGlobalQueryError::InvalidEnvironment(site.binding.environment));
            return None;
        }
        if !globals.contains_global_binding(site.binding) {
            self.report_native_global(NativeGlobalQueryError::InvalidBinding(site.binding));
            return None;
        }
        if !globals.contains_value_declaration(site.declaration)
            || !globals.global_binding_declarations(site.binding).contains(&site.declaration)
            || globals.value_declaration_header(site.declaration).role != NativeGlobalSourceRole::Declaration
        {
            self.report_native_global(NativeGlobalQueryError::InvalidDeclaration(site.declaration));
            return None;
        }
        let contract = self.native_global_binding_type(site.binding).ok()?;
        let signatures = self.signatures(contract, false);
        if !signatures.iter().any(|&head| self.sig_native_signature_decl(head).is_some_and(|(key, _)| key == original)) {
            self.report_native_global(NativeGlobalQueryError::InvalidDeclaration(site.declaration));
            return None;
        }
        Some(site)
    }

    /// A copying operation can use its result context for otherwise unconstrained
    /// arguments. Precise instantiated parameter constraints remain authoritative.
    pub(super) fn native_intrinsic_argument_context(&mut self, signature: SigId, expected_scalar: Option<TypeId>, expected_pack: Option<NativeResultPack>) -> Option<NativeResultPack> {
        let site = self.native_intrinsic_site(signature)?;
        if site.kind != NativeIntrinsicKind::Assert { return None }
        let expected = expected_pack.or_else(|| expected_scalar.map(|ty| self.native_scalar_pack(ty)))?;
        let params = self.sig_params(signature);
        let declared = NativeResultPack { ty: self.rest_type_at_position(&params, 0, false) };
        let declared_rows = self.native_pack_alternatives(declared)?;
        let mut rows = self.native_pack_alternatives(expected)?;
        let fixed = declared_rows.iter().map(|row| row.fixed.len()).max().unwrap_or(0);
        let declared_tail = declared_rows.iter().find_map(|row| row.tail);
        for row in &mut rows {
            let count = row.fixed.len().max(fixed);
            for index in 0..count {
                let parameter = self.native_pack_contextual_slot(declared, index);
                let precise = parameter.filter(|&ty| !self.is_any(ty) && ty != TypeId::UNKNOWN);
                if let Some(slot) = row.fixed.get_mut(index) {
                    if let Some(ty) = precise { slot.ty = ty; }
                } else {
                    row.fixed.push(NativePackSlot { ty: precise.or(row.tail).or(parameter).unwrap_or(TypeId::UNKNOWN), optional: false });
                }
            }
            if row.tail.is_none() { row.tail = declared_tail; }
        }
        Some(self.native_result_pack(&rows))
    }

    /// Original demands are checked under the same selected parameter context.
    /// Unlike generic rest inference, identity returns do not widen resolved values.
    fn native_intrinsic_argument_pack(&mut self, args: &[Arg], context: NativeResultPack) -> Option<NativeResultPack> {
        let mut prefix = Vec::with_capacity(args.len());
        let mut tail = self.native_result_pack(&[NativePackAlternative { fixed: Vec::new(), tail: None }]);
        for (index, &arg) in args.iter().enumerate() {
            match arg {
                Arg::Resolved(ty) => prefix.push(ty),
                Arg::Native(expression) => {
                    let expected = self.native_pack_contextual_slot(context, index);
                    prefix.push(self.check_native_argument(expression, expected, None, CheckMode::empty()));
                }
                Arg::ResolvedPack(pack) if index + 1 == args.len() => { tail = pack; }
                Arg::NativePack(expression) if index + 1 == args.len() => {
                    let expected = self.native_pack_contextual_tail(context, &prefix);
                    tail = self.check_native_argument_pack(expression, expected, None, CheckMode::empty());
                }
                _ => return None,
            }
        }
        Some(self.native_concat_pack(&prefix, tail))
    }

    fn native_assert_pack(&mut self, pack: NativeResultPack) -> Option<NativeResultPack> {
        if let Some(mut rows) = self.native_pack_alternatives(pack) {
            rows.retain_mut(|row| {
                if let Some(first) = row.fixed.first_mut() {
                    first.ty = self.lua_truthiness_type(first.ty, true);
                    first.optional = false;
                    !first.ty.is_never()
                } else if let Some(tail) = row.tail {
                    let first = self.lua_truthiness_type(tail, true);
                    if first.is_never() { return false }
                    row.fixed.push(NativePackSlot { ty: first, optional: false });
                    true
                } else { false }
            });
            return Some(self.native_result_pack(&rows));
        }
        // A symbolic variadic tail can still be retained exactly when a fixed
        // first argument exists. Rewrite that element, keeping its original tail.
        let parts = self.parts(pack.ty).to_vec();
        let mut results = Vec::with_capacity(parts.len());
        for part in parts {
            let TypeData::Tuple { flags, readonly, .. } = self.data(part) else { return None };
            let mut flags = flags.to_vec();
            let readonly = *readonly;
            if flags.first().is_none_or(|flag| !flag.intersects(ElemFlags::REQUIRED | ElemFlags::OPTIONAL)) { return None }
            let mut elements = self.type_arguments(part).to_vec();
            elements[0] = self.lua_truthiness_type(elements[0], true);
            if elements[0].is_never() { continue }
            flags[0].remove(ElemFlags::OPTIONAL);
            flags[0].insert(ElemFlags::REQUIRED);
            results.push(self.normalized_tuple(&elements, &flags, readonly));
        }
        Some(NativeResultPack { ty: self.union(&results) })
    }

    pub(super) fn native_intrinsic_result_pack(&mut self, signature: SigId, args: &[Arg], expected_scalar: Option<TypeId>, expected_pack: Option<NativeResultPack>) -> Result<Option<NativeResultPack>, ResolvedTypeCallError> {
        let Some(site) = self.native_intrinsic_site(signature) else { return Ok(None) };
        if site.kind != NativeIntrinsicKind::Assert { return Ok(None) }
        let params = self.sig_params(signature);
        let context = self.native_intrinsic_argument_context(signature, expected_scalar, expected_pack)
            .unwrap_or_else(|| NativeResultPack { ty: self.rest_type_at_position(&params, 0, false) });
        let pack = self.native_intrinsic_argument_pack(args, context).ok_or(ResolvedTypeCallError::UnsupportedRest)?;
        match site.kind {
            NativeIntrinsicKind::Assert => self.native_assert_pack(pack).map(Some).ok_or(ResolvedTypeCallError::UnsupportedRest),
            NativeIntrinsicKind::Type | NativeIntrinsicKind::Require | NativeIntrinsicKind::SetMetatable => Ok(None),
        }
    }
}
