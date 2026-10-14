use super::{CheckMode, Checker, ResolvedTypeCall, ResolvedTypeCallError};
use super::call::Arg;
use crate::session::ArenaBox;
use crate::types::{ElemFlags, MapperId, NativeResultPack, Shape, SigData, SigId, SigParam, TypeData, TypeId};

#[derive(Copy, Clone, Debug)]
pub struct NativePackSlot {
    pub ty: TypeId,
    pub optional: bool,
}

#[derive(Clone, Debug)]
pub struct NativePackAlternative {
    pub fixed: Vec<NativePackSlot>,
    pub tail: Option<TypeId>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NativePackAdjustmentError {
    TooComplex { count: usize, alternatives: usize },
    Unresolved,
}

#[derive(Copy, Clone, Debug)]
pub enum NativePackResult {
    Declaration(NativeResultPack),
    Checked(NativeResultPack),
}

#[derive(Clone, Debug)]
pub struct NativeResultPackCall {
    pub pack: NativeResultPack,
    pub errors: Vec<ResolvedTypeCallError>,
}

#[derive(Copy, Clone, Debug)]
pub enum NativeCallArgument {
    Resolved(TypeId),
    Expression(crate::types::NativeExpressionId),
    Expand(NativeResultPack),
    ExpandExpression(crate::types::NativeExpressionId),
}

impl<'p, 's> Checker<'p, 's> {
    pub fn call_native_arguments(
        &mut self,
        callee: TypeId,
        call: Option<crate::types::NativeExpressionId>,
        args: &[NativeCallArgument],
        type_args: &[TypeId],
        expected: Option<NativeResultPack>,
    ) -> NativeResultPackCall {
        assert!(call.is_none_or(|call| self.contains_native_expression(call)));
        let args: Vec<Arg> = args.iter().enumerate().map(|(index, &arg)| match arg {
            NativeCallArgument::Resolved(ty) => Arg::Resolved(ty),
            NativeCallArgument::Expression(expression) => {
                assert!(self.contains_native_expression(expression));
                Arg::Native(expression)
            }
            NativeCallArgument::Expand(pack) => {
                assert_eq!(index + 1, args.len(), "only the final native argument expands");
                Arg::ResolvedPack(pack)
            }
            NativeCallArgument::ExpandExpression(expression) => {
                assert_eq!(index + 1, args.len(), "only the final native argument expands");
                assert!(self.contains_native_expression(expression));
                Arg::NativePack(expression)
            }
        }).collect();
        let resolved = self.resolve_native_arguments_with_result_pack(callee, &args, type_args, None, expected);
        self.result_pack_of_native_call(resolved)
    }

    pub fn call_resolved_result_pack(
        &mut self,
        callee: TypeId,
        args: &[TypeId],
        type_args: &[TypeId],
        expected: Option<NativeResultPack>,
    ) -> NativeResultPackCall {
        let args: Vec<Arg> = args.iter().copied().map(Arg::Resolved).collect();
        let resolved = self.resolve_native_arguments_with_result_pack(callee, &args, type_args, None, expected);
        self.result_pack_of_native_call(resolved)
    }

    pub fn call_expression_result_pack(
        &mut self,
        callee: TypeId,
        call: crate::types::NativeExpressionId,
        args: &[crate::types::NativeExpressionId],
        type_args: &[TypeId],
        expected: Option<NativeResultPack>,
    ) -> NativeResultPackCall {
        assert!(self.contains_native_expression(call));
        assert!(args.iter().all(|&arg| self.contains_native_expression(arg)));
        let args: Vec<Arg> = args.iter().copied().map(Arg::Native).collect();
        let resolved = self.resolve_native_arguments_with_result_pack(callee, &args, type_args, None, expected);
        self.result_pack_of_native_call(resolved)
    }

