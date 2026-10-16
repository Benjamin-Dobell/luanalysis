use crate::*;
use std::cell::Cell;
use std::num::NonZeroUsize;

const SNAPSHOT: u64 = 81932;

fn module(index: u32) -> NativeModuleKey { NativeModuleKey { snapshot: SNAPSHOT, index } }
fn alias(index: u32) -> NativeTypeDeclarationKey { NativeTypeDeclarationKey { snapshot: SNAPSHOT, index } }
fn declaration(index: u32) -> NativeValueDeclarationKey { NativeValueDeclarationKey { snapshot: SNAPSHOT, index } }
fn environment() -> NativeEnvironmentKey { NativeEnvironmentKey { snapshot: SNAPSHOT, index: 0 } }
fn binding() -> NativeGlobalBindingKey { NativeGlobalBindingKey { environment: environment(), index: 0 } }

fn payload() -> Type {
    Type::Shape(vec![
        Property::new("nested", Type::Shape(vec![Property::new("marker", Type::StringLiteral(b"published".to_vec()))])),
        Property::new("callback", Type::Function(FunctionType {
            params: vec![Parameter { name: "value".into(), ty: Type::Number, optional: false }],
            returns: Box::new(Type::Boolean),
        })),
    ])
}

struct PublishedProvider {
    valid: bool,
    modules: Cell<usize>,
    contracts: Cell<usize>,
    initializers: Cell<usize>,
    aliases: Cell<usize>,
}

impl PublishedProvider {
    fn new(valid: bool) -> Self {
        Self { valid, modules: Cell::new(0), contracts: Cell::new(0), initializers: Cell::new(0), aliases: Cell::new(0) }
    }
    fn counts(&self) -> [usize; 4] {
        [self.modules.get(), self.contracts.get(), self.initializers.get(), self.aliases.get()]
    }
}

impl NativeTypeProvider for PublishedProvider {
    fn contains_function(&self, _: NativeFunctionKey) -> bool { false }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { unreachable!() }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { unreachable!() }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type { unreachable!() }
    fn return_cycle(&self, _: NativeFunctionKey) { unreachable!() }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { unreachable!() }

    fn contains_module(&self, key: NativeModuleKey) -> bool { self.valid && key.snapshot == SNAPSHOT && key.index < 2 }
    fn module_header(&self, _: NativeModuleKey) -> NativeModuleHeader {
        NativeModuleHeader { role: ModuleSourceRole::Implementation }
    }
    fn module_inferred_result_pack(&self, key: NativeModuleKey, queries: &mut dyn TypeQueries) -> ValuePack {
        self.modules.set(self.modules.get() + 1);
        if key.index == 1 {
            return queries.module_result_pack(key).unwrap_or_else(|_| ValuePack::single(Type::Any));
        }
        ValuePack::Alternatives(vec![PackAlternative {
            fixed: vec![PackSlot::required(payload()), PackSlot::required(Type::StringLiteral(b"second".to_vec()))],
            tail: None,
        }])
    }

    fn contains_global_environment(&self, key: NativeEnvironmentKey) -> bool { self.valid && key == environment() }
    fn contains_global_binding(&self, key: NativeGlobalBindingKey) -> bool { self.valid && key == binding() }
    fn global_binding_declarations(&self, _: NativeGlobalBindingKey) -> Vec<NativeValueDeclarationKey> { vec![declaration(0)] }
    fn contains_value_declaration(&self, key: NativeValueDeclarationKey) -> bool { self.valid && key.snapshot == SNAPSHOT && key.index < 2 }
    fn value_declaration_header(&self, _: NativeValueDeclarationKey) -> ValueDeclarationHeader {
        ValueDeclarationHeader { role: GlobalSourceRole::Implementation, kind: GlobalValueKind::Variable }
    }
    fn value_declaration_contract(&self, _: NativeValueDeclarationKey, _: &mut dyn TypeQueries) -> Option<Type> {
        self.contracts.set(self.contracts.get() + 1);
        None
    }
    fn value_declaration_initializer(&self, key: NativeValueDeclarationKey, queries: &mut dyn TypeQueries) -> Type {
        self.initializers.set(self.initializers.get() + 1);
        if key.index == 1 { queries.value_declaration_type(key).unwrap_or(Type::Any) } else { payload() }
    }

    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool { self.valid && key.snapshot == SNAPSHOT && key.index < 2 }
    fn type_declaration_header(&self, _: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        TypeDeclarationHeader { kind: TypeDeclarationKind::Alias, name: "Published".into(), object_form: None, parameters: Vec::new(), outer_parameters: Vec::new() }
    }
    fn type_alias_target(&self, key: NativeTypeDeclarationKey, queries: &mut dyn TypeQueries) -> Type {
        self.aliases.set(self.aliases.get() + 1);
        if key.index == 1 { queries.named_type_reference(key, &[]).unwrap_or(Type::Any) } else { payload() }
    }
}

