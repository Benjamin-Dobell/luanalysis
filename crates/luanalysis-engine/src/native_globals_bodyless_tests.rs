use super::*;
use crate::{
    Engine, NativeFunctionKey, NativeSignatureKey, NativeTypeParameterKey, Parameter,
    SignatureHeader, SignatureOwner, SignatureSite, TypeDeclarationOwner, TypeQueries, ValuePack,
};
use std::cell::{Cell, RefCell};

fn declaration(snapshot: u64, index: u32) -> NativeValueDeclarationKey {
    NativeValueDeclarationKey { snapshot, index }
}
fn signature(snapshot: u64, index: u32) -> NativeSignatureKey {
    NativeSignatureKey { snapshot, index }
}
fn parameter(key: NativeSignatureKey) -> NativeTypeParameterKey {
    NativeTypeParameterKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn binding() -> NativeGlobalBindingKey {
    NativeGlobalBindingKey {
        environment: NativeEnvironmentKey {
            snapshot: 31,
            index: 0,
        },
        index: 0,
    }
}

struct Head {
    key: NativeSignatureKey,
    input: Type,
    output: Type,
    generic: bool,
}
struct Declaration {
    key: NativeValueDeclarationKey,
    heads: Option<Vec<NativeSignatureKey>>,
    role: GlobalSourceRole,
}
struct Provider {
    declarations: Vec<Declaration>,
    heads: Vec<Head>,
    owners: Vec<NativeValueDeclarationKey>,
    errors: RefCell<Vec<GlobalQueryError>>,
    signature_reads: Cell<usize>,
}
impl Provider {
    fn head(&self, key: NativeSignatureKey) -> &Head {
        self.heads
            .iter()
            .find(|head| head.key == key)
            .expect("original signature")
    }
    fn declaration(&self, key: NativeValueDeclarationKey) -> &Declaration {
        self.declarations
            .iter()
            .find(|declaration| declaration.key == key)
            .expect("original declaration")
    }
    fn new(declarations: Vec<Declaration>, heads: Vec<Head>) -> Self {
        let owners = declarations
            .iter()
            .map(|declaration| declaration.key)
            .rev()
            .collect();
        Self {
            declarations,
            heads,
            owners,
            errors: RefCell::new(Vec::new()),
            signature_reads: Cell::new(0),
        }
    }
}
impl NativeTypeProvider for Provider {
    fn contains_global_environment(&self, key: NativeEnvironmentKey) -> bool {
        key == binding().environment
    }
    fn contains_global_binding(&self, key: NativeGlobalBindingKey) -> bool {
        key == binding()
    }
    fn global_binding_declarations(
        &self,
        _: NativeGlobalBindingKey,
    ) -> Vec<NativeValueDeclarationKey> {
        self.owners.clone()
    }
    fn contains_value_declaration(&self, key: NativeValueDeclarationKey) -> bool {
        self.declarations
            .iter()
            .any(|declaration| declaration.key == key)
    }
    fn value_declaration_header(&self, key: NativeValueDeclarationKey) -> ValueDeclarationHeader {
        ValueDeclarationHeader {
            role: self.declaration(key).role,
            kind: GlobalValueKind::Function,
        }
    }
    fn value_declaration_signatures(
        &self,
        key: NativeValueDeclarationKey,
    ) -> Option<Vec<NativeSignatureKey>> {
        self.declaration(key).heads.clone()
    }
    fn value_declaration_contract(
        &self,
        _: NativeValueDeclarationKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        panic!("pure callable contracts use original signature keys")
    }
    fn value_declaration_initializer(
        &self,
        _: NativeValueDeclarationKey,
        _: &mut dyn TypeQueries,
    ) -> Type {
        panic!("pure callable has no initializer")
    }
    fn global_query_error(&self, error: GlobalQueryError, _: &mut dyn TypeQueries) {
        self.errors.borrow_mut().push(error);
    }
    fn contains_signature(&self, key: NativeSignatureKey) -> bool {
        self.heads.iter().any(|head| head.key == key)
    }
    fn signature_header(&self, key: NativeSignatureKey) -> SignatureHeader {
        self.signature_reads.set(self.signature_reads.get() + 1);
        SignatureHeader {
            owner: SignatureOwner::Standalone,
            site: SignatureSite {
                snapshot: key.snapshot,
                index: key.index,
            },
            parent: None,
            has_direct_literal_parameter: matches!(
                self.head(key).input,
                Type::StringLiteral(_) | Type::NumberLiteral(_) | Type::BooleanLiteral(_)
            ),
        }
    }
    fn signature_parameters(
        &self,
        key: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        vec![Parameter {
            name: "value".into(),
            ty: self.head(key).input.clone(),
            optional: false,
        }]
    }
    fn signature_declared_return_pack(
        &self,
        key: NativeSignatureKey,
        _: &mut dyn TypeQueries,
    ) -> ValuePack {
        ValuePack::single(self.head(key).output.clone())
    }
    fn signature_type_parameters(&self, key: NativeSignatureKey) -> Vec<NativeTypeParameterKey> {
        self.head(key)
            .generic
            .then(|| parameter(key))
            .into_iter()
            .collect()
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        self.heads
            .iter()
            .any(|head| head.generic && parameter(head.key) == key)
    }
    fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String {
        "T".into()
    }
    fn type_parameter_owner(&self, key: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> {
        self.heads
            .iter()
            .find(|head| head.generic && parameter(head.key) == key)
            .map(|head| TypeDeclarationOwner::Signature(head.key))
    }
    fn contains_function(&self, _: NativeFunctionKey) -> bool {
        false
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        panic!("no runtime function")
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        panic!("no runtime function")
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("no runtime function")
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("no runtime function")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("no runtime function")
    }
}
fn pure(snapshot: u64, heads: Vec<NativeSignatureKey>) -> Declaration {
    Declaration {
        key: declaration(snapshot, 0),
        heads: Some(heads),
        role: GlobalSourceRole::Declaration,
    }
}
fn head(snapshot: u64, input: Type, output: Type) -> Head {
    Head {
        key: signature(snapshot, 0),
        input,
        output,
        generic: false,
    }
}
fn call(queries: &mut dyn TypeQueries, callable: &Type, argument: Type) -> Type {
    let called = queries.call_pack(callable, &[argument], &[], None);
    assert!(called.errors.is_empty(), "{:#?}", called.errors);
    queries.pack_slot(&called.return_pack, 0)
}

#[test]
fn pure_global_merges_original_cross_snapshot_heads_without_runtime_identity() {
    Engine::new().with_native_provider(
        || {
            Provider::new(
                vec![
                    pure(31, vec![signature(31, 0)]),
                    pure(42, vec![signature(42, 0)]),
                ],
                vec![
                    head(31, Type::Number, Type::String),
                    head(42, Type::String, Type::Boolean),
                ],
            )
        },
        |provider, queries| {
            let callable = queries.global_binding_type(binding()).unwrap();
            assert_eq!(queries.function_implementation_type(&callable), None);
            assert_eq!(call(queries, &callable, Type::Number), Type::String);
            assert_eq!(call(queries, &callable, Type::String), Type::Boolean);
            assert!(
                !queries
                    .call_pack(&callable, &[Type::Boolean], &[], None)
                    .errors
                    .is_empty()
            );
            let original = queries.value_declaration_type(declaration(31, 0)).unwrap();
            assert_eq!(call(queries, &original, Type::Number), Type::String);
            assert!(
                !queries
                    .call_pack(&original, &[Type::String], &[], None)
                    .errors
                    .is_empty()
            );
            assert!(provider.errors.borrow().is_empty());
        },
    );
}

#[test]
fn pure_global_preserves_each_original_generic_scope() {
    let generic = signature(42, 0);
    Engine::new().with_native_provider(
        || {
            Provider::new(
                vec![pure(31, vec![signature(31, 0)]), pure(42, vec![generic])],
                vec![
                    head(31, Type::String, Type::Boolean),
                    Head {
                        key: generic,
                        input: Type::NativeTypeParameter(parameter(generic)),
                        output: Type::NativeTypeParameter(parameter(generic)),
                        generic: true,
                    },
                ],
            )
        },
        |provider, queries| {
            let callable = queries.global_binding_type(binding()).unwrap();
            assert_eq!(call(queries, &callable, Type::Number), Type::Number);
            assert_eq!(call(queries, &callable, Type::String), Type::Boolean);
            assert!(provider.errors.borrow().is_empty());
        },
    );
}

#[test]
fn pure_contract_never_demands_body_even_with_implementation_source_role() {
    Engine::new().with_native_provider(
        || {
            let mut declaration = pure(31, vec![signature(31, 0)]);
            declaration.role = GlobalSourceRole::Implementation;
            Provider::new(
                vec![declaration],
                vec![head(31, Type::Number, Type::String)],
            )
        },
        |_, queries| {
            let callable = queries.value_declaration_type(declaration(31, 0)).unwrap();
            assert_eq!(call(queries, &callable, Type::Number), Type::String);
        },
    );
}

#[test]
fn each_pure_value_can_own_multiple_heads_and_keeps_native_literal_priority() {
    let literal = signature(31, 1);
    Engine::new().with_native_provider(
        || {
            Provider::new(
                vec![pure(31, vec![signature(31, 0), literal])],
                vec![
                    head(31, Type::String, Type::Boolean),
                    Head {
                        key: literal,
                        input: Type::StringLiteral("special".into()),
                        output: Type::Number,
                        generic: false,
                    },
                ],
            )
        },
        |_, queries| {
            let callable = queries.global_binding_type(binding()).unwrap();
            assert_eq!(
                call(queries, &callable, Type::StringLiteral("special".into())),
                Type::Number
            );
            assert_eq!(call(queries, &callable, Type::String), Type::Boolean);
        },
    );
}

#[test]
fn mixed_pure_and_runtime_owners_report_all_original_declarations_without_demanding_either() {
    Engine::new().with_native_provider(
        || {
            Provider::new(
                vec![
                    pure(31, vec![signature(31, 0)]),
                    Declaration {
                        key: declaration(42, 0),
                        heads: None,
                        role: GlobalSourceRole::Implementation,
                    },
                ],
                vec![head(31, Type::Number, Type::String)],
            )
        },
        |provider, queries| {
            let expected = GlobalQueryError::MixedCallableDeclarations {
                binding: binding(),
                declarations: vec![declaration(31, 0), declaration(42, 0)],
            };
            for _ in 0..2 {
                assert_eq!(
                    queries.global_binding_type(binding()),
                    Err(expected.clone())
                );
            }
            assert_eq!(provider.errors.borrow().as_slice(), [expected]);
            assert_eq!(provider.signature_reads.get(), 0);
        },
    );
}

#[test]
fn empty_pure_head_sets_are_errors_and_cannot_fall_back_to_nil_or_any() {
    Engine::new().with_native_provider(
        || Provider::new(vec![pure(31, vec![])], vec![]),
        |_, queries| {
            let expected = GlobalQueryError::InvalidDeclarationSignatures {
                declaration: declaration(31, 0),
                signature: None,
            };
            assert_eq!(
                queries.value_declaration_type(declaration(31, 0)),
                Err(expected.clone())
            );
            assert_eq!(queries.global_binding_type(binding()), Err(expected));
        },
    );
}

#[test]
fn invalid_original_signature_is_reported_at_its_original_value_owner() {
    let foreign = signature(99, 0);
    Engine::new().with_native_provider(
        || {
            Provider::new(
                vec![pure(31, vec![signature(31, 0)]), pure(42, vec![foreign])],
                vec![head(31, Type::Number, Type::String)],
            )
        },
        |_, queries| {
            let expected = GlobalQueryError::InvalidDeclarationSignatures {
                declaration: declaration(42, 0),
                signature: Some(foreign),
            };
            assert_eq!(
                queries.value_declaration_type(declaration(42, 0)),
                Err(expected.clone())
            );
            assert_eq!(queries.global_binding_type(binding()), Err(expected));
        },
    );
}

#[test]
fn foreign_global_owner_rejects_before_pure_metadata_callbacks() {
    let foreign = declaration(99, 0);
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new(
                vec![pure(31, vec![signature(31, 0)])],
                vec![head(31, Type::Number, Type::String)],
            );
            provider.owners.push(foreign);
            provider
        },
        |_, queries| {
            assert_eq!(
                queries.global_binding_type(binding()),
                Err(GlobalQueryError::InvalidDeclaration(foreign))
            );
        },
    );
}
