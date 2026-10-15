use super::*;
use std::cell::{Cell, RefCell};

const SNAPSHOT: u64 = 11_085;
fn environment() -> NativeEnvironmentKey {
    NativeEnvironmentKey {
        snapshot: SNAPSHOT,
        index: 0,
    }
}
fn binding(index: u32) -> NativeGlobalBindingKey {
    NativeGlobalBindingKey {
        environment: environment(),
        index,
    }
}
fn declaration(index: u32) -> NativeValueDeclarationKey {
    NativeValueDeclarationKey {
        snapshot: SNAPSHOT,
        index,
    }
}
fn signature(index: u32) -> NativeSignatureKey {
    NativeSignatureKey {
        snapshot: SNAPSHOT,
        index,
    }
}
fn parameter(index: u32) -> NativeTypeParameterKey {
    NativeTypeParameterKey {
        snapshot: SNAPSHOT,
        index,
    }
}
fn expression(node: u32) -> NativeExpressionKey {
    NativeExpressionKey {
        snapshot: SNAPSHOT,
        node,
    }
}
fn function() -> NativeFunctionKey {
    NativeFunctionKey {
        snapshot: SNAPSHOT,
        index: 0,
    }
}
fn fixed(types: &[Type]) -> ValuePack {
    ValuePack::Alternatives(vec![PackAlternative {
        fixed: types.iter().cloned().map(PackSlot::required).collect(),
        tail: None,
    }])
}
fn callback() -> Type {
    Type::Function(FunctionType {
        params: vec![Parameter {
            name: "text".into(),
            ty: Type::String,
            optional: false,
        }],
        returns: Box::new(Type::String),
    })
}

struct Asserts {
    tag: Option<NativeIntrinsicSite>,
    missing_context: Cell<usize>,
    modes: RefCell<Vec<NativeCheckMode>>,
    errors: RefCell<Vec<GlobalQueryError>>,
    written_symbolic_constraint: RefCell<Option<Type>>,
}