fn assert_complete(queries: &mut dyn TypeQueries, ty: &Type) {
    let nested = queries.property_type(ty, "nested").expect("published nested shape");
    let marker = queries.property_type(&nested, "marker").expect("published nested atom");
    assert_eq!(queries.inspect_type(&marker), Type::StringLiteral(b"published".to_vec()));
    let callback = queries.property_type(ty, "callback").expect("published signature");
    let called = queries.call(&callback, &[Type::Number]);
    assert!(called.errors.is_empty(), "published signature parameters remain readable");
    assert_eq!(queries.inspect_type(&called.return_type), Type::Boolean);
}

#[test]
fn completed_original_queries_publish_linked_types_to_fresh_parallel_tasks() {
    let run = Engine::new().with_native_task_plan_report(
        NonZeroUsize::new(2).unwrap(), &[vec![0], vec![1, 2]],
        |_| PublishedProvider::new(true),
        |_, provider, queries| {
            let required = queries.require_pack(module(0), NativeLoaderPolicy::Lua54StandardFile).unwrap();
            let value = queries.pack_slot(&required, 0);
            assert_complete(queries, &value);
            let original = queries.module_result_pack(module(0)).unwrap();
            let second = queries.pack_slot(&original, 1);
            assert_eq!(queries.inspect_type(&second), Type::StringLiteral(b"second".to_vec()));
            let global = queries.global_binding_type(binding()).unwrap();
            assert_complete(queries, &global);
            let named = queries.named_type_reference(alias(0), &[]).unwrap();
            assert_complete(queries, &named);
            provider.counts()
        },
    );
    assert_eq!(run.results, vec![[1, 1, 1, 1], [0, 0, 0, 0], [0, 0, 0, 0]]);
    assert_eq!((run.stats.workers, run.stats.tasks, run.stats.barriers), (2, 3, 2));
    assert!(run.stats.linked_records > 0);
    assert!(run.stats.published_entries > 0);
}

#[test]
fn published_answers_do_not_bypass_original_provider_membership() {
    let run = Engine::new().with_native_task_plan_report(
        NonZeroUsize::new(2).unwrap(), &[vec![0], vec![1, 2]],
        |job| PublishedProvider::new(job == 0),
        |job, provider, queries| {
            if job == 0 {
                queries.module_result_pack(module(0)).unwrap();
                queries.global_binding_type(binding()).unwrap();
                queries.named_type_reference(alias(0), &[]).unwrap();
            } else {
                assert_eq!(queries.module_result_pack(module(0)), Err(NativeProjectError::InvalidModule(module(0))));
                assert_eq!(queries.global_binding_type(binding()), Err(GlobalQueryError::InvalidEnvironment(environment())));
                assert_eq!(queries.named_type_reference(alias(0), &[]), Err(NamedTypeError::InvalidDeclaration(alias(0))));
            }
            provider.counts()
        },
    );
    assert_eq!(run.results, vec![[1, 1, 1, 1], [0, 0, 0, 0], [0, 0, 0, 0]]);
}

#[test]
fn circular_native_answers_remain_local_instead_of_publishing_fallback_types() {
    let run = Engine::new().with_native_task_plan_report(
        NonZeroUsize::new(2).unwrap(), &[vec![0], vec![1, 2]],
        |_| PublishedProvider::new(true),
        |_, provider, queries| {
            assert!(matches!(queries.module_result_pack(module(1)), Err(NativeProjectError::CircularQuery(_))));
            assert!(matches!(queries.value_declaration_type(declaration(1)), Err(GlobalQueryError::CircularQuery(_))));
            assert_eq!(queries.named_type_reference(alias(1), &[]), Err(NamedTypeError::CircularAlias(alias(1))));
            provider.counts()
        },
    );
    assert_eq!(run.results, vec![[1, 1, 1, 1], [1, 0, 1, 1], [1, 0, 1, 1]]);
}
