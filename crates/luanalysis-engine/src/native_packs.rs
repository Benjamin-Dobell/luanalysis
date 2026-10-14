//! Lua result sequences remain distinct from scalar tuple/array values.

use super::{
    CallError, CapturedFunctionContext, Checker, ContextualFunctionContext, NativeExpressionKey,
    NativeTypeParameterKey, Parameter, SessionQueries, Type, TypeHandles, core_expression_key,
    export_type, import_type,
};
use bun_sema::check::native_packs::{
    NativePackAdjustmentError, NativePackAlternative, NativePackSlot,
};
use bun_sema::types::NativeResultPack;
use bun_sema::types::SigParam;

#[derive(Debug, Clone, PartialEq)]
pub struct PackFunctionType {
    pub params: Vec<Parameter>,
    pub rest: Option<ValuePack>,
    pub type_parameters: Vec<NativeTypeParameterKey>,
    pub returns: ValuePack,
}

/// Arguments retain their original expression demand or exact pack identity.
/// Only the final argument may expand into multiple physical values.
#[derive(Debug, Clone, PartialEq)]
pub enum PackArgument {
    Resolved(Type),
    Expression(NativeExpressionKey),
    Expand(ValuePack),
    ExpandExpression(NativeExpressionKey),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackSlot {
    pub ty: Type,
    pub optional: bool,
}

impl PackSlot {
    pub fn required(ty: Type) -> Self {
        Self {
            ty,
            optional: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackAlternative {
    pub fixed: Vec<PackSlot>,
    pub tail: Option<Box<Type>>,
}

/// One alternative is one correlated physical result sequence. An empty
/// alternative list has no reachable result; one empty row has zero results.
#[derive(Debug, Clone, PartialEq)]
pub enum ValuePack {
    Alternatives(Vec<PackAlternative>),
    Session(SessionPackRef),
}

impl ValuePack {
    pub fn empty() -> Self {
        Self::Alternatives(vec![PackAlternative {
            fixed: Vec::new(),
            tail: None,
        }])
    }
    pub fn single(ty: Type) -> Self {
        Self::Alternatives(vec![PackAlternative {
            fixed: vec![PackSlot::required(ty)],
            tail: None,
        }])
    }
    pub fn variadic(element: Type) -> Self {
        Self::Alternatives(vec![PackAlternative {
            fixed: Vec::new(),
            tail: Some(Box::new(element)),
        }])
    }
    pub fn never() -> Self {
        Self::Alternatives(Vec::new())
    }
}

/// Exact native pack identity in one checker epoch, never a backend type ID.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionPackRef {
    epoch: u64,
    slot: u32,
}

impl std::fmt::Debug for SessionPackRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SessionPackRef")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidSessionPack(pub SessionPackRef);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackAdjustmentError {
    TooComplex { count: usize, alternatives: usize },
    Unresolved,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackCallResult {
    pub return_pack: ValuePack,
    pub return_type: Type,
    pub errors: Vec<CallError>,
}

impl TypeHandles {
    pub(super) fn retain_pack(
        &self,
        checker: &Checker<'_, '_>,
        pack: NativeResultPack,
    ) -> ValuePack {
        let ty = checker.native_pack_type(pack);
        let mut state = self.state.borrow_mut();
        let slot = match state.pack_slots.get(&ty).copied() {
            Some(slot) => slot,
            None => {
                let slot = u32::try_from(state.packs.len()).expect("pack handle capacity");
                state.packs.push(pack);
                state.pack_slots.insert(ty, slot);
                slot
            }
        };
        ValuePack::Session(SessionPackRef {
            epoch: self.epoch,
            slot,
        })
    }
    fn resolve_pack(&self, reference: SessionPackRef) -> NativeResultPack {
        if reference.epoch != self.epoch {
            std::panic::panic_any(InvalidSessionPack(reference));
        }
        self.state
            .borrow()
            .packs
            .get(reference.slot as usize)
            .copied()
            .unwrap_or_else(|| std::panic::panic_any(InvalidSessionPack(reference)))
    }
}

pub(super) fn import_pack(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    pack: &ValuePack,
) -> NativeResultPack {
    match pack {
        ValuePack::Session(reference) => handles.resolve_pack(*reference),
        ValuePack::Alternatives(alternatives) => {
            let mut native = Vec::with_capacity(alternatives.len());
            for alternative in alternatives {
                let fixed = alternative
                    .fixed
                    .iter()
                    .map(|slot| NativePackSlot {
                        ty: import_type(checker, handles, &slot.ty),
                        optional: slot.optional,
                    })
                    .collect();
                let tail = alternative
                    .tail
                    .as_ref()
                    .map(|ty| import_type(checker, handles, ty));
                native.push(NativePackAlternative { fixed, tail });
            }
            checker.native_result_pack(&native)
        }
    }
}

pub(super) fn import_declared_pack(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    pack: &ValuePack,
) -> NativeResultPack {
    let pack = import_pack(checker, handles, pack);
    checker.native_declared_result_pack(pack)
}

impl SessionQueries<'_, '_, '_> {
    pub(super) fn declared_result_pack_native(&mut self, pack: &ValuePack) -> ValuePack {
        let pack = import_declared_pack(self.checker, self.handles, pack);
        self.handles.retain_pack(self.checker, pack)
    }
    pub(super) fn return_pack_with_cardinality_native(
        &mut self,
        declared: &ValuePack,
        actual: &ValuePack,
    ) -> ValuePack {
        let declared = import_pack(self.checker, self.handles, declared);
        let actual = import_pack(self.checker, self.handles, actual);
        let pack = self
            .checker
            .native_return_pack_with_cardinality(declared, actual);
        self.handles.retain_pack(self.checker, pack)
    }
    pub(super) fn function_declared_scalar_return_native(
        &mut self,
        function: &Type,
    ) -> Option<Type> {
        let function = import_type(self.checker, self.handles, function);
        let signatures = self.checker.signatures(function, false);
        if signatures.len() != 1 {
            return None;
        }
        self.checker
            .sig_declared_scalar_return(signatures[0])
            .map(|ty| export_type(self.checker, self.handles, ty))
    }
    fn import_arguments(
        &mut self,
        arguments: &[PackArgument],
    ) -> Vec<bun_sema::check::native_packs::NativeCallArgument> {
        let mut native = Vec::with_capacity(arguments.len());
        for argument in arguments {
            use bun_sema::check::native_packs::NativeCallArgument as Arg;
            native.push(match argument {
                PackArgument::Resolved(ty) => {
                    Arg::Resolved(import_type(self.checker, self.handles, ty))
                }
                PackArgument::Expression(key) => {
                    self.validate_expression(*key);
                    Arg::Expression(core_expression_key(*key))
                }
                PackArgument::Expand(pack) => {
                    Arg::Expand(import_pack(self.checker, self.handles, pack))
                }
                PackArgument::ExpandExpression(key) => {
                    self.validate_expression(*key);
                    Arg::ExpandExpression(core_expression_key(*key))
                }
            });
        }
        native
    }
    pub(super) fn call_native_arguments_scalar_owned(
        &mut self,
        callee: &Type,
        call: Option<NativeExpressionKey>,
        arguments: &[PackArgument],
        type_arguments: &[Type],
        expected_return: Option<&Type>,
    ) -> super::CallResult {
        if let Some(call) = call {
            self.validate_expression(call);
        }
        let callee = import_type(self.checker, self.handles, callee);
        let native = self.import_arguments(arguments);
        let type_arguments: Vec<_> = type_arguments
            .iter()
            .map(|ty| import_type(self.checker, self.handles, ty))
            .collect();
        let expected = expected_return.map(|ty| import_type(self.checker, self.handles, ty));
        let result = self.checker.call_native_arguments_scalar(
            callee,
            call.map(core_expression_key),
            &native,
            &type_arguments,
            expected,
        );
        self.export_call_result(result)
    }
    pub(super) fn call_native_arguments_owned(
        &mut self,
        callee: &Type,
        call: Option<NativeExpressionKey>,
        arguments: &[PackArgument],
        type_arguments: &[Type],
        expected: Option<&ValuePack>,
    ) -> PackCallResult {
        if let Some(call) = call {
            self.validate_expression(call);
        }
        let callee = import_type(self.checker, self.handles, callee);
        let native = self.import_arguments(arguments);
        let type_arguments: Vec<_> = type_arguments
            .iter()
            .map(|ty| import_type(self.checker, self.handles, ty))
            .collect();
        let expected = expected.map(|pack| import_pack(self.checker, self.handles, pack));
        let result = self.checker.call_native_arguments(
            callee,
            call.map(core_expression_key),
            &native,
            &type_arguments,
            expected,
        );
        self.export_pack_call(result)
    }
    pub(super) fn pack_function_native(&mut self, function: &PackFunctionType) -> Type {
        let mut params: Vec<_> = function
            .params
            .iter()
            .map(|param| {
                let mut ty = import_type(self.checker, self.handles, &param.ty);
                if param.optional {
                    ty = self.checker.optional(ty);
                }
                SigParam {
                    name: self.checker.atoms().intern(param.name.as_bytes()),
                    ty,
                    optional: param.optional,
                    rest: false,
                    is_required_rest: false,
                    declaration: None,
                }
            })
            .collect();
        if let Some(rest) = &function.rest {
            let pack = import_pack(self.checker, self.handles, rest);
            params.push(SigParam {
                name: self.checker.atoms().intern(b"..."),
                ty: self.checker.native_pack_type(pack),
                optional: false,
                rest: true,
                is_required_rest: false,
                declaration: None,
            });
        }
        let type_parameters: Vec<_> = function
            .type_parameters
            .iter()
            .map(|key| import_type(self.checker, self.handles, &Type::NativeTypeParameter(*key)))
            .collect();
        let pack = import_pack(self.checker, self.handles, &function.returns);
        let ty = self
            .checker
            .native_pack_function(&params, &type_parameters, pack);
        self.handles.retain(ty)
    }
    fn export_pack_call(
        &mut self,
        result: bun_sema::check::native_packs::NativeResultPackCall,
    ) -> PackCallResult {
        let ret = self.checker.native_pack_slot(result.pack, 0);
        let return_pack = self.handles.retain_pack(self.checker, result.pack);
        let scalar = self.export_call_result(bun_sema::check::ResolvedTypeCall {
            sig: None,
            ret,
            errors: result.errors,
        });
        PackCallResult {
            return_pack,
            return_type: scalar.return_type,
            errors: scalar.errors,
        }
    }
    pub(super) fn call_pack_native(
        &mut self,
        callee: &Type,
        arguments: &[Type],
        type_arguments: &[Type],
        expected: Option<&ValuePack>,
    ) -> PackCallResult {
        let callee = import_type(self.checker, self.handles, callee);
        let arguments: Vec<_> = arguments
            .iter()
            .map(|ty| import_type(self.checker, self.handles, ty))
            .collect();
        let type_arguments: Vec<_> = type_arguments
            .iter()
            .map(|ty| import_type(self.checker, self.handles, ty))
            .collect();
        let expected = expected.map(|pack| import_pack(self.checker, self.handles, pack));
        let result =
            self.checker
                .call_resolved_result_pack(callee, &arguments, &type_arguments, expected);
        self.export_pack_call(result)
    }
    pub(super) fn call_expression_pack_native(
        &mut self,
        callee: &Type,
        call: NativeExpressionKey,
        arguments: &[NativeExpressionKey],
        type_arguments: &[Type],
        expected: Option<&ValuePack>,
    ) -> PackCallResult {
        self.validate_expression(call);
        for argument in arguments {
            self.validate_expression(*argument);
        }
        let callee = import_type(self.checker, self.handles, callee);
        let arguments: Vec<_> = arguments.iter().copied().map(core_expression_key).collect();
        let type_arguments: Vec<_> = type_arguments
            .iter()
            .map(|ty| import_type(self.checker, self.handles, ty))
            .collect();
        let expected = expected.map(|pack| import_pack(self.checker, self.handles, pack));
        let result = self.checker.call_expression_result_pack(
            callee,
            core_expression_key(call),
            &arguments,
            &type_arguments,
            expected,
        );
        self.export_pack_call(result)
    }
    pub(super) fn function_result_pack_native(&mut self, function: &Type) -> Option<ValuePack> {
        let function = import_type(self.checker, self.handles, function);
        let signatures = self.checker.signatures(function, false);
        if signatures.len() != 1 {
            return None;
        }
        let pack = self.checker.sig_result_pack(signatures[0]);
        Some(self.handles.retain_pack(self.checker, pack))
    }
    pub(super) fn signature_rest_pack_native(&mut self, function: &Type) -> Option<ValuePack> {
        let function = import_type(self.checker, self.handles, function);
        let signatures = self.checker.signatures(function, false);
        if signatures.len() != 1 {
            return None;
        }
        self.checker
            .signature_rest_pack(signatures[0])
            .map(|pack| self.handles.retain_pack(self.checker, pack))
    }
    pub(super) fn contextual_rest_pack_native(
        &mut self,
        context: &ContextualFunctionContext,
        fixed_count: usize,
    ) -> ValuePack {
        let context = self.handles.resolve_context(context);
        let pack = self.checker.contextual_rest_pack(context, fixed_count);
        self.handles.retain_pack(self.checker, pack)
    }
    pub(super) fn contextual_return_pack_native(
        &mut self,
        context: &ContextualFunctionContext,
    ) -> ValuePack {
        let context = self.handles.resolve_context(context);
        let pack = self.checker.native_contextual_return_pack(context);
        self.handles.retain_pack(self.checker, pack)
    }
    pub(super) fn captured_rest_pack_native(
        &mut self,
        context: &CapturedFunctionContext,
    ) -> Option<ValuePack> {
        let context = self.handles.resolve_capture(context);
        self.checker
            .native_capture_rest_pack(context)
            .map(|pack| self.handles.retain_pack(self.checker, pack))
    }
    pub(super) fn instantiate_pack_in_capture_native(
        &mut self,
        context: &CapturedFunctionContext,
        pack: &ValuePack,
    ) -> ValuePack {
        let context = self.handles.resolve_capture(context);
        let pack = import_pack(self.checker, self.handles, pack);
        let mapped = self
            .checker
            .instantiate_native_pack_in_capture(context, pack);
        self.handles.retain_pack(self.checker, mapped)
    }
    pub(super) fn retain_pack_native(&mut self, pack: &ValuePack) -> ValuePack {
        let pack = import_pack(self.checker, self.handles, pack);
        self.handles.retain_pack(self.checker, pack)
    }
    pub(super) fn inspect_pack_native(&mut self, pack: &ValuePack) -> Option<Vec<PackAlternative>> {
        let pack = import_pack(self.checker, self.handles, pack);
        let alternatives = self.checker.native_pack_alternatives(pack)?;
        Some(
            alternatives
                .into_iter()
                .map(|alternative| PackAlternative {
                    fixed: alternative
                        .fixed
                        .into_iter()
                        .map(|slot| PackSlot {
                            ty: export_type(self.checker, self.handles, slot.ty),
                            optional: slot.optional,
                        })
                        .collect(),
                    tail: alternative
                        .tail
                        .map(|ty| Box::new(export_type(self.checker, self.handles, ty))),
                })
                .collect(),
        )
    }
    pub(super) fn pack_slot_native(&mut self, pack: &ValuePack, index: usize) -> Type {
        let pack = import_pack(self.checker, self.handles, pack);
        let ty = self.checker.native_pack_slot(pack, index);
        export_type(self.checker, self.handles, ty)
    }
    pub(super) fn pack_contextual_slot_native(
        &mut self,
        pack: &ValuePack,
        index: usize,
    ) -> Option<Type> {
        let pack = import_pack(self.checker, self.handles, pack);
        self.checker
            .native_pack_contextual_slot(pack, index)
            .map(|ty| export_type(self.checker, self.handles, ty))
    }
    pub(super) fn adjust_pack_native(
        &mut self,
        pack: &ValuePack,
        count: usize,
    ) -> Result<ValuePack, PackAdjustmentError> {
        let pack = import_pack(self.checker, self.handles, pack);
        self.checker
            .native_adjust_pack(pack, count)
            .map(|pack| self.handles.retain_pack(self.checker, pack))
            .map_err(|error| match error {
                NativePackAdjustmentError::TooComplex {
                    count,
                    alternatives,
                } => PackAdjustmentError::TooComplex {
                    count,
                    alternatives,
                },
                NativePackAdjustmentError::Unresolved => PackAdjustmentError::Unresolved,
            })
    }
    pub(super) fn concat_pack_native(&mut self, prefix: &[Type], tail: &ValuePack) -> ValuePack {
        let prefix: Vec<_> = prefix
            .iter()
            .map(|ty| import_type(self.checker, self.handles, ty))
            .collect();
        let tail = import_pack(self.checker, self.handles, tail);
        let pack = self.checker.native_concat_pack(&prefix, tail);
        self.handles.retain_pack(self.checker, pack)
    }
    pub(super) fn pack_contextual_tail_native(
        &mut self,
        expected: &ValuePack,
        prefix: &[Type],
    ) -> Option<ValuePack> {
        let expected = import_pack(self.checker, self.handles, expected);
        let prefix: Vec<_> = prefix
            .iter()
            .map(|ty| import_type(self.checker, self.handles, ty))
            .collect();
        self.checker
            .native_pack_contextual_tail(expected, &prefix)
            .map(|pack| self.handles.retain_pack(self.checker, pack))
    }
    pub(super) fn join_packs_native(&mut self, packs: &[ValuePack]) -> ValuePack {
        let packs: Vec<_> = packs
            .iter()
            .map(|pack| import_pack(self.checker, self.handles, pack))
            .collect();
        let pack = self.checker.native_join_packs(&packs);
        self.handles.retain_pack(self.checker, pack)
    }
    pub(super) fn aggregate_result_packs_native(
        &mut self,
        packs: &[ValuePack],
        has_fallthrough: bool,
        context: Option<&ValuePack>,
    ) -> ValuePack {
        let packs: Vec<_> = packs
            .iter()
            .map(|pack| import_pack(self.checker, self.handles, pack))
            .collect();
        let context = context.map(|pack| import_pack(self.checker, self.handles, pack));
        let pack = self
            .checker
            .native_aggregate_result_packs(&packs, has_fallthrough, context);
        self.handles.retain_pack(self.checker, pack)
    }
    pub(super) fn pack_assignable_native(
        &mut self,
        actual: &ValuePack,
        declared: &ValuePack,
    ) -> bool {
        let actual = import_pack(self.checker, self.handles, actual);
        let declared = import_pack(self.checker, self.handles, declared);
        self.checker.native_pack_assignable(actual, declared)
    }
    pub(super) fn pack_allows_implicit_return_native(&mut self, pack: &ValuePack) -> bool {
        let pack = import_pack(self.checker, self.handles, pack);
        self.checker.native_pack_allows_implicit_return(pack)
    }
    pub(super) fn pack_with_implicit_empty_native(&mut self, pack: &ValuePack) -> ValuePack {
        let pack = import_pack(self.checker, self.handles, pack);
        let pack = self.checker.native_pack_with_implicit_empty(pack);
        self.handles.retain_pack(self.checker, pack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, NativeFunctionKey, NativeTypeProvider, Parameter, TypeQueries};
    use std::cell::Cell;

    fn row(types: Vec<Type>) -> PackAlternative {
        PackAlternative {
            fixed: types.into_iter().map(PackSlot::required).collect(),
            tail: None,
        }
    }
    fn fixed(types: Vec<Type>) -> ValuePack {
        ValuePack::Alternatives(vec![row(types)])
    }

    #[test]
    fn native_packs_keep_correlated_return_alternatives_and_exact_required_counts() {
        Engine::new().with_session(|queries| {
            let actual = ValuePack::Alternatives(vec![
                row(vec![Type::Number, Type::String]),
                row(vec![Type::String, Type::Number]),
            ]);
            let mismatched = ValuePack::Alternatives(vec![
                row(vec![Type::Number, Type::Number]),
                row(vec![Type::String, Type::String]),
            ]);
            assert!(queries.pack_assignable(&actual, &actual));
            assert!(!queries.pack_assignable(&actual, &mismatched));
            assert!(!queries.pack_assignable(
                &fixed(vec![Type::Number]),
                &fixed(vec![
                    Type::Number,
                    Type::Union(vec![Type::String, Type::Nil])
                ])
            ));
            assert!(!queries.pack_assignable(
                &fixed(vec![Type::Number, Type::String]),
                &fixed(vec![Type::Number])
            ));
            let joined = queries.join_packs(&[
                fixed(vec![Type::Number, Type::String]),
                fixed(vec![Type::String, Type::Number]),
            ]);
            assert!(queries.pack_assignable(&joined, &actual));
            assert!(!queries.pack_assignable(&joined, &mismatched));
            assert_eq!(queries.inspect_pack(&joined).unwrap().len(), 2);
        });
    }

    #[test]
    fn native_result_pack_aggregation_keeps_contextual_discriminants_and_fallthrough_rows() {
        Engine::new().with_session(|queries| {
            let expected = ValuePack::Alternatives(vec![
                row(vec![Type::BooleanLiteral(true), Type::Number]),
                row(vec![Type::BooleanLiteral(false), Type::String]),
            ]);
            let returned = queries.aggregate_result_packs(
                &[
                    fixed(vec![Type::BooleanLiteral(true), Type::NumberLiteral(1.0)]),
                    fixed(vec![
                        Type::BooleanLiteral(false),
                        Type::StringLiteral(vec![0xff]),
                    ]),
                ],
                false,
                Some(&expected),
            );
            assert!(queries.pack_assignable(&returned, &expected));
            assert_eq!(queries.inspect_pack(&returned).unwrap().len(), 2);
            let with_fallthrough = queries.aggregate_result_packs(
                &[fixed(vec![Type::NumberLiteral(1.0)])],
                true,
                None,
            );
            assert_eq!(queries.inspect_pack(&with_fallthrough).unwrap().len(), 2);
            assert!(!queries.pack_assignable(&with_fallthrough, &ValuePack::single(Type::Number)));
            assert!(queries.pack_assignable(
                &with_fallthrough,
                &ValuePack::Alternatives(vec![row(Vec::new()), row(vec![Type::Number]),])
            ));
        });
    }

    #[test]
    fn native_pack_tail_context_filters_whole_alternatives_before_consuming_prefix() {
        Engine::new().with_session(|queries| {
            let expected = ValuePack::Alternatives(vec![
                row(vec![Type::BooleanLiteral(false)]),
                row(vec![Type::BooleanLiteral(true), Type::Number]),
                row(vec![Type::BooleanLiteral(true), Type::String]),
            ]);
            let tail = queries
                .pack_contextual_tail(&expected, &[Type::BooleanLiteral(true)])
                .unwrap();
            assert_eq!(queries.inspect_pack(&tail).unwrap().len(), 2);
            assert!(!queries.pack_assignable(&ValuePack::empty(), &tail));
            assert!(queries.pack_assignable(&ValuePack::single(Type::Number), &tail));
            assert!(queries.pack_assignable(&ValuePack::single(Type::String), &tail));
            let false_tail = queries
                .pack_contextual_tail(&expected, &[Type::BooleanLiteral(false)])
                .unwrap();
            assert!(queries.pack_assignable(&ValuePack::empty(), &false_tail));
            assert!(!queries.pack_assignable(&ValuePack::single(Type::Number), &false_tail));
            assert!(
                queries
                    .pack_contextual_tail(&expected, &[Type::Number])
                    .is_none()
            );
        });
    }

    #[test]
    fn native_dynamic_call_results_keep_unknown_cardinality_until_assignment_adjustment() {
        Engine::new().with_session(|queries| {
            let called = queries.call_pack(&Type::Any, &[], &[], None);
            assert!(called.errors.is_empty());
            assert_eq!(called.return_type, Type::Any);
            assert_eq!(queries.pack_slot(&called.return_pack, 100), Type::Any);
            assert!(queries.pack_assignable(&called.return_pack, &ValuePack::empty()));
            assert!(queries.pack_assignable(
                &called.return_pack,
                &fixed(vec![Type::Number, Type::String])
            ));
            let listed = queries.concat_pack(&[], &called.return_pack);
            assert_eq!(listed, called.return_pack);
            assert!(queries.pack_assignable(&listed, &ValuePack::empty()));
            assert!(queries.pack_assignable(&listed, &fixed(vec![Type::Number, Type::String])));
            let adjusted = queries.adjust_pack(&listed, 2).unwrap();
            let alternatives = queries.inspect_pack(&adjusted).unwrap();
            assert_eq!(alternatives.len(), 1);
            assert_eq!(alternatives[0].fixed.len(), 2);
            assert_eq!(queries.pack_slot(&adjusted, 0), Type::Any);
            assert_eq!(queries.pack_slot(&adjusted, 1), Type::Any);
            assert_eq!(queries.pack_slot(&adjusted, 2), Type::Nil);
        });
    }

    #[test]
    fn dynamic_any_tail_assignment_keeps_known_prefix_and_unknown_suffix() {
        Engine::new().with_session(|queries| {
            let any = queries.call_pack(&Type::Any, &[], &[], None).return_pack;
            let packed = queries.concat_pack(&[Type::Number, Type::String], &any);
            let adjusted = queries.adjust_pack(&packed, 4).unwrap();
            for (index, expected) in [Type::Number, Type::String, Type::Any, Type::Any]
                .into_iter()
                .enumerate()
            {
                assert_eq!(queries.pack_slot(&adjusted, index), expected);
            }
            assert_eq!(queries.inspect_pack(&adjusted).unwrap().len(), 1);
            let zero = queries.pack_function(&PackFunctionType {
                params: Vec::new(),
                rest: None,
                type_parameters: Vec::new(),
                returns: ValuePack::empty(),
            });
            let called = queries.call_native_arguments(
                &zero,
                None,
                &[PackArgument::Expand(packed)],
                &[],
                None,
            );
            assert!(matches!(
                called.errors.as_slice(),
                [CallError::PackArity {
                    actual_minimum: 2,
                    actual_maximum: None,
                    maximum: Some(0),
                    ..
                }]
            ));
            let finite = queries
                .adjust_pack(&fixed(vec![Type::Number, Type::String]), 4)
                .unwrap();
            assert_eq!(queries.pack_slot(&finite, 2), Type::Nil);
            let numeric = queries.concat_pack(
                &[Type::Number, Type::String],
                &ValuePack::variadic(Type::Number),
            );
            let numeric = queries.adjust_pack(&numeric, 4).unwrap();
            let third = queries.pack_slot(&numeric, 2);
            assert!(queries.is_assignable(&Type::Number, &third));
            assert!(queries.is_assignable(&Type::Nil, &third));
            assert!(queries.inspect_pack(&numeric).unwrap().len() > 1);
        });
    }

    #[test]
    fn native_pack_implicit_return_policy_preserves_required_counts_and_scalar_ts_policy() {
        Engine::new().with_session(|queries| {
            for permitted in [
                ValuePack::empty(),
                ValuePack::single(Type::Void),
                ValuePack::single(Type::Nil),
                ValuePack::single(Type::Any),
                ValuePack::variadic(Type::Number),
                ValuePack::Alternatives(vec![PackAlternative {
                    fixed: vec![PackSlot {
                        ty: Type::Number,
                        optional: true,
                    }],
                    tail: None,
                }]),
            ] {
                assert!(
                    queries.pack_allows_implicit_return(&permitted),
                    "{permitted:?}"
                );
            }
            for required in [
                ValuePack::single(Type::Number),
                ValuePack::single(Type::Union(vec![Type::Number, Type::Nil])),
                fixed(vec![Type::Number, Type::Nil]),
                fixed(vec![Type::Void, Type::Nil]),
                ValuePack::Alternatives(vec![PackAlternative {
                    fixed: vec![PackSlot::required(Type::Number)],
                    tail: Some(Box::new(Type::Boolean)),
                }]),
            ] {
                assert!(
                    !queries.pack_allows_implicit_return(&required),
                    "{required:?}"
                );
                let unchanged = queries.pack_with_implicit_empty(&required);
                assert!(!queries.pack_assignable(&ValuePack::empty(), &unchanged));
            }
            let alternatives = ValuePack::Alternatives(vec![
                row(vec![Type::Nil]),
                row(vec![Type::Number, Type::String]),
            ]);
            assert!(queries.pack_allows_implicit_return(&alternatives));
            assert!(!queries.pack_assignable(&ValuePack::empty(), &alternatives));
            let actual = queries.pack_with_implicit_empty(&alternatives);
            assert!(queries.pack_assignable(&ValuePack::empty(), &actual));
            assert!(queries.pack_assignable(&alternatives, &actual));
            assert!(!queries.pack_assignable(&fixed(vec![Type::Number]), &actual));
            assert_eq!(queries.inspect_pack(&actual).unwrap().len(), 3);
        });
    }

    #[test]
    fn native_pack_context_guidance_never_pads_a_missing_target_with_nil() {
        Engine::new().with_session(|queries| {
            let single = ValuePack::single(Type::Number);
            assert_eq!(queries.pack_contextual_slot(&single, 0), Some(Type::Number));
            assert_eq!(queries.pack_contextual_slot(&single, 1), None);
            assert_eq!(queries.pack_slot(&single, 1), Type::Nil);
            let optional = ValuePack::Alternatives(vec![PackAlternative {
                fixed: vec![PackSlot {
                    ty: Type::String,
                    optional: true,
                }],
                tail: None,
            }]);
            let expected = queries.pack_contextual_slot(&optional, 0).unwrap();
            assert!(queries.is_assignable(&Type::String, &expected));
            assert!(queries.is_assignable(&Type::Nil, &expected));
            assert_eq!(queries.pack_contextual_slot(&optional, 1), None);
            assert_eq!(
                queries.pack_contextual_slot(&ValuePack::variadic(Type::String), 10),
                Some(Type::String)
            );
            let dynamic = queries.call_pack(&Type::Any, &[], &[], None).return_pack;
            assert_eq!(queries.pack_contextual_slot(&dynamic, 10), Some(Type::Any));
            assert_eq!(queries.pack_contextual_slot(&ValuePack::empty(), 0), None);
        });
    }

    #[test]
    fn native_pack_return_projection_preserves_function_identity_and_regularizes_literal_shapes() {
        Engine::new().with_session(|queries| {
            let fresh = Type::FreshShape(vec![
                crate::Property::new("x", Type::NumberLiteral(1.0)),
                crate::Property::new("extra", Type::BooleanLiteral(true)),
            ]);
            let target = Type::Shape(vec![crate::Property::new("x", Type::Number)]);
            assert!(!queries.compare(&fresh, &target).assignable);
            let returned = queries.aggregate_result_packs(&[ValuePack::single(fresh)], false, None);
            let result = queries.pack_slot(&returned, 0);
            assert!(queries.compare(&result, &target).assignable);
            let function = queries.pack_function(&PackFunctionType {
                params: Vec::new(),
                rest: None,
                type_parameters: Vec::new(),
                returns: fixed(vec![Type::Number, Type::String]),
            });
            let outer = queries.aggregate_result_packs(&[ValuePack::single(function)], false, None);
            let function = queries.pack_slot(&outer, 0);
            let results = queries.call_pack(&function, &[], &[], None);
            assert!(results.errors.is_empty());
            assert_eq!(queries.pack_slot(&results.return_pack, 1), Type::String);
        });
    }

    #[test]
    fn native_packs_adjust_zero_fixed_optional_and_tail_results() {
        Engine::new().with_session(|queries| {
            assert_eq!(queries.pack_slot(&ValuePack::empty(), 0), Type::Nil);
            assert_eq!(queries.pack_slot(&ValuePack::never(), 0), Type::Never);
            let many = fixed(vec![Type::Number, Type::String]);
            assert_eq!(queries.pack_slot(&many, 0), Type::Number);
            assert_eq!(queries.pack_slot(&many, 2), Type::Nil);
            let truncated = queries.adjust_pack(&many, 1).unwrap();
            assert!(queries.pack_assignable(&truncated, &fixed(vec![Type::Number])));
            let padded = queries
                .adjust_pack(&ValuePack::single(Type::Number), 2)
                .unwrap();
            assert!(queries.pack_assignable(&padded, &fixed(vec![Type::Number, Type::Nil])));
            let tail = ValuePack::variadic(Type::Number);
            let slot = queries.pack_slot(&tail, 7);
            assert!(queries.is_assignable(&Type::Number, &slot));
            assert!(queries.is_assignable(&Type::Nil, &slot));
            assert!(queries.pack_assignable(&ValuePack::empty(), &tail));
            let prefix_tail = ValuePack::Alternatives(vec![PackAlternative {
                fixed: vec![PackSlot::required(Type::String)],
                tail: Some(Box::new(Type::Number)),
            }]);
            assert!(!queries.pack_assignable(&ValuePack::empty(), &prefix_tail));
            let adjusted = queries.adjust_pack(&tail, 2).unwrap();
            let expected = ValuePack::Alternatives(vec![
                row(vec![Type::Nil, Type::Nil]),
                row(vec![Type::Number, Type::Nil]),
                row(vec![Type::Number, Type::Number]),
            ]);
            assert!(queries.pack_assignable(&adjusted, &expected));
            assert!(queries.pack_assignable(&expected, &adjusted));
            assert!(!queries.pack_assignable(&fixed(vec![Type::Nil, Type::Number]), &adjusted));
            let optional = ValuePack::Alternatives(vec![PackAlternative {
                fixed: vec![PackSlot {
                    ty: Type::Number,
                    optional: true,
                }],
                tail: None,
            }]);
            let optional_slot = queries.pack_slot(&optional, 0);
            assert!(queries.is_assignable(&Type::Nil, &optional_slot));
            assert!(queries.is_assignable(&Type::Number, &optional_slot));
            let concat = queries.concat_pack(&[Type::Boolean], &many);
            assert!(queries.pack_assignable(
                &concat,
                &fixed(vec![Type::Boolean, Type::Number, Type::String])
            ));
            assert!(matches!(
                queries.adjust_pack(&tail, usize::MAX),
                Err(PackAdjustmentError::TooComplex { .. })
            ));
        });
    }

    #[test]
    fn native_pack_capabilities_validate_epoch_and_cleanup() {
        let pack = Engine::new()
            .with_session(|queries| queries.retain_pack(&fixed(vec![Type::Number, Type::String])));
        let ValuePack::Session(reference) = pack else {
            panic!("exact native capability expected");
        };
        let panic = std::panic::catch_unwind(|| {
            Engine::new().with_session(|queries| queries.pack_slot(&pack, 0))
        })
        .unwrap_err();
        assert_eq!(
            panic.downcast_ref::<InvalidSessionPack>(),
            Some(&InvalidSessionPack(reference))
        );
        assert!(Engine::new().is_assignable(&Type::Number, &Type::Number));
    }

    #[derive(Default)]
    struct LazyPackProvider {
        returns: Cell<usize>,
        cycles: Cell<usize>,
    }

    impl NativeTypeProvider for LazyPackProvider {
        fn contains_function(&self, key: NativeFunctionKey) -> bool {
            key.snapshot == 401 && key.index < 2
        }
        fn function_parameters(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
            Vec::new()
        }
        fn function_declared_return(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Option<Type> {
            panic!("packed signature must not demand scalar source return metadata")
        }
        fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
            panic!("packed signature must not invoke scalar inference")
        }
        fn function_has_result_pack(&self, _: NativeFunctionKey) -> bool {
            true
        }
        fn function_declared_return_pack(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Option<ValuePack> {
            None
        }
        fn function_inferred_return_pack(
            &self,
            key: NativeFunctionKey,
            queries: &mut dyn TypeQueries,
        ) -> ValuePack {
            self.returns.set(self.returns.get() + 1);
            match key.index {
                0 => fixed(vec![Type::Number, Type::String]),
                1 => queries
                    .function_result_pack(&Type::NativeFunction(key))
                    .unwrap(),
                _ => unreachable!(),
            }
        }
        fn return_cycle(&self, _: NativeFunctionKey) {
            self.cycles.set(self.cycles.get() + 1);
        }
        fn return_type_too_deep(&self, _: NativeFunctionKey) {
            panic!("fixture has finite depth")
        }
    }

    #[test]
    fn native_lazy_result_packs_share_return_query_cache_and_cycle_guards() {
        Engine::new().with_native_provider(LazyPackProvider::default, |provider, queries| {
            let function = Type::NativeFunction(NativeFunctionKey {
                snapshot: 401,
                index: 0,
            });
            let first = queries.function_result_pack(&function).unwrap();
            let second = queries.call_pack(&function, &[], &[], None);
            assert!(second.errors.is_empty());
            assert_eq!(first, second.return_pack);
            assert_eq!(queries.call(&function, &[]).return_type, Type::Number);
            assert_eq!(queries.pack_slot(&first, 1), Type::String);
            assert_eq!(provider.returns.get(), 1);

            let circular = Type::NativeFunction(NativeFunctionKey {
                snapshot: 401,
                index: 1,
            });
            let cycle = queries.function_result_pack(&circular).unwrap();
            assert_eq!(queries.pack_slot(&cycle, 0), Type::Any);
            assert_eq!(provider.cycles.get(), 1);
            let cycle_again = queries.function_result_pack(&circular).unwrap();
            assert_eq!(cycle, cycle_again);
            assert_eq!(provider.returns.get(), 2);
            assert_eq!(provider.cycles.get(), 1);
        });
    }

    struct CapturedPackProvider;
    impl NativeTypeProvider for CapturedPackProvider {
        fn contains_function(&self, key: NativeFunctionKey) -> bool {
            key.snapshot == 402 && key.index == 0
        }
        fn function_parameters(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
            Vec::new()
        }
        fn function_declared_return(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Option<Type> {
            None
        }
        fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
            panic!("pack fixture")
        }
        fn function_has_result_pack(&self, _: NativeFunctionKey) -> bool {
            true
        }
        fn function_declared_return_pack(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Option<ValuePack> {
            None
        }
        fn function_inferred_return_pack(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> ValuePack {
            panic!("capture must be retained")
        }
        fn function_inferred_return_pack_in_capture(
            &self,
            _: NativeFunctionKey,
            context: &CapturedFunctionContext,
            queries: &mut dyn TypeQueries,
        ) -> ValuePack {
            fixed(
                (0..2)
                    .map(|index| {
                        queries
                            .captured_binding(
                                context,
                                crate::NativeBindingKey {
                                    snapshot: 402,
                                    index,
                                },
                            )
                            .unwrap()
                    })
                    .collect(),
            )
        }
        fn contains_binding(&self, key: crate::NativeBindingKey) -> bool {
            key.snapshot == 402 && key.index < 2
        }
        fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
            key.snapshot == 402 && key.index < 2
        }
        fn type_parameter_name(&self, key: NativeTypeParameterKey) -> String {
            if key.index == 0 { "T" } else { "U" }.into()
        }
        fn type_parameter_siblings(
            &self,
            _: NativeTypeParameterKey,
        ) -> Vec<NativeTypeParameterKey> {
            (0..2)
                .map(|index| NativeTypeParameterKey {
                    snapshot: 402,
                    index,
                })
                .collect()
        }
        fn return_cycle(&self, _: NativeFunctionKey) {
            panic!("acyclic capture")
        }
        fn return_type_too_deep(&self, _: NativeFunctionKey) {
            panic!("finite capture")
        }
    }

    #[test]
    fn native_captured_packs_apply_simultaneous_signature_substitution_once() {
        crate::with_program(|program, handles| {
            let provider = CapturedPackProvider;
            let adapter = crate::ProviderAdapter {
                provider: &provider,
                handles,
            };
            let mut checker = program.checker_with_native_provider(&adapter);
            checker.enable_native_arrays();
            let parameter = |index| {
                Type::NativeTypeParameter(NativeTypeParameterKey {
                    snapshot: 402,
                    index,
                })
            };
            let captured = SessionQueries {
                checker: &mut checker,
                handles,
            }
            .captured_function(
                NativeFunctionKey {
                    snapshot: 402,
                    index: 0,
                },
                &[
                    (
                        crate::NativeBindingKey {
                            snapshot: 402,
                            index: 0,
                        },
                        parameter(0),
                    ),
                    (
                        crate::NativeBindingKey {
                            snapshot: 402,
                            index: 1,
                        },
                        parameter(1),
                    ),
                ],
            );
            let raw = import_type(&mut checker, handles, &captured);
            let t = import_type(&mut checker, handles, &parameter(0));
            let u = import_type(&mut checker, handles, &parameter(1));
            let mapper = checker
                .types()
                .mapper(vec![(t, u), (u, bun_sema::types::TypeId::NUMBER)]);
            let mapped = checker.instantiate(raw, mapper);
            let mapped = export_type(&mut checker, handles, mapped);
            let mut queries = SessionQueries {
                checker: &mut checker,
                handles,
            };
            let result = queries.call_pack(&mapped, &[], &[], None);
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            assert_eq!(queries.pack_slot(&result.return_pack, 0), parameter(1));
            assert_eq!(queries.pack_slot(&result.return_pack, 1), Type::Number);
        });
    }
}
