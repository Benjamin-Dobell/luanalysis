use crate::*;
use std::cell::{Cell, RefCell};

const SNAPSHOT: u64 = 718;
fn module(index: u32) -> NativeModuleKey {
    NativeModuleKey {
        snapshot: SNAPSHOT,
        index,
    }
}
fn function(index: u32) -> NativeFunctionKey {
    NativeFunctionKey {
        snapshot: SNAPSHOT,
        index,
    }
}
fn parameter() -> NativeTypeParameterKey {
    NativeTypeParameterKey {
        snapshot: SNAPSHOT,
        index: 0,
    }
}
fn binding() -> NativeBindingKey {
    NativeBindingKey {
        snapshot: SNAPSHOT,
        index: 0,
    }
}
fn fixed(types: Vec<Type>) -> ValuePack {
    ValuePack::Alternatives(vec![PackAlternative {
        fixed: types.into_iter().map(PackSlot::required).collect(),
        tail: None,
    }])
}

#[derive(Default)]
struct Modules {
    demands: Cell<usize>,
    errors: RefCell<Vec<NativeProjectError>>,
    context: RefCell<Option<ContextualFunctionContext>>,
    capture: RefCell<Option<CapturedFunctionContext>>,
}
impl NativeTypeProvider for Modules {
    fn contains_module(&self, key: NativeModuleKey) -> bool {
        (key.snapshot == SNAPSHOT && key.index < 2010)
            || (key.snapshot == SNAPSHOT + 1 && key.index == 0)
    }
    fn module_header(&self, key: NativeModuleKey) -> NativeModuleHeader {
        NativeModuleHeader {
            role: if key.index == 7 {
                ModuleSourceRole::Declaration
            } else {
                ModuleSourceRole::Implementation
            },
        }
    }
    fn module_inferred_result_pack(
        &self,
        key: NativeModuleKey,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        self.demands.set(self.demands.get() + 1);
        if key.snapshot == SNAPSHOT + 1 {
            return ValuePack::single(Type::Boolean);
        }
        match key.index {
            0 => fixed(vec![Type::Number, Type::String]),
            1 => ValuePack::empty(),
            2 => ValuePack::single(Type::Nil),
            3 => ValuePack::single(Type::BooleanLiteral(false)),
            4 => ValuePack::single(Type::Union(vec![Type::String, Type::Nil])),
            5 => ValuePack::single(Type::NativeFunction(function(0))),
            6 => ValuePack::single(
                queries.captured_function(function(1), &[(binding(), Type::Number)]),
            ),
            7 => panic!("declaration stub body must never execute"),
            8 => queries
                .module_result_pack(key)
                .unwrap_or_else(|_| ValuePack::single(Type::Any)),
            9 => queries
                .require_pack(module(10), NativeLoaderPolicy::Lua54StandardFile)
                .unwrap_or_else(|_| ValuePack::single(Type::Any)),
            10 => queries
                .require_pack(module(9), NativeLoaderPolicy::Lua54StandardFile)
                .unwrap_or_else(|_| ValuePack::single(Type::Any)),
            11 => ValuePack::never(),
            12 => ValuePack::single(Type::Any),
            13 => {
                assert_eq!(queries.current_contextual_function(), None);
                assert_eq!(queries.current_captured_function(), None);
                ValuePack::single(Type::String)
            }
            index if index >= 20 && index < 2009 => queries
                .module_result_pack(module(index + 1))
                .unwrap_or_else(|_| ValuePack::single(Type::Any)),
            _ => ValuePack::single(Type::Boolean),
        }
    }
    fn project_query_error(&self, error: NativeProjectError) {
        self.errors.borrow_mut().push(error);
    }
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        key.snapshot == SNAPSHOT && key.index < 2
    }
    fn function_parameters(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        if key.index == 0 {
            vec![Parameter {
                name: "value".into(),
                ty: Type::NativeTypeParameter(parameter()),
                optional: false,
            }]
        } else {
            vec![]
        }
    }
    fn function_declared_return(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        if key.index == 0 {
            Some(Type::NativeTypeParameter(parameter()))
        } else {
            None
        }
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("captured fixture requires immutable environment")
    }
    fn function_inferred_return_in_capture(
        &self,
        _: NativeFunctionKey,
        context: &CapturedFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        self.capture.replace(Some(*context));
        queries
            .captured_binding(context, binding())
            .expect("original capture retained")
    }
    fn contains_expression(&self, key: NativeExpressionKey) -> bool {
        key.snapshot == SNAPSHOT + 10 && key.node == 0
    }
    fn function_declared_return_in_context(
        &self,
        key: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        self.context.replace(Some(*context));
        self.function_declared_return(key, queries)
    }
    fn function_type_parameters(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<NativeTypeParameterKey> {
        if key.index == 0 {
            vec![parameter()]
        } else {
            vec![]
        }
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        key == parameter()
    }
    fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String {
        "T".into()
    }
    fn contains_binding(&self, key: NativeBindingKey) -> bool {
        key == binding()
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("module cycles do not borrow function return queries")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("module depth does not borrow function return queries")
    }
}