    fn result_pack_of_native_call(&mut self, resolved: ResolvedTypeCall) -> NativeResultPackCall {
        let pack = match resolved.sig {
            Some(sig) => self.sig_result_pack(sig),
            None if self.is_any(resolved.ret) => NativeResultPack { ty: resolved.ret },
            None => self.native_scalar_pack(resolved.ret),
        };
        NativeResultPackCall { pack, errors: resolved.errors }
    }

    pub fn sig_has_native_result_pack(&self, sig: SigId) -> bool {
        match self.types().sig(sig) {
            SigData::WithNativeResultPack { .. } => true,
            SigData::Native { declaration, .. }
            | SigData::NativeCaptured { declaration, .. }
            | SigData::NativeContextual { declaration, .. } => self.native_function_provider
                .is_some_and(|provider| provider.has_result_pack(*declaration)),
            SigData::WithReturn { sig, .. } => self.sig_has_native_result_pack(*sig),
            SigData::Synth { of, .. } => of.len() > 1 && of.iter().all(|&sig| self.sig_has_native_result_pack(sig)),
            _ => false,
        }
    }

    pub fn sig_result_pack(&mut self, sig: SigId) -> NativeResultPack {
        if let SigData::WithReturn { sig: inner, ret } = *self.types().sig(sig)
            && ret != TypeId::UNRESOLVED && self.sig_has_native_result_pack(inner)
        {
            let pack = self.sig_result_pack(inner);
            return self.native_pack_with_first_result(pack, ret);
        }
        if self.sig_has_native_result_pack(sig) {
            NativeResultPack { ty: self.signature_return_type(sig) }
        } else {
            let scalar = self.sig_return(sig);
            self.native_scalar_pack(scalar)
        }
    }

    pub fn native_pack_function(&mut self, params: &[SigParam], type_params: &[TypeId], pack: NativeResultPack) -> TypeId {
        let ret = self.native_pack_slot(pack, 0);
        let sig = self.types().intern_sig(SigData::Synth {
            type_params: self.list(type_params), params: self.list(params), ret, this: None,
            of: ArenaBox::empty(), is_union: false,
        });
        let sig = self.types().intern_sig(SigData::WithNativeResultPack { sig, pack });
        // A materialized foreign function type can mention free type parameters despite having
        // no TS declaration symbol, just like the ordinary owned function-type constructor.
        let mut shape = Shape::new_in(self.arena);
        shape.call.push(sig);
        self.synth(shape)
    }

    pub fn contextual_rest_pack(&mut self, context: crate::types::NativeFunctionContext, fixed_count: usize) -> NativeResultPack {
        let params = self.sig_params(context.expected);
        NativeResultPack { ty: self.rest_type_at_position(&params, fixed_count, false) }
    }

    pub fn native_contextual_return_pack(&mut self, context: crate::types::NativeFunctionContext) -> NativeResultPack {
        let returned = self.sig_result_pack(context.expected);
        self.get_native_inference_context(context.expression)
            .and_then(|level| self.with_inference_context(level, |c, n| {
                let mapper = c.non_fixing_mapper(n, returned.ty);
                c.instantiate_native_pack(returned, mapper)
            }))
            .unwrap_or(returned)
    }

    pub fn native_capture_rest_pack(&mut self, context: crate::types::NativeCapturedFunctionContext) -> Option<NativeResultPack> {
        self.signature_rest_pack(context.signature)
    }

    pub fn instantiate_native_pack_in_capture(&mut self, context: crate::types::NativeCapturedFunctionContext, pack: NativeResultPack) -> NativeResultPack {
        NativeResultPack { ty: self.instantiate_in_native_capture(context, pack.ty) }
    }

    pub fn signature_rest_pack(&mut self, sig: SigId) -> Option<NativeResultPack> {
        let params = self.sig_params(sig);
        let parameter = params.last().filter(|parameter| parameter.rest)?;
        Some(NativeResultPack { ty: self.get_type_of_parameter(parameter) })
    }

