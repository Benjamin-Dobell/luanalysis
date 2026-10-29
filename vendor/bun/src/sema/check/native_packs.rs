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
    pub sig: Option<SigId>,
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
        let args = self.native_call_arguments(call, args);
        let resolved = self.resolve_native_arguments_with_result_pack(callee, &args, type_args, None, expected);
        self.result_pack_of_native_call(resolved)
    }

    pub fn call_native_arguments_scalar(
        &mut self,
        callee: TypeId,
        call: Option<crate::types::NativeExpressionId>,
        args: &[NativeCallArgument],
        type_args: &[TypeId],
        expected: Option<TypeId>,
    ) -> ResolvedTypeCall {
        let args = self.native_call_arguments(call, args);
        self.resolve_native_arguments_with_result_pack(callee, &args, type_args, expected, None)
    }

    fn native_call_arguments(&self, call: Option<crate::types::NativeExpressionId>, args: &[NativeCallArgument]) -> Vec<Arg> {
        assert!(call.is_none_or(|call| self.contains_native_expression(call)));
        args.iter().enumerate().map(|(index, &arg)| match arg {
            NativeCallArgument::Resolved(ty) => Arg::Resolved(ty),
            NativeCallArgument::Expression(expression) => {
                assert!(self.contains_native_expression(expression));
                Arg::Native(self.native_argument_expression(expression))
            }
            NativeCallArgument::Expand(pack) => {
                assert_eq!(index + 1, args.len(), "only the final native argument expands");
                Arg::ResolvedPack(pack)
            }
            NativeCallArgument::ExpandExpression(expression) => {
                assert_eq!(index + 1, args.len(), "only the final native argument expands");
                assert!(self.contains_native_expression(expression));
                Arg::NativePack(self.native_argument_expression(expression))
            }
        }).collect()
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

    pub(super) fn call_native_iterator_result_pack(&mut self, callee: TypeId, state: TypeId, control: TypeId) -> NativeResultPackCall {
        let args = [Arg::Resolved(state), Arg::Resolved(control)];
        let resolved = self.resolve_native_iterator_arguments(callee, &args);
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
        let args: Vec<Arg> = args.iter().copied().map(|expression| Arg::Native(self.native_argument_expression(expression))).collect();
        let resolved = self.resolve_native_arguments_with_result_pack(callee, &args, type_args, None, expected);
        self.result_pack_of_native_call(resolved)
    }

    fn result_pack_of_native_call(&mut self, resolved: ResolvedTypeCall) -> NativeResultPackCall {
        let pack = match resolved.sig {
            Some(sig) => self.sig_result_pack(sig),
            None if self.is_any(resolved.ret) => NativeResultPack { ty: resolved.ret },
            None => self.native_scalar_pack(resolved.ret),
        };
        NativeResultPackCall { pack, sig: resolved.sig, errors: resolved.errors }
    }

    pub fn sig_has_native_result_pack(&self, sig: SigId) -> bool {
        match self.types().sig(sig) {
            SigData::NativeDeclarationSignature { .. } => true,
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

    /// The sequence that varargs written `...: T...` are: a type parameter's whole sequence, as a
    /// result list's final type parameter is, or a tuple's elements (TypeScript's `...args: T`).
    pub fn native_spread_pack(&mut self, ty: TypeId) -> NativeResultPack {
        if matches!(self.data(ty), TypeData::NativeTypeParam { .. }) {
            return NativeResultPack { ty: self.intern(TypeData::NativePackParameter(ty)) };
        }
        let parts = self.parts(ty).to_vec();
        let rows: Option<Vec<_>> = parts.iter().map(|&part| self.native_tuple_rows(part)).collect();
        match rows {
            Some(rows) => self.native_result_pack(&rows.concat()),
            None => NativeResultPack { ty },
        }
    }

    /// A Lua tuple, `[A, B?]`, as result rows: a table whose only members are the fields `1..n`,
    /// of which only the last may be optional. An optional element may be absent, so each row
    /// without it is one too, as a result list's `T?` is: the rows hold values, never nil for it.
    fn native_tuple_rows(&mut self, ty: TypeId) -> Option<Vec<NativePackAlternative>> {
        let table = self.native_object_schema_resolved(ty).unwrap_or(ty);
        let TypeData::NativeTable(data) = self.data(table) else { return None };
        if data.is_array || !data.indexers.is_empty() || data.fields.is_empty() { return None }
        let written: Vec<_> = data.fields.iter().map(|field| (field.key, field.value, field.optional)).collect();
        let mut fields: Vec<(i64, NativePackSlot)> = Vec::with_capacity(written.len());
        for (key, value, optional) in written {
            let super::native_tables::NativeTableKey::Integer(index) = key else { return None };
            // An optional element's value is no nil: it is absent instead.
            let ty = if optional { self.remove_missing_or_undefined_type(value) } else { value };
            fields.push((index, NativePackSlot { ty, optional }));
        }
        fields.sort_by_key(|(index, _)| *index);
        let optional: Vec<_> = fields.iter().map(|(_, slot)| slot.optional).collect();
        if fields.iter().enumerate().any(|(position, (index, _))| *index != position as i64 + 1) { return None }
        let fixed: Vec<_> = fields.into_iter().map(|(_, slot)| NativePackSlot { optional: false, ..slot }).collect();
        let required = optional.iter().position(|&optional| optional).unwrap_or(fixed.len());
        if optional[required..].iter().any(|&optional| !optional) { return None }
        Some((required..=fixed.len()).map(|count| NativePackAlternative { fixed: fixed[..count].to_vec(), tail: None }).collect())
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
    pub(super) fn native_argument_pack_has_correct_arity(&mut self, args: &[Arg], params: &[SigParam], mode: CheckMode) -> bool {
        let expected = NativeResultPack { ty: self.rest_type_at_position(params, 0, false) };
        let Some(mut expected_rows) = self.native_pack_alternatives(expected) else { return true };
        for row in &mut expected_rows {
            for slot in &mut row.fixed { slot.ty = TypeId::ANY }
            if row.tail.is_some() { row.tail = Some(TypeId::ANY) }
        }
        let mut actual_rows = if let Some(&arg) = args.last().filter(|arg| arg.is_spread()) {
            let pack = match arg {
                Arg::ResolvedPack(pack) => pack,
                Arg::NativePack(expression) => self.check_native_argument_pack(expression, None, None, mode),
                _ => unreachable!("TS spread in native call arity"),
            };
            if self.is_any(pack.ty) {
                let prefix = args.len() - 1;
                let (_, maximum) = self.native_pack_count_bounds(expected);
                return maximum.is_none_or(|maximum| prefix <= maximum);
            }
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
        self.native_result_list_assignable(actual, expected)
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

    /// The domain of values physically present in a result list. Absence is
    /// represented by the pack's cardinality, never by inventing a nil element.
    pub(super) fn native_pack_element_domain(&mut self, pack: NativeResultPack) -> TypeId {
        if self.is_any(pack.ty) { return pack.ty; }
        let Some(rows) = self.native_pack_alternatives(pack) else {
            return self.intern(TypeData::NativePackElement(pack.ty));
        };
        let mut values = Vec::new();
        for row in rows {
            values.extend(row.fixed.into_iter().map(|slot| slot.ty));
            values.extend(row.tail);
        }
        self.union(&values)
    }

    pub(super) fn native_pack_parameter_first_is_optional(&mut self, parameter: TypeId) -> bool {
        let scalar = self.base_constraint(parameter);
        self.native_contains_nil(scalar)
            || self.allows_implicit_return_without_explicit_return(scalar)
    }

    /// Physical arity is independent of permissive nil value assignments. An
    /// unreachable sequence vacuously meets the first-result requirement.
    pub(super) fn native_pack_has_required_first(&mut self, pack: NativeResultPack) -> bool {
        if pack.ty.is_never() { return true; }
        let bound = self.native_pack_covariant_bound(pack.ty);
        self.native_pack_count_bounds(NativeResultPack { ty: bound }).0 != 0
    }

    /// A sound covariant envelope for an opaque physical sequence. Keep this
    /// separate from the sequence used by inference and published call results:
    /// the envelope forgets later-slot correlation and is only a relation proof.
    fn native_pack_covariant_bound(&mut self, ty: TypeId) -> TypeId {
        match self.data(ty) {
            TypeData::NativePackParameter(parameter) => {
                let parameter = *parameter;
                let optional = self.native_pack_parameter_first_is_optional(parameter);
                let element = self.native_pack_element_domain(NativeResultPack { ty });
                let mut rows = vec![NativePackAlternative {
                    fixed: vec![NativePackSlot { ty: parameter, optional: false }],
                    tail: Some(element),
                }];
                if optional {
                    // Zero results have no physical nil element. An optional
                    // tuple slot would introduce a missing-value marker when
                    // this sequence starts inside a homogeneous target tail.
                    rows.push(NativePackAlternative { fixed: Vec::new(), tail: None });
                }
                self.native_result_pack(&rows).ty
            }
            TypeData::Tuple { flags, readonly, .. } => {
                let flags = flags.to_vec();
                let readonly = *readonly;
                let original = self.type_arguments(ty).to_vec();
                let mut elems = original.clone();
                for (element, flag) in elems.iter_mut().zip(&flags) {
                    if flag.contains(ElemFlags::VARIADIC) {
                        *element = self.native_pack_covariant_bound(*element);
                    }
                }
                if elems == original { ty } else { self.normalized_tuple(&elems, &flags, readonly) }
            }
            TypeData::Union(parts) => {
                let parts = parts.to_vec();
                let bounded: Vec<_> = parts.iter().map(|&part| self.native_pack_covariant_bound(part)).collect();
                if bounded == parts { ty } else { self.union(&bounded) }
            }
            _ => ty,
        }
    }

    /// Lua scalar adjustment: an absent slot is nil, including the first slot of zero results.
    pub fn native_pack_slot(&mut self, pack: NativeResultPack, index: usize) -> TypeId {
        if pack.ty.is_never() || self.is_any(pack.ty) {
            return pack.ty;
        }
        let parts = self.parts(pack.ty).to_vec();
        let mut slots = Vec::with_capacity(parts.len());
        for part in parts {
            let slot = if let TypeData::NativePackParameter(parameter) = *self.data(part) {
                if index == 0 { parameter } else {
                    let index = self.number_literal(index as f64, false);
                    self.intern(TypeData::IndexedAccess { obj: part, index, undefined: true })
                }
            } else if let Some(element) = self.array_element(part) {
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
                    let rest = elems[fixed];
                    if flags[fixed].contains(ElemFlags::VARIADIC)
                        && matches!(self.data(rest), TypeData::NativePackParameter(_))
                    {
                        self.native_pack_slot(NativeResultPack { ty: rest }, index - fixed)
                    } else {
                        let ty = self.rest_element_type(part, index);
                        self.union(&[ty, TypeId::UNDEFINED])
                    }
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
            if self.any_type_in(pack.ty, false, |ty| matches!(self.data(ty), TypeData::NativePackParameter(_))) {
                if count >= 10_000 {
                    return Err(NativePackAdjustmentError::TooComplex { count, alternatives: 1 });
                }
                let fixed = (0..count).map(|index| NativePackSlot {
                    ty: self.native_pack_slot(pack, index), optional: false,
                }).collect();
                return Ok(self.native_result_pack(&[NativePackAlternative { fixed, tail: None }]));
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
                    ty: if index >= present {
                        if index >= alternative.fixed.len() && alternative.tail.is_some_and(|tail| self.is_any(tail)) {
                            alternative.tail.unwrap()
                        } else { TypeId::UNDEFINED }
                    }
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

    /// In a written Lua scalar return contract, void is absence, whereas nil is one value.
    /// This normalization happens before generic substitution and never rewrites actual values.
    pub fn native_declared_result_pack(&mut self, pack: NativeResultPack) -> NativeResultPack {
        let Some(alternatives) = self.native_pack_alternatives(pack) else { return pack };
        let mut rows = Vec::new();
        for alternative in alternatives {
            if alternative.fixed.len() == 1 && alternative.tail.is_none() {
                let slot = alternative.fixed[0];
                let parts = self.parts(slot.ty);
                if parts.contains(&TypeId::VOID) {
                    let values: Vec<TypeId> = parts.iter().copied().filter(|&part| part != TypeId::VOID).collect();
                    rows.push(NativePackAlternative { fixed: Vec::new(), tail: None });
                    if !values.is_empty() {
                        let ty = self.union(&values);
                        rows.push(NativePackAlternative { fixed: vec![NativePackSlot { ty, ..slot }], tail: None });
                    }
                    continue;
                }
            }
            rows.push(alternative);
        }
        let mut expanded = Vec::with_capacity(rows.len());
        for mut row in rows {
            if let Some(parameter) = row.tail
                && self.native_type_parameter_is_result_pack(parameter)
            {
                // A producer's first scalar retains T. Repeating its result
                // sequence can place any of that sequence's physical values in
                // later slots, while every repeated slot may be absent.
                let facet = self.intern(TypeData::NativePackParameter(parameter));
                let element = self.native_pack_element_domain(NativeResultPack { ty: facet });
                let domain = self.union(&[parameter, element]);
                row.fixed.push(NativePackSlot { ty: parameter, optional: true });
                row.tail = Some(domain);
                expanded.push(self.native_result_pack(&[row]));
            } else if row.tail.is_none()
                && let Some(last) = row.fixed.last()
                && !last.optional
                && (self.native_type_parameter_is_result_pack(last.ty) || matches!(self.data(last.ty), TypeData::NativePackParameter(_)))
            {
                // A final result that is a whole sequence, a result pack parameter or its
                // sequence, is each of its values.
                let last = row.fixed.pop().unwrap().ty;
                let prefix: Vec<_> = row.fixed.iter().map(|slot| slot.ty).collect();
                let tail = if matches!(self.data(last), TypeData::NativePackParameter(_)) { last }
                    else { self.intern(TypeData::NativePackParameter(last)) };
                expanded.push(self.native_concat_pack(&prefix, NativeResultPack { ty: tail }));
            } else {
                expanded.push(self.native_result_pack(&[row]));
            }
        }
        self.native_join_packs(&expanded)
    }

    /// A single required physical row cannot be refined by inspecting an implementation.
    /// Apply the same void/implicit-empty policy as return refinement before deciding this;
    /// a nullable scalar, optional slot or variadic tail still needs its body cardinality.
    pub fn native_return_pack_has_fixed_cardinality(&mut self, pack: NativeResultPack) -> bool {
        let pack = self.native_declared_result_pack(pack);
        let pack = self.native_pack_with_implicit_empty(pack);
        let Some(rows) = self.native_pack_alternatives(pack) else { return false };
        matches!(rows.as_slice(), [row] if row.tail.is_none() && row.fixed.iter().all(|slot| !slot.optional))
    }

    /// Keep the declared value types while refining only physical rows established by the body.
    /// Invalid or still-symbolic body rows retain the contract for diagnostic recovery.
    pub fn native_return_pack_with_cardinality(&mut self, declared: NativeResultPack, actual: NativeResultPack) -> NativeResultPack {
        let declared = self.native_declared_result_pack(declared);
        let allowed = self.native_pack_with_implicit_empty(declared);
        let Some(actual_rows) = self.native_pack_alternatives(actual) else { return allowed };
        let Some(expected_rows) = self.native_pack_alternatives(allowed) else { return allowed };
        let mut rows = Vec::new();
        for actual in actual_rows {
            let actual_count = self.native_pack_row_count_type(&actual);
            let mut matched = false;
            for expected in &expected_rows {
                let expected_count = self.native_pack_row_count_type(expected);
                if !self.native_pack_assignable(actual_count, expected_count) { continue }
                // A written variadic tail is the contract callers see, however many values the
                // body happens to return.
                if expected.tail.is_some() {
                    rows.push(expected.clone());
                    matched = true;
                    continue;
                }
                let mut fixed = Vec::with_capacity(actual.fixed.len());
                for (index, actual_slot) in actual.fixed.iter().enumerate() {
                    let ty = match expected.fixed.get(index) {
                        Some(slot) if slot.optional => self.optional(slot.ty),
                        Some(slot) => slot.ty,
                        None => expected.tail.expect("count-compatible return pack lacks a value slot"),
                    };
                    fixed.push(NativePackSlot { ty, optional: actual_slot.optional });
                }
                rows.push(NativePackAlternative { fixed, tail: actual.tail.and(expected.tail) });
                matched = true;
            }
            if !matched { return allowed }
        }
        self.native_result_pack(&rows)
    }

    fn native_pack_row_count_type(&mut self, row: &NativePackAlternative) -> NativeResultPack {
        self.native_result_pack(&[NativePackAlternative {
            fixed: row.fixed.iter().map(|slot| NativePackSlot { ty: TypeId::ANY, optional: slot.optional }).collect(),
            tail: row.tail.map(|_| TypeId::ANY),
        }])
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
    /// `actual` as a result list delivered to `declared`'s fixed slots: Lua drops values past
    /// them, so a variadic tail that reaches past every declared slot is no excess. Within the
    /// declared slots it stands for each count of values it may supply, none included.
    pub fn native_pack_without_possible_excess(&mut self, actual: NativeResultPack, declared: NativeResultPack) -> NativeResultPack {
        let Some(declared_rows) = self.native_pack_alternatives(declared) else { return actual };
        if declared_rows.is_empty() || declared_rows.iter().any(|row| row.tail.is_some()) { return actual }
        let slots = declared_rows.iter().map(|row| row.fixed.len()).max().unwrap_or(0);
        let Some(actual_rows) = self.native_pack_alternatives(actual) else { return actual };
        if !actual_rows.iter().any(|row| row.tail.is_some()) { return actual }
        let mut rows = Vec::new();
        for row in actual_rows {
            let Some(tail) = row.tail else { rows.push(row); continue };
            let mut fixed = row.fixed;
            loop {
                rows.push(NativePackAlternative { fixed: fixed.clone(), tail: None });
                if fixed.len() >= slots { break }
                fixed.push(NativePackSlot { ty: tail, optional: false });
            }
        }
        self.native_result_pack(&rows)
    }

    /// A result list (a return's, a call's arguments) fits `declared` when its values do; a
    /// variadic tail past the declared slots is dropped, as Lua drops it.
    pub fn native_result_list_assignable(&mut self, actual: NativeResultPack, declared: NativeResultPack) -> bool {
        // A failed call's values are the error type wherever a list carries them, as TypeScript
        // relates its error type to anything: the failure is reported once, at the call.
        if self.native_pack_alternatives(actual).is_some_and(|rows| rows.iter().any(|row| {
            row.tail.is_some_and(|tail| self.is_error_type(tail)) || row.fixed.iter().any(|slot| self.is_error_type(slot.ty))
        })) { return true }
        let actual = self.native_pack_without_possible_excess(actual, declared);
        self.native_pack_assignable(actual, declared)
    }

    pub fn native_pack_assignable(&mut self, actual: NativeResultPack, declared: NativeResultPack) -> bool {
        if self.is_assignable(actual.ty, declared.ty) { return true; }
        // A variadic opaque source can straddle an optional first slot and a
        // homogeneous tail. Prove that its covariant envelope fits, without
        // changing the original source or discarding any target prefix/length.
        let bounded = self.native_pack_covariant_bound(actual.ty);
        bounded != actual.ty && self.is_assignable(bounded, declared.ty)
    }

    /// Callable targets with no observed results have TypeScript's void-return
    /// variance. This does not change physical result-list assignability. Never
    /// has no reachable rows and must not be mistaken for a zero-result row.
    pub(super) fn native_pack_ignores_callback_results(&mut self, declared: NativeResultPack) -> bool {
        let declared = self.native_declared_result_pack(declared);
        self.native_pack_alternatives(declared).is_some_and(|rows| {
            !rows.is_empty() && rows.iter().all(|row| row.fixed.is_empty() && row.tail.is_none())
        })
    }

    pub fn instantiate_native_pack(&mut self, pack: NativeResultPack, mapper: MapperId) -> NativeResultPack {
        NativeResultPack { ty: self.instantiate(pack.ty, mapper) }
    }
}