#[test]
fn provenance_only_intrinsics_retain_original_identity_without_assert_result_effects() {
    for kind in [NativeIntrinsicKind::Type, NativeIntrinsicKind::Require, NativeIntrinsicKind::SetMetatable] {
        Engine::new().with_native_provider(
            || Asserts {
                tag: Some(NativeIntrinsicSite { binding: binding(0), declaration: declaration(0), kind }),
                ..Asserts::default()
            },
            |_, queries| {
                let original = queries.global_binding_type(binding(0)).unwrap();
                let site = queries.function_intrinsic(&original).unwrap();
                assert_eq!(site.kind, kind);
                let custom = queries.global_binding_type(binding(1)).unwrap();
                assert_eq!(queries.function_intrinsic(&custom), None);
                let mixed = Type::Union(vec![original.clone(), custom]);
                assert_eq!(queries.function_intrinsic(&mixed), None);
                let result = queries.call(&original, &[Type::BooleanLiteral(false)]);
                assert!(result.errors.is_empty(), "{:?}", result.errors);
                assert_ne!(result.return_type, Type::Never, "provenance cannot acquire assert(false)'s effect");
            },
        );
    }
}
impl Default for Asserts {
    fn default() -> Self {
        Self {
            tag: Some(NativeIntrinsicSite {
                binding: binding(0),
                declaration: declaration(0),
                kind: NativeIntrinsicKind::Assert,
            }),
            missing_context: Cell::new(0),
            modes: RefCell::new(vec![]),
            errors: RefCell::new(vec![]),
            written_symbolic_constraint: RefCell::new(None),
        }
    }
}
impl NativeTypeProvider for Asserts {
    fn contains_global_environment(&self, key: NativeEnvironmentKey) -> bool {
        key == environment()
    }
    fn contains_global_binding(&self, key: NativeGlobalBindingKey) -> bool {
        key.environment == environment() && key.index < 2
    }
    fn global_binding_declarations(
        &self,
        key: NativeGlobalBindingKey,
    ) -> Vec<NativeValueDeclarationKey> {
        vec![declaration(key.index)]
    }
    fn contains_value_declaration(&self, key: NativeValueDeclarationKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 2
    }
    fn value_declaration_header(&self, _: NativeValueDeclarationKey) -> ValueDeclarationHeader {
        ValueDeclarationHeader {
            role: GlobalSourceRole::Declaration,
            kind: GlobalValueKind::Function,
        }
    }
    fn value_declaration_contract(
        &self,
        key: NativeValueDeclarationKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        Some(queries.signature_group_type(&[signature(key.index)]))
    }
    fn value_declaration_initializer(
        &self,
        _: NativeValueDeclarationKey,
        _: &mut dyn TypeQueries,
    ) -> Type {
        panic!("original declaration stub must never be inferred")
    }
    fn global_query_error(&self, error: GlobalQueryError, _: &mut dyn TypeQueries) {
        self.errors.borrow_mut().push(error);
    }
    fn contains_signature(&self, key: NativeSignatureKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 3
    }
    fn signature_intrinsic(&self, key: NativeSignatureKey) -> Option<NativeIntrinsicSite> {
        (key == signature(0)).then_some(self.tag).flatten()
    }
    fn signature_header(&self, key: NativeSignatureKey) -> SignatureHeader {
        SignatureHeader {
            owner: SignatureOwner::Standalone,
            site: SignatureSite {
                snapshot: SNAPSHOT,
                index: key.index,
            },
            parent: None,
            has_direct_literal_parameter: false,
        }
    }
    fn signature_type_parameters(&self, key: NativeSignatureKey) -> Vec<NativeTypeParameterKey> {
        vec![parameter(key.index)]
    }
    fn signature_parameters(
        &self,
        key: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        vec![Parameter {
            name: "value".into(),
            ty: Type::NativeTypeParameter(parameter(key.index)),
            optional: true,
        }]
    }
    fn signature_rest_pack(
        &self,
        _: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> Option<ValuePack> {
        Some(ValuePack::variadic(Type::Unknown))
    }
    fn signature_declared_return_pack(
        &self,
        key: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> ValuePack {
        ValuePack::single(Type::NativeTypeParameter(parameter(key.index)))
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 3
    }
    fn type_parameter_owner(&self, key: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> {
        Some(TypeDeclarationOwner::Signature(signature(key.index)))
    }
    fn type_parameter_name(&self, key: NativeTypeParameterKey) -> String {
        format!("T{}", key.index)
    }
    fn type_parameter_siblings(&self, key: NativeTypeParameterKey) -> Vec<NativeTypeParameterKey> {
        vec![key]
    }
    fn has_type_parameter_constraint(&self, key: NativeTypeParameterKey) -> bool {
        key == parameter(2) && self.written_symbolic_constraint.borrow().is_some()
    }
    fn type_parameter_constraint(
        &self,
        key: NativeTypeParameterKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        (key == parameter(2))
            .then(|| self.written_symbolic_constraint.borrow().clone())
            .flatten()
    }
    fn contains_expression(&self, key: NativeExpressionKey) -> bool {
        key.snapshot == SNAPSHOT && key.node < 3
    }
    fn is_context_sensitive(&self, key: NativeExpressionKey) -> bool {
        key.node != 0
    }
    fn has_context_sensitive_parameters(&self, key: NativeExpressionKey) -> bool {
        key.node != 0
    }
    fn check_expression(
        &self,
        key: NativeExpressionKey,
        expected: Option<&Type>,
        mode: NativeCheckMode,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        if key.node == 0 {
            return Type::BooleanLiteral(true);
        }
        assert_eq!(key.node, 1);
        self.modes.borrow_mut().push(mode);
        let publishing = !mode.intersects(
            NativeCheckMode::INFERENTIAL
                | NativeCheckMode::SKIP_CONTEXT_SENSITIVE
                | NativeCheckMode::SKIP_GENERIC_FUNCTIONS
                | NativeCheckMode::TYPE_ONLY
                | NativeCheckMode::IS_FOR_SIGNATURE_HELP,
        );
        if publishing && expected.is_none_or(|ty| queries.call_signature_types(ty).is_empty()) {
            self.missing_context.set(self.missing_context.get() + 1);
        }
        expected.map_or(Type::NativeFunction(function()), |expected| {
            queries.contextual_function(function(), key, expected, mode)
        })
    }
    fn check_expression_pack(
        &self,
        key: NativeExpressionKey,
        expected: Option<&ValuePack>,
        mode: NativeCheckMode,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        if key.node != 2 {
            let contextual = expected.and_then(|pack| queries.pack_contextual_slot(pack, 0));
            return ValuePack::single(self.check_expression(
                key,
                contextual.as_ref(),
                mode,
                queries,
            ));
        }
        let contextual = expected.and_then(|pack| queries.pack_contextual_slot(pack, 1));
        let callable = self.check_expression(expression(1), contextual.as_ref(), mode, queries);
        fixed(&[Type::BooleanLiteral(true), callable])
    }
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        key == function()
    }
    fn contextual_parameter_indices(&self, _: NativeFunctionKey) -> Vec<usize> {
        vec![0]
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        vec![Parameter {
            name: "text".into(),
            ty: Type::Unknown,
            optional: false,
        }]
    }
    fn function_parameters_in_context(
        &self,
        _: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        vec![Parameter {
            name: "text".into(),
            ty: queries
                .contextual_parameter_type(context, 0)
                .unwrap_or(Type::Unknown),
            optional: false,
        }]
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        None
    }
    fn function_inferred_return(
        &self,
        _: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        queries
            .current_contextual_function()
            .and_then(|context| queries.contextual_parameter_type(&context, 0))
            .unwrap_or(Type::Unknown)
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("callback has no recursive return")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("callback is finite")
    }
}

#[test]
fn assert_returns_every_original_argument_without_literal_widening() {
    Engine::new().with_native_provider(Asserts::default, |provider, queries| {
        let callee = queries.global_binding_type(binding(0)).unwrap();
        let arguments = [
            Type::NumberLiteral(0.0),
            Type::StringLiteral(b"message".to_vec()),
            Type::BooleanLiteral(false),
            Type::Nil,
        ];
        let result = queries.call_pack(&callee, &arguments, &[], None);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(queries.pack_assignable(&result.return_pack, &fixed(&arguments)));
        assert!(queries.pack_assignable(&fixed(&arguments), &result.return_pack));
        assert_eq!(result.return_type, Type::NumberLiteral(0.0));
        assert!(provider.errors.borrow().is_empty());
    });
}

#[test]
fn assert_filters_false_nil_and_empty_rows_but_preserves_successful_correlations() {
    Engine::new().with_native_provider(Asserts::default, |_, queries| {
        let callee = queries.global_binding_type(binding(0)).unwrap();
        let input = queries.join_packs(&[
            fixed(&[
                Type::BooleanLiteral(false),
                Type::StringLiteral(b"failed".to_vec()),
            ]),
            fixed(&[Type::Number, callback()]),
            fixed(&[Type::Nil, Type::Boolean]),
            ValuePack::empty(),
        ]);
        let result =
            queries.call_native_arguments(&callee, None, &[PackArgument::Expand(input)], &[], None);
        if let Some(CallError::Argument { source, target, .. }) = result.errors.first() {
            panic!("{} -> {}", queries.format(source), queries.format(target));
        }
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        let expected = fixed(&[Type::Number, callback()]);
        assert!(queries.pack_assignable(&result.return_pack, &expected));
        assert!(queries.pack_assignable(&expected, &result.return_pack));
    });
}

#[test]
fn assert_makes_a_successful_variadic_first_slot_present_and_keeps_the_tail() {
    Engine::new().with_native_provider(Asserts::default, |_, queries| {
        let callee = queries.global_binding_type(binding(0)).unwrap();
        let element = Type::Union(vec![Type::Number, Type::Boolean, Type::Nil]);
        let result = queries.call_native_arguments(
            &callee,
            None,
            &[PackArgument::Expand(ValuePack::variadic(element.clone()))],
            &[],
            None,
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        let rows = queries.inspect_pack(&result.return_pack).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].fixed.len(), 1);
        assert!(!rows[0].fixed[0].optional);
        assert!(queries.is_assignable(
            &rows[0].fixed[0].ty,
            &Type::Union(vec![Type::Number, Type::BooleanLiteral(true)])
        ));
        assert!(!queries.is_assignable(&Type::BooleanLiteral(false), &rows[0].fixed[0].ty));
        assert!(!queries.is_assignable(&Type::Nil, &rows[0].fixed[0].ty));
        assert!(queries.is_assignable(rows[0].tail.as_ref().unwrap(), &element));
        assert!(queries.is_assignable(&element, rows[0].tail.as_ref().unwrap()));
    });
}

#[test]
fn assert_false_nil_zero_results_and_no_arguments_have_no_successful_result() {
    Engine::new().with_native_provider(Asserts::default, |_, queries| {
        let callee = queries.global_binding_type(binding(0)).unwrap();
        for arguments in [
            vec![],
            vec![PackArgument::Resolved(Type::BooleanLiteral(false))],
            vec![PackArgument::Resolved(Type::Nil)],
            vec![PackArgument::Expand(ValuePack::empty())],
        ] {
            let result = queries.call_native_arguments(&callee, None, &arguments, &[], None);
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            assert_eq!(result.return_type, Type::Never);
            assert_eq!(queries.inspect_pack(&result.return_pack).unwrap().len(), 0);
        }
    });
}

#[test]
fn untagged_original_lookalikes_and_missing_policy_use_only_their_written_contract() {
    Engine::new().with_native_provider(
        || Asserts {
            tag: None,
            ..Asserts::default()
        },
        |_, queries| {
            let callee = queries.global_binding_type(binding(0)).unwrap();
            let result = queries.call_pack(
                &callee,
                &[Type::BooleanLiteral(false), Type::String],
                &[],
                None,
            );
            assert!(result.errors.is_empty());
            assert_ne!(result.return_type, Type::Never);
            assert_eq!(
                queries.inspect_pack(&result.return_pack).unwrap()[0]
                    .fixed
                    .len(),
                1
            );
        },
    );
    Engine::new().with_native_provider(Asserts::default, |_, queries| {
        let lookalike = queries.global_binding_type(binding(1)).unwrap();
        let result = queries.call_pack(&lookalike, &[Type::BooleanLiteral(false)], &[], None);
        assert!(result.errors.is_empty());
        assert_ne!(result.return_type, Type::Never);
    });
}

#[test]
fn assert_uses_outer_result_context_for_an_original_callback_argument() {
    Engine::new().with_native_provider(Asserts::default, |provider, queries| {
        let callee = queries.global_binding_type(binding(0)).unwrap();
        let expected = fixed(&[Type::BooleanLiteral(true), callback()]);
        let result = queries.call_native_arguments(
            &callee,
            None,
            &[
                PackArgument::Expression(expression(0)),
                PackArgument::Expression(expression(1)),
            ],
            &[],
            Some(&expected),
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        let returned = queries.pack_slot(&result.return_pack, 1);
        assert_eq!(
            queries.call(&returned, &[Type::String]).return_type,
            Type::String
        );
        assert!(matches!(
            queries.call(&returned, &[Type::Number]).errors.as_slice(),
            [CallError::Argument { index: 0, .. }]
        ));
        assert_eq!(provider.missing_context.get(), 0);
    });
}

#[test]
fn expanded_original_callback_pack_arity_probes_do_not_publish_an_untyped_facet() {
    Engine::new().with_native_provider(Asserts::default, |provider, queries| {
        let callee = queries.global_binding_type(binding(0)).unwrap();
        let expected = fixed(&[Type::BooleanLiteral(true), callback()]);
        let result = queries.call_native_arguments(
            &callee,
            None,
            &[PackArgument::ExpandExpression(expression(2))],
            &[],
            Some(&expected),
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        let returned = queries.pack_slot(&result.return_pack, 1);
        assert_eq!(
            queries.call(&returned, &[Type::String]).return_type,
            Type::String
        );
        assert_eq!(provider.missing_context.get(), 0);
        assert!(
            provider
                .modes
                .borrow()
                .iter()
                .any(|mode| mode.contains(NativeCheckMode::TYPE_ONLY))
        );
    });
}

#[test]
fn intrinsic_metadata_must_belong_to_the_selected_original_global_declaration() {
    for site in [
        NativeIntrinsicSite {
            binding: binding(1),
            declaration: declaration(0),
            kind: NativeIntrinsicKind::Assert,
        },
        NativeIntrinsicSite {
            binding: binding(9),
            declaration: declaration(0),
            kind: NativeIntrinsicKind::Assert,
        },
        NativeIntrinsicSite {
            binding: binding(0),
            declaration: declaration(9),
            kind: NativeIntrinsicKind::Assert,
        },
    ] {
        Engine::new().with_native_provider(
            || Asserts {
                tag: Some(site),
                ..Asserts::default()
            },
            |provider, queries| {
                let callee = queries.global_binding_type(binding(0)).unwrap();
                let result = queries.call_pack(&callee, &[Type::BooleanLiteral(false)], &[], None);
                assert_ne!(result.return_type, Type::Never);
                assert!(!provider.errors.borrow().is_empty());
            },
        );
    }
}

#[test]
fn ordinary_type_argument_and_parameter_errors_precede_intrinsic_reduction() {
    Engine::new().with_native_provider(Asserts::default, |_, queries| {
        let callee = queries.global_binding_type(binding(0)).unwrap();
        let result = queries.call_pack(&callee, &[Type::String], &[Type::Number], None);
        assert!(matches!(
            result.errors.as_slice(),
            [CallError::Argument { index: 0, .. }]
        ));
    });
}

#[test]
fn assert_retains_a_fixed_first_result_and_an_original_symbolic_variadic_tail() {
    with_program(|program, handles| {
        let provider = Asserts::default();
        let adapter = ProviderAdapter {
            provider: &provider,
            handles,
        };
        let mut checker = program.checker_with_native_provider(&adapter);
        checker.set_native_globals_provider(&adapter);
        checker.enable_native_arrays();
        // Original fixture declaration: P extends unknown[]. This is a written
        // constraint capability, never an inferred source-type cache.
        let array = checker.array_of(TypeId::UNKNOWN);
        *provider.written_symbolic_constraint.borrow_mut() =
            Some(export_type(&mut checker, handles, array));
        let callee = import_type(&mut checker, handles, &Type::NativeSignature(signature(0)));
        let first = import_type(
            &mut checker,
            handles,
            &Type::Union(vec![Type::BooleanLiteral(false), Type::Number]),
        );
        let symbolic = import_type(
            &mut checker,
            handles,
            &Type::NativeTypeParameter(parameter(2)),
        );
        let tuple = checker.tuple(
            &[first, symbolic],
            &[
                bun_sema::types::ElemFlags::REQUIRED,
                bun_sema::types::ElemFlags::VARIADIC,
            ],
            false,
        );
        let rest = SigParam {
            name: checker.atoms().intern(b"values"),
            ty: tuple,
            optional: false,
            rest: true,
            is_required_rest: false,
            declaration: None,
        };
        let pack_signature = checker.types().intern_sig(SigData::Synth {
            type_params: checker.list(&[]),
            params: checker.list(&[rest]),
            ret: TypeId::UNKNOWN,
            this: None,
            of: checker.list(&[]),
            is_union: false,
        });
        let input = checker.signature_rest_pack(pack_signature).unwrap();
        assert!(checker.native_pack_alternatives(input).is_none());
        let original_type = checker.native_pack_type(input);
        let retained = handles.retain_pack(&checker, input);
        {
            let mut q = SessionQueries { checker: &mut checker, handles };
            let PackInspection::Symbolic(exact) = q.inspect_pack_view(&retained) else { panic!("original symbolic pack") };
            assert_eq!(import_type(q.checker, handles, &exact), original_type);
            let displayed = q.format(&exact);
            assert!(displayed.contains("T2") && displayed.contains("...") && displayed.contains("number"), "{displayed}");
            assert!(!displayed.contains("Session") && !displayed.contains("uninstantiated"), "{displayed}");
            assert!(q.inspect_pack(&retained).is_none());
        }
        let result = checker.call_native_arguments(
            callee,
            None,
            &[bun_sema::check::native_packs::NativeCallArgument::Expand(
                input,
            )],
            &[],
            None,
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        let result_type = checker.native_pack_type(result.pack);
        let elements = checker.type_arguments(result_type);
        assert_eq!(elements.first(), Some(&TypeId::NUMBER));
        assert_eq!(elements.last(), Some(&symbolic));
        assert!(
            matches!(checker.data(result_type), TypeData::Tuple { flags, .. } if flags.last().unwrap().contains(bun_sema::types::ElemFlags::VARIADIC))
        );
    });
}