    fn native_pack_with_first_result(&mut self, pack: NativeResultPack, ty: TypeId) -> NativeResultPack {
        let Some(mut alternatives) = self.native_pack_alternatives(pack) else { return pack };
        let mut expanded = Vec::new();
        for alternative in &mut alternatives {
            if let Some(first) = alternative.fixed.first_mut() {
                first.ty = ty;
            } else if alternative.tail.is_some() {
                expanded.push(NativePackAlternative {
                    fixed: vec![NativePackSlot { ty, optional: false }], tail: alternative.tail,
                });
                alternative.tail = None;
            }
        }
        alternatives.extend(expanded);
        self.native_result_pack(&alternatives)
    }

    pub fn native_scalar_pack(&mut self, ty: TypeId) -> NativeResultPack {
        self.native_result_pack(&[NativePackAlternative {
            fixed: vec![NativePackSlot { ty, optional: false }], tail: None,
        }])
    }

    /// An empty alternative list is no reachable result; one empty row is zero results.
    pub fn native_result_pack(&mut self, alternatives: &[NativePackAlternative]) -> NativeResultPack {
        assert!(self.native_arrays, "native packs require intrinsic array mode");
        let mut rows = Vec::with_capacity(alternatives.len());
        for alternative in alternatives {
            let mut elems: Vec<TypeId> = alternative.fixed.iter().map(|slot| slot.ty).collect();
            let mut flags: Vec<ElemFlags> = alternative.fixed.iter().map(|slot| {
                if slot.optional { ElemFlags::OPTIONAL } else { ElemFlags::REQUIRED }
            }).collect();
            if let Some(tail) = alternative.tail {
                elems.push(tail);
                flags.push(ElemFlags::REST);
            }
            rows.push(self.tuple(&elems, &flags, false));
        }
        NativeResultPack { ty: self.union(&rows) }
    }

    pub fn native_pack_type(&self, pack: NativeResultPack) -> TypeId {
        pack.ty
    }

    pub fn native_pack_count_bounds(&mut self, pack: NativeResultPack) -> (usize, Option<usize>) {
        let Some(alternatives) = self.native_pack_alternatives(pack) else { return (0, None) };
        let minimum = alternatives.iter().map(|alternative| alternative.fixed.iter()
            .filter(|slot| !slot.optional).count()).min().unwrap_or(0);
        let maximum = alternatives.iter().try_fold(0, |maximum, alternative| {
            alternative.tail.is_none().then_some(maximum.max(alternative.fixed.len()))
        });
        (minimum, maximum)
    }

    /// The existing tuple relation checks physical required/optional/rest lengths after values
    /// have been erased. A still-symbolic generic rest is rechecked by `chooseOverload` after
    /// signature instantiation, as in TS.
    pub(super) fn native_argument_pack_has_correct_arity(&mut self, args: &[Arg], params: &[SigParam]) -> bool {
        let expected = NativeResultPack { ty: self.rest_type_at_position(params, 0, false) };
        let Some(mut expected_rows) = self.native_pack_alternatives(expected) else { return true };
        for row in &mut expected_rows {
            for slot in &mut row.fixed { slot.ty = TypeId::ANY }
            if row.tail.is_some() { row.tail = Some(TypeId::ANY) }
        }
        let mut actual_rows = if let Some(&arg) = args.last().filter(|arg| arg.is_spread()) {
            let pack = match arg {
                Arg::ResolvedPack(pack) => pack,
                Arg::NativePack(expression) => self.check_native_argument_pack(expression, None, None, CheckMode::empty()),
                _ => unreachable!("TS spread in native call arity"),
            };
            if self.is_any(pack.ty) { return true }
            let Some(rows) = self.native_pack_alternatives(pack) else { return true };
            rows
        } else { vec![NativePackAlternative { fixed: Vec::new(), tail: None }] };
        let prefix = args.len() - usize::from(args.last().is_some_and(|arg| arg.is_spread()));
        for row in &mut actual_rows {
            let mut fixed = vec![NativePackSlot { ty: TypeId::ANY, optional: false }; prefix];
            fixed.extend(row.fixed.iter().map(|slot| NativePackSlot { ty: TypeId::ANY, optional: slot.optional }));
            row.fixed = fixed;
            if row.tail.is_some() { row.tail = Some(TypeId::ANY) }
        }
        let actual = self.native_result_pack(&actual_rows);
        let expected = self.native_result_pack(&expected_rows);
        self.native_pack_assignable(actual, expected)
    }