#[test]
fn module_chunk_queries_retain_whole_packs_and_cache_the_original_recipe() {
    Engine::new().with_native_provider(Modules::default, |provider, queries| {
        let first = queries.module_result_pack(module(0)).unwrap();
        assert_eq!(queries.pack_slot(&first, 0), Type::Number);
        assert_eq!(queries.pack_slot(&first, 1), Type::String);
        assert_eq!(queries.pack_slot(&first, 2), Type::Nil);
        assert_eq!(first, queries.module_result_pack(module(0)).unwrap());
        for policy in [
            NativeLoaderPolicy::Lua50StandardFile,
            NativeLoaderPolicy::Lua51StandardFile,
            NativeLoaderPolicy::Lua52StandardFile,
            NativeLoaderPolicy::Lua53StandardFile,
            NativeLoaderPolicy::Lua54StandardFile,
        ] {
            assert_eq!(queries.module_value(module(0), policy), Ok(Type::Number));
        }
        assert_eq!(provider.demands.get(), 1);
        assert!(provider.errors.borrow().is_empty());
    });
}

#[test]
fn module_values_discard_later_results_replace_only_nil_and_preserve_false() {
    Engine::new().with_native_provider(Modules::default, |_, queries| {
        for (index, expected) in [
            (0, Type::Number),
            (1, Type::BooleanLiteral(true)),
            (2, Type::BooleanLiteral(true)),
            (3, Type::BooleanLiteral(false)),
            (11, Type::Never),
            (12, Type::Any),
        ] {
            let actual = queries.module_value(module(index), NativeLoaderPolicy::Lua54StandardFile).unwrap();
            assert_eq!(queries.inspect_type(&actual), expected);
        }
        let optional = queries
            .module_value(module(4), NativeLoaderPolicy::Lua54StandardFile)
            .unwrap();
        assert!(queries.is_assignable(&Type::String, &optional));
        assert!(queries.is_assignable(&Type::BooleanLiteral(true), &optional));
        assert!(!queries.is_assignable(&Type::Nil, &optional));
        assert!(!queries.is_assignable(&Type::BooleanLiteral(false), &optional));
    });
}

#[test]
fn require_lua54_preserves_cache_hit_and_initial_loader_data_rows() {
    Engine::new().with_native_provider(Modules::default, |_, queries| {
        let pack = queries
            .require_pack(module(0), NativeLoaderPolicy::Lua54StandardFile)
            .unwrap();
        let rows = queries.inspect_pack(&pack).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .any(|row| row.fixed.len() == 1 && row.fixed[0].ty == Type::Number)
        );
        assert!(rows.iter().any(|row| row.fixed.len() == 2
            && row.fixed[0].ty == Type::Number
            && row.fixed[1].ty == Type::String));
        for policy in [
            NativeLoaderPolicy::Lua50StandardFile,
            NativeLoaderPolicy::Lua51StandardFile,
            NativeLoaderPolicy::Lua52StandardFile,
            NativeLoaderPolicy::Lua53StandardFile,
        ] {
            let pack = queries.require_pack(module(0), policy).unwrap();
            let rows = queries.inspect_pack(&pack).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].fixed.len(), 1);
        }
    });
}

#[test]
fn exported_generic_and_captured_functions_keep_exact_native_identity() {
    Engine::new().with_native_provider(Modules::default, |_, queries| {
        let generic = queries
            .module_value(module(5), NativeLoaderPolicy::Lua54StandardFile)
            .unwrap();
        assert_eq!(queries.native_function_key(&generic), Some(function(0)));
        for ty in [Type::Number, Type::String, Type::Boolean] {
            let call = queries.call(&generic, &[ty.clone()]);
            assert!(call.errors.is_empty(), "{:?}", call.errors);
            assert_eq!(call.return_type, ty);
        }
        let closure = queries
            .module_value(module(6), NativeLoaderPolicy::Lua54StandardFile)
            .unwrap();
        assert_eq!(queries.native_function_key(&closure), Some(function(1)));
        let call = queries.call(&closure, &[]);
        assert!(call.errors.is_empty());
        assert_eq!(call.return_type, Type::Number);
        assert_eq!(
            queries
                .module_value(module(6), NativeLoaderPolicy::Lua54StandardFile)
                .unwrap(),
            closure
        );
    });
}