    /// Descriptors are unavailable while a symbolic variadic pack has not been instantiated.
    pub fn native_pack_alternatives(&mut self, pack: NativeResultPack) -> Option<Vec<NativePackAlternative>> {
        if pack.ty.is_never() {
            return Some(Vec::new());
        }
        let parts = self.parts(pack.ty).to_vec();
        let mut alternatives = Vec::with_capacity(parts.len());
        for part in parts {
            if let Some(element) = self.array_element(part) {
                alternatives.push(NativePackAlternative { fixed: Vec::new(), tail: Some(element) });
                continue;
            }
            let TypeData::Tuple { flags, .. } = self.data(part) else { return None };
            if flags.iter().any(|flag| flag.contains(ElemFlags::VARIADIC)) {
                return None;
            }
            let elems = self.type_arguments(part);
            let fixed = Self::fixed_length(flags);
            if flags.len() > fixed + 1 {
                return None;
            }
            alternatives.push(NativePackAlternative {
                fixed: (0..fixed).map(|index| NativePackSlot {
                    ty: elems[index], optional: flags[index].contains(ElemFlags::OPTIONAL),
                }).collect(),
                tail: (fixed < flags.len()).then(|| elems[fixed]),
            });
        }
        Some(alternatives)
    }

    /// Lua scalar adjustment: an absent slot is nil, including the first slot of zero results.
    pub fn native_pack_slot(&mut self, pack: NativeResultPack, index: usize) -> TypeId {
        if pack.ty.is_never() || self.is_any(pack.ty) {
            return pack.ty;
        }
        let parts = self.parts(pack.ty).to_vec();
        let mut slots = Vec::with_capacity(parts.len());
        for part in parts {
            let slot = if let Some(element) = self.array_element(part) {
                self.union(&[element, TypeId::UNDEFINED])
            } else if let TypeData::Tuple { flags, .. } = self.data(part) {
                let elems = self.type_arguments(part);
                let fixed = Self::fixed_length(flags);
                if index < fixed {
                    let ty = elems[index];
                    if flags[index].contains(ElemFlags::OPTIONAL) {
                        self.union(&[ty, TypeId::UNDEFINED])
                    } else { ty }
                } else if fixed < flags.len() {
                    let ty = self.rest_element_type(part, index);
                    self.union(&[ty, TypeId::UNDEFINED])
                } else { TypeId::UNDEFINED }
            } else { TypeId::ERROR };
            slots.push(slot);
        }
        self.union(&slots)
    }

    /// Context describes existing expected positions. It does not invent nil for an unannotated
    /// destination after the end of a fixed contract.
    pub fn native_pack_contextual_slot(&mut self, pack: NativeResultPack, index: usize) -> Option<TypeId> {
        if self.is_any(pack.ty) { return Some(pack.ty) }
        let parts = self.parts(pack.ty).to_vec();
        let mut slots = Vec::new();
        for part in parts {
            if let Some(element) = self.array_element(part) {
                slots.push(element);
            } else if let TypeData::Tuple { flags, .. } = self.data(part) {
                let fixed = Self::fixed_length(flags);
                if index < fixed {
                    let ty = self.type_arguments(part)[index];
                    slots.push(if flags[index].contains(ElemFlags::OPTIONAL) { self.optional(ty) } else { ty });
                } else if fixed < flags.len() {
                    slots.push(self.rest_element_type(part, index));
                }
            }
        }
        (!slots.is_empty()).then(|| self.union(&slots))
    }