#[test]
fn circular_module_fallback_any_is_never_published_as_a_finished_export() {
    Engine::new().with_native_provider(Modules::default, |provider, queries| {
        for index in [8, 9, 10] {
            for _ in 0..2 {
                let result = queries.module_result_pack(module(index));
                assert!(
                    matches!(result, Err(NativeProjectError::CircularQuery(_))),
                    "{result:?}"
                );
                let result =
                    queries.module_value(module(index), NativeLoaderPolicy::Lua54StandardFile);
                assert!(
                    matches!(result, Err(NativeProjectError::CircularQuery(_))),
                    "{result:?}"
                );
            }
        }
        assert!(
            provider
                .errors
                .borrow()
                .iter()
                .any(|error| matches!(error, NativeProjectError::CircularQuery(_)))
        );
    });
}

#[test]
fn module_queries_reject_wrong_source_keys_declaration_execution_and_unknown_loaders() {
    Engine::new().with_native_provider(Modules::default, |provider, queries| {
        for invalid in [
            NativeModuleKey {
                snapshot: SNAPSHOT + 2,
                index: 0,
            },
            module(2010),
        ] {
            assert_eq!(
                queries.module_result_pack(invalid),
                Err(NativeProjectError::InvalidModule(invalid))
            );
        }
        assert_eq!(
            queries.module_result_pack(module(7)),
            Err(NativeProjectError::MissingDeclarationContract(module(7)))
        );
        assert_eq!(
            queries.module_value(module(0), NativeLoaderPolicy::Unsupported),
            Err(NativeProjectError::UnsupportedLoader {
                module: module(0),
                policy: NativeLoaderPolicy::Unsupported
            })
        );
        assert_eq!(provider.demands.get(), 0);
    });
}

#[test]
fn module_depth_refusal_does_not_cache_fallback_any_or_damage_later_queries() {
    Engine::new().with_native_provider(Modules::default, |provider, queries| {
        for _ in 0..2 {
            assert!(queries.module_result_pack(module(20)).is_err());
        }
        assert!(
            provider
                .errors
                .borrow()
                .iter()
                .any(|error| matches!(error, NativeProjectError::TooDeep(_)))
        );
        assert_eq!(
            queries.module_value(module(0), NativeLoaderPolicy::Lua54StandardFile),
            Ok(Type::Number)
        );
    });
}

#[test]
fn module_chunks_isolate_importer_context_and_capture_frames_then_restore_them() {
    Engine::new().with_native_provider(Modules::default, |provider, queries| {
        let closure = queries.captured_function(function(1), &[(binding(), Type::Number)]);
        assert_eq!(queries.call(&closure, &[]).return_type, Type::Number);
        let capture = provider.capture.borrow().expect("capture body ran");
        let expected = Type::Function(FunctionType {
            params: vec![Parameter {
                name: "value".into(),
                ty: Type::Number,
                optional: false,
            }],
            returns: Box::new(Type::Number),
        });
        let contextual = queries.contextual_function(
            function(0),
            NativeExpressionKey {
                snapshot: SNAPSHOT + 10,
                node: 0,
            },
            &expected,
            NativeCheckMode::CONTEXTUAL,
        );
        let call = queries.call(&contextual, &[Type::Number]);
        assert!(call.errors.is_empty(), "{:?}", call.errors);
        let context = provider.context.borrow().expect("contextual body ran");
        queries.with_contextual_function(&context, &mut |queries| {
            queries.with_captured_function(&capture, &mut |queries| {
                assert_eq!(queries.current_contextual_function(), Some(context));
                assert_eq!(queries.current_captured_function(), Some(capture));
                let pack = queries.module_result_pack(module(13)).unwrap();
                assert_eq!(queries.pack_slot(&pack, 0), Type::String);
                assert_eq!(queries.current_contextual_function(), Some(context));
                assert_eq!(queries.current_captured_function(), Some(capture));
            });
        });
        assert_eq!(queries.current_contextual_function(), None);
        assert_eq!(queries.current_captured_function(), None);
    });
}

#[test]
fn module_cache_separates_equal_chunk_indices_in_different_source_snapshots() {
    Engine::new().with_native_provider(Modules::default, |provider, queries| {
        let other = NativeModuleKey {
            snapshot: SNAPSHOT + 1,
            index: 0,
        };
        for _ in 0..2 {
            assert_eq!(
                queries.module_value(module(0), NativeLoaderPolicy::Lua54StandardFile),
                Ok(Type::Number)
            );
            assert_eq!(
                queries.module_value(other, NativeLoaderPolicy::Lua54StandardFile),
                Ok(Type::Boolean)
            );
        }
        assert_eq!(provider.demands.get(), 2);
    });
}