    /// Preserve tail-length correlation while padding or truncating an assignment result list.
    pub fn native_adjust_pack(&mut self, pack: NativeResultPack, count: usize) -> Result<NativeResultPack, NativePackAdjustmentError> {
        let Some(alternatives) = self.native_pack_alternatives(pack) else {
            if self.is_any(pack.ty) {
                if count >= 10_000 {
                    return Err(NativePackAdjustmentError::TooComplex { count, alternatives: 1 });
                }
                return Ok(self.native_result_pack(&[NativePackAlternative {
                    fixed: vec![NativePackSlot { ty: pack.ty, optional: false }; count], tail: None,
                }]));
            }
            return Err(NativePackAdjustmentError::Unresolved);
        };
        let rows = alternatives.iter().try_fold(0usize, |total, alternative| {
            let minimum = alternative.fixed.iter().position(|slot| slot.optional)
                .unwrap_or(alternative.fixed.len()).min(count);
            let maximum = if alternative.tail.is_some() { count } else { alternative.fixed.len().min(count) };
            total.checked_add((maximum.max(minimum) - minimum).checked_add(1)?)
        });
        if count >= 10_000 || rows.and_then(|rows| rows.checked_mul(count.max(1))).is_none_or(|slots| slots >= 100_000) {
            return Err(NativePackAdjustmentError::TooComplex { count, alternatives: rows.unwrap_or(usize::MAX) });
        }
        let mut rows = Vec::new();
        for alternative in alternatives {
            let minimum = alternative.fixed.iter().position(|slot| slot.optional)
                .unwrap_or(alternative.fixed.len());
            let maximum = if alternative.tail.is_some() { count } else { alternative.fixed.len().min(count) };
            let minimum = minimum.min(count);
            for present in minimum..=maximum.max(minimum) {
                let fixed = (0..count).map(|index| NativePackSlot {
                    ty: if index >= present { TypeId::UNDEFINED }
                        else { alternative.fixed.get(index).map(|slot| slot.ty).or(alternative.tail).unwrap_or(TypeId::UNDEFINED) },
                    optional: false,
                }).collect();
                rows.push(NativePackAlternative { fixed, tail: None });
            }
        }
        Ok(self.native_result_pack(&rows))
    }

    /// Prefix items have already been scalarized; only the final pack expands.
    pub fn native_concat_pack(&mut self, prefix: &[TypeId], tail: NativeResultPack) -> NativeResultPack {
        if prefix.is_empty() {
            return tail;
        }
        let mut elems = prefix.to_vec();
        let mut flags = vec![ElemFlags::REQUIRED; elems.len()];
        elems.push(tail.ty);
        flags.push(ElemFlags::VARIADIC);
        NativeResultPack { ty: self.normalized_tuple(&elems, &flags, false) }
    }

    pub fn native_join_packs(&mut self, packs: &[NativeResultPack]) -> NativeResultPack {
        let types: Vec<TypeId> = packs.iter().map(|pack| pack.ty).collect();
        NativeResultPack { ty: self.union(&types) }
    }

    /// A whole row must permit falling through; projecting nullable first slots loses that
    /// distinction. The single-result policy is the same as TS's explicit-return check.
    pub fn native_pack_allows_implicit_return(&mut self, pack: NativeResultPack) -> bool {
        if self.is_any(pack.ty) { return true }
        let Some(alternatives) = self.native_pack_alternatives(pack) else { return false };
        alternatives.iter().any(|alternative| {
            alternative.fixed.iter().all(|slot| slot.optional)
                || (alternative.fixed.len() == 1 && alternative.tail.is_none()
                    && self.allows_implicit_return_without_explicit_return(alternative.fixed[0].ty))
        })
    }

    /// Preserve actual zero-result fallthrough separately from a written scalar nil contract.
    pub fn native_pack_with_implicit_empty(&mut self, pack: NativeResultPack) -> NativeResultPack {
        if self.is_any(pack.ty) || !self.native_pack_allows_implicit_return(pack) { return pack }
        let empty = self.native_result_pack(&[NativePackAlternative { fixed: Vec::new(), tail: None }]);
        self.native_join_packs(&[pack, empty])
    }

    /// Context for the final expression after already checked scalar prefix values.
    pub fn native_pack_contextual_tail(&mut self, expected: NativeResultPack, prefix: &[TypeId]) -> Option<NativeResultPack> {
        let parts = self.parts(expected.ty).to_vec();
        let mut remaining = Vec::new();
        for part in parts {
            let mut matches = true;
            for (index, &actual) in prefix.iter().enumerate() {
                let target = if let Some(element) = self.array_element(part) {
                    Some(element)
                } else if let TypeData::Tuple { flags, .. } = self.data(part) {
                    let fixed = Self::fixed_length(flags);
                    (index < fixed || fixed < flags.len()).then(|| self.rest_element_type(part, index))
                } else { None };
                if !target.is_some_and(|target| self.is_assignable(actual, target)) {
                    matches = false;
                    break;
                }
            }
            if !matches { continue }
            let tail = if self.is_array(part) {
                part
            } else if let TypeData::Tuple { flags, .. } = self.data(part) {
                let elems = self.type_arguments(part);
                self.slice_tuple(elems, flags, prefix.len(), 0)
            } else { continue };
            remaining.push(tail);
        }
        (!remaining.is_empty()).then(|| NativeResultPack { ty: self.union(&remaining) })
    }

    /// Widen returned values with the existing literal policy, preserving each complete row.
    pub fn native_aggregate_result_packs(
        &mut self,
        packs: &[NativeResultPack],
        has_fallthrough: bool,
        context: Option<NativeResultPack>,
    ) -> NativeResultPack {
        let joined = self.native_join_packs(packs);
        let Some(mut alternatives) = self.native_pack_alternatives(joined) else { return joined };
        if has_fallthrough {
            alternatives.push(NativePackAlternative { fixed: Vec::new(), tail: None });
        }
        let contextual = context.and_then(|pack| self.native_pack_alternatives(pack));
        for alternative in &mut alternatives {
            let mut matching = Vec::new();
            if let Some(contextual) = &contextual {
                for expected in contextual {
                    if alternative.fixed.len() != expected.fixed.len() && expected.tail.is_none() {
                        continue;
                    }
                    let matches = alternative.fixed.iter().enumerate().all(|(index, slot)| {
                        if !self.is_fresh_literal(slot.ty) { return true }
                        let target = expected.fixed.get(index).map(|slot| slot.ty).or(expected.tail);
                        target.is_some_and(|target| self.is_assignable(slot.ty, target))
                    });
                    if matches { matching.push(expected) }
                }
            }
            for (index, slot) in alternative.fixed.iter_mut().enumerate() {
                let expected: Vec<TypeId> = matching.iter()
                    .filter_map(|alternative| alternative.fixed.get(index).map(|slot| slot.ty).or(alternative.tail))
                    .collect();
                let expected = (!expected.is_empty()).then(|| self.union(&expected));
                let ty = self.widen_literal_for_context(slot.ty, expected);
                slot.ty = self.get_widened_type(ty);
            }
            if let Some(tail) = alternative.tail {
                let expected: Vec<TypeId> = matching.iter().filter_map(|alternative| alternative.tail).collect();
                let expected = (!expected.is_empty()).then(|| self.union(&expected));
                let ty = self.widen_literal_for_context(tail, expected);
                alternative.tail = Some(self.get_widened_type(ty));
            }
        }
        self.native_result_pack(&alternatives)
    }

    /// Written return contracts keep required lengths and whole alternative correlations.
    pub fn native_pack_assignable(&mut self, actual: NativeResultPack, declared: NativeResultPack) -> bool {
        self.is_assignable(actual.ty, declared.ty)
    }

    pub fn instantiate_native_pack(&mut self, pack: NativeResultPack, mapper: MapperId) -> NativeResultPack {
        NativeResultPack { ty: self.instantiate(pack.ty, mapper) }
    }
}
