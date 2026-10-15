//! Original environment/global/value identities and lazy declaration contracts.

use super::{Checker, NativeTypeProvider, ProviderAdapter, SessionQueries, Type, import_type};
use bun_sema::check::native_globals as core;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeEnvironmentKey {
    pub snapshot: u64,
    pub index: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeGlobalBindingKey {
    pub environment: NativeEnvironmentKey,
    pub index: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeValueDeclarationKey {
    pub snapshot: u64,
    pub index: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativePrimitiveMemberKind {
    String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalSourceRole {
    Implementation,
    Declaration,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalValueKind {
    Variable,
    Function,
    Member,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValueDeclarationHeader {
    pub role: GlobalSourceRole,
    pub kind: GlobalValueKind,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GlobalQuery {
    ValueDeclarationContract(NativeValueDeclarationKey),
    ValueDeclaration(NativeValueDeclarationKey),
    GlobalBinding(NativeGlobalBindingKey),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalQueryError {
    InvalidEnvironment(NativeEnvironmentKey),
    InvalidBinding(NativeGlobalBindingKey),
    InvalidDeclaration(NativeValueDeclarationKey),
    InvalidPrimitiveMemberNamespace {
        binding: NativeGlobalBindingKey,
        kind: NativePrimitiveMemberKind,
    },
    MissingDeclarationContract(NativeValueDeclarationKey),
    ConflictingDeclarations {
        binding: NativeGlobalBindingKey,
        declarations: Vec<NativeValueDeclarationKey>,
    },
    CircularQuery(GlobalQuery),
    TooDeep(GlobalQuery),
    UnresolvedQuery(GlobalQuery),
}

fn core_environment(key: NativeEnvironmentKey) -> core::NativeEnvironmentId {
    core::NativeEnvironmentId {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn environment(key: core::NativeEnvironmentId) -> NativeEnvironmentKey {
    NativeEnvironmentKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn core_binding(key: NativeGlobalBindingKey) -> core::NativeGlobalBindingId {
    core::NativeGlobalBindingId {
        environment: core_environment(key.environment),
        index: key.index,
    }
}
fn binding(key: core::NativeGlobalBindingId) -> NativeGlobalBindingKey {
    NativeGlobalBindingKey {
        environment: environment(key.environment),
        index: key.index,
    }
}
fn core_declaration(key: NativeValueDeclarationKey) -> core::NativeValueDeclarationId {
    core::NativeValueDeclarationId {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn declaration(key: core::NativeValueDeclarationId) -> NativeValueDeclarationKey {
    NativeValueDeclarationKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn query(key: core::NativeGlobalsQuery) -> GlobalQuery {
    match key {
        core::NativeGlobalsQuery::ValueDeclarationContract(key) => {
            GlobalQuery::ValueDeclarationContract(declaration(key))
        }
        core::NativeGlobalsQuery::ValueDeclaration(key) => {
            GlobalQuery::ValueDeclaration(declaration(key))
        }
        core::NativeGlobalsQuery::GlobalBinding(key) => GlobalQuery::GlobalBinding(binding(key)),
    }
}
fn error(error: core::NativeGlobalQueryError) -> GlobalQueryError {
    match error {
        core::NativeGlobalQueryError::InvalidEnvironment(key) => {
            GlobalQueryError::InvalidEnvironment(environment(key))
        }
        core::NativeGlobalQueryError::InvalidBinding(key) => {
            GlobalQueryError::InvalidBinding(binding(key))
        }
        core::NativeGlobalQueryError::InvalidDeclaration(key) => {
            GlobalQueryError::InvalidDeclaration(declaration(key))
        }
        core::NativeGlobalQueryError::InvalidPrimitiveMemberNamespace { binding: key, kind } => {
            GlobalQueryError::InvalidPrimitiveMemberNamespace {
                binding: binding(key),
                kind: match kind { core::NativePrimitiveMemberKind::String => NativePrimitiveMemberKind::String },
            }
        }
        core::NativeGlobalQueryError::MissingDeclarationContract(key) => {
            GlobalQueryError::MissingDeclarationContract(declaration(key))
        }
        core::NativeGlobalQueryError::ConflictingDeclarations {
            binding: key,
            declarations,
        } => GlobalQueryError::ConflictingDeclarations {
            binding: binding(key),
            declarations: declarations.into_iter().map(declaration).collect(),
        },
        core::NativeGlobalQueryError::CircularQuery(key) => {
            GlobalQueryError::CircularQuery(query(key))
        }
        core::NativeGlobalQueryError::TooDeep(key) => GlobalQueryError::TooDeep(query(key)),
        core::NativeGlobalQueryError::UnresolvedQuery(key) => {
            GlobalQueryError::UnresolvedQuery(query(key))
        }
    }
}

impl<P: NativeTypeProvider> core::NativeGlobalsProvider for ProviderAdapter<'_, P> {
    fn primitive_member_environment(&self) -> Option<core::NativeEnvironmentId> {
        self.provider.primitive_member_environment().map(core_environment)
    }
    fn primitive_member_binding(&self, key: core::NativeEnvironmentId, kind: core::NativePrimitiveMemberKind) -> Option<core::NativeGlobalBindingId> {
        let kind = match kind { core::NativePrimitiveMemberKind::String => NativePrimitiveMemberKind::String };
        self.provider.primitive_member_binding(environment(key), kind).map(core_binding)
    }
    fn contains_global_environment(&self, key: core::NativeEnvironmentId) -> bool {
        self.provider.contains_global_environment(environment(key))
    }
    fn contains_global_binding(&self, key: core::NativeGlobalBindingId) -> bool {
        self.provider.contains_global_binding(binding(key))
    }
    fn global_binding_declarations(
        &self,
        key: core::NativeGlobalBindingId,
    ) -> Vec<core::NativeValueDeclarationId> {
        self.provider
            .global_binding_declarations(binding(key))
            .into_iter()
            .map(core_declaration)
            .collect()
    }
    fn contains_value_declaration(&self, key: core::NativeValueDeclarationId) -> bool {
        self.provider.contains_value_declaration(declaration(key))
    }
    fn value_declaration_header(
        &self,
        key: core::NativeValueDeclarationId,
    ) -> core::NativeValueDeclarationHeader {
        let header = self.provider.value_declaration_header(declaration(key));
        core::NativeValueDeclarationHeader {
            role: match header.role {
                GlobalSourceRole::Implementation => core::NativeGlobalSourceRole::Implementation,
                GlobalSourceRole::Declaration => core::NativeGlobalSourceRole::Declaration,
            },
            kind: match header.kind {
                GlobalValueKind::Variable => core::NativeGlobalValueKind::Variable,
                GlobalValueKind::Function => core::NativeGlobalValueKind::Function,
                GlobalValueKind::Member => core::NativeGlobalValueKind::Member,
            },
        }
    }
    fn value_declaration_contract(
        &self,
        checker: &mut Checker<'_, '_>,
        key: core::NativeValueDeclarationId,
    ) -> Option<bun_sema::types::TypeId> {
        let value = self.provider.value_declaration_contract(
            declaration(key),
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        )?;
        Some(import_type(checker, self.handles, &value))
    }
    fn value_declaration_initializer(
        &self,
        checker: &mut Checker<'_, '_>,
        key: core::NativeValueDeclarationId,
    ) -> bun_sema::types::TypeId {
        let value = self.provider.value_declaration_initializer(
            declaration(key),
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        import_type(checker, self.handles, &value)
    }
    fn global_query_error(
        &self,
        checker: &mut Checker<'_, '_>,
        value: core::NativeGlobalQueryError,
    ) {
        self.provider.global_query_error(
            error(value),
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
    }
}

impl SessionQueries<'_, '_, '_> {
    pub(super) fn global_binding_type_native(
        &mut self,
        key: NativeGlobalBindingKey,
    ) -> Result<Type, GlobalQueryError> {
        let value = self
            .checker
            .native_global_binding_type(core_binding(key))
            .map_err(error)?;
        Ok(self.handles.retain(value))
    }
    pub(super) fn value_declaration_type_native(
        &mut self,
        key: NativeValueDeclarationKey,
    ) -> Result<Type, GlobalQueryError> {
        let value = self
            .checker
            .native_value_declaration_type(core_declaration(key))
            .map_err(error)?;
        Ok(self.handles.retain(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, FunctionType, NativeFunctionKey, PackArgument, Parameter, TypeQueries};
    use std::cell::{Cell, RefCell};

    const SNAPSHOT: u64 = 4913;
    fn declaration(index: u32) -> NativeValueDeclarationKey {
        NativeValueDeclarationKey {
            snapshot: SNAPSHOT,
            index,
        }
    }
    fn environment(index: u32) -> NativeEnvironmentKey {
        NativeEnvironmentKey {
            snapshot: SNAPSHOT,
            index,
        }
    }
    fn binding(environment_index: u32, index: u32) -> NativeGlobalBindingKey {
        NativeGlobalBindingKey {
            environment: environment(environment_index),
            index,
        }
    }

    struct Globals {
        contracts: Vec<Option<Type>>,
        roles: Vec<GlobalSourceRole>,
        owners: Vec<Vec<u32>>,
        reads: Vec<Cell<usize>>,
        initializers: Cell<usize>,
        errors: RefCell<Vec<GlobalQueryError>>,
        cycle: bool,
    }
    impl Globals {
        fn new(contracts: Vec<Option<Type>>, owners: Vec<Vec<u32>>) -> Self {
            let count = contracts.len();
            Self {
                contracts,
                roles: vec![GlobalSourceRole::Declaration; count],
                owners,
                reads: (0..count).map(|_| Cell::new(0)).collect(),
                initializers: Cell::new(0),
                errors: RefCell::new(Vec::new()),
                cycle: false,
            }
        }
    }
    impl NativeTypeProvider for Globals {
        fn contains_global_environment(&self, key: NativeEnvironmentKey) -> bool {
            key.snapshot == SNAPSHOT && key.index < 2
        }
        fn contains_global_binding(&self, key: NativeGlobalBindingKey) -> bool {
            self.contains_global_environment(key.environment)
                && (key.index as usize) < self.owners.len()
        }
        fn global_binding_declarations(
            &self,
            key: NativeGlobalBindingKey,
        ) -> Vec<NativeValueDeclarationKey> {
            self.owners[(key.index as usize + key.environment.index as usize) % self.owners.len()]
                .iter()
                .map(|&index| declaration(index))
                .collect()
        }
        fn contains_value_declaration(&self, key: NativeValueDeclarationKey) -> bool {
            key.snapshot == SNAPSHOT && (key.index as usize) < self.contracts.len()
        }
        fn value_declaration_header(
            &self,
            key: NativeValueDeclarationKey,
        ) -> ValueDeclarationHeader {
            ValueDeclarationHeader {
                role: self.roles[key.index as usize],
                kind: GlobalValueKind::Variable,
            }
        }
        fn value_declaration_contract(
            &self,
            key: NativeValueDeclarationKey,
            queries: &mut dyn TypeQueries,
        ) -> Option<Type> {
            let index = key.index as usize;
            self.reads[index].set(self.reads[index].get() + 1);
            if self.cycle {
                return Some(queries.value_declaration_type(key).unwrap_or(Type::Any));
            }
            self.contracts[index].clone()
        }
        fn value_declaration_initializer(
            &self,
            _: NativeValueDeclarationKey,
            _: &mut dyn TypeQueries,
        ) -> Type {
            self.initializers.set(self.initializers.get() + 1);
            Type::StringLiteral("inferred".into())
        }
        fn global_query_error(&self, error: GlobalQueryError, _: &mut dyn TypeQueries) {
            self.errors.borrow_mut().push(error);
        }
        fn contains_function(&self, _: NativeFunctionKey) -> bool {
            false
        }
        fn function_parameters(
            &self,
            _: NativeFunctionKey,
            _: &mut dyn TypeQueries,
        ) -> Vec<Parameter> {
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
            panic!("ambient bodies must never be inferred")
        }
        fn return_cycle(&self, _: NativeFunctionKey) {
            panic!("no runtime function")
        }
        fn return_type_too_deep(&self, _: NativeFunctionKey) {
            panic!("no runtime function")
        }
    }

    #[test]
    fn ambient_contracts_are_lazy_cached_and_never_read_placeholder_initializers() {
        Engine::new().with_native_provider(
            || Globals::new(vec![Some(Type::Number)], vec![vec![0]]),
            |provider, queries| {
                for _ in 0..2 {
                    let ty = queries.global_binding_type(binding(0, 0)).unwrap();
                    assert!(queries.is_assignable(&ty, &Type::Number));
                }
                let ty = queries.value_declaration_type(declaration(0)).unwrap();
                assert!(queries.is_assignable(&ty, &Type::Number));
                assert_eq!(provider.reads[0].get(), 1);
                assert_eq!(provider.initializers.get(), 0);
                assert!(provider.errors.borrow().is_empty());
            },
        );
    }

    #[test]
    fn compatible_duplicate_contracts_share_type_without_source_order_selection() {
        Engine::new().with_native_provider(
            || {
                Globals::new(
                    vec![Some(Type::Number), Some(Type::Number)],
                    vec![vec![1, 0, 1]],
                )
            },
            |provider, queries| {
                let ty = queries.global_binding_type(binding(0, 0)).unwrap();
                assert!(queries.is_assignable(&ty, &Type::Number));
                assert_eq!(
                    provider.reads.iter().map(Cell::get).collect::<Vec<_>>(),
                    [1, 1]
                );
                assert!(provider.errors.borrow().is_empty());
            },
        );
    }

    #[test]
    fn every_conflicting_original_owner_is_reported_in_canonical_order() {
        Engine::new().with_native_provider(
            || {
                Globals::new(
                    vec![Some(Type::Number), Some(Type::String), Some(Type::Number)],
                    vec![vec![2, 1, 0]],
                )
            },
            |provider, queries| {
                let expected = GlobalQueryError::ConflictingDeclarations {
                    binding: binding(0, 0),
                    declarations: vec![declaration(0), declaration(1), declaration(2)],
                };
                assert_eq!(
                    queries.global_binding_type(binding(0, 0)),
                    Err(expected.clone())
                );
                assert_eq!(
                    queries.global_binding_type(binding(0, 0)),
                    Err(expected.clone())
                );
                assert_eq!(provider.errors.borrow().as_slice(), [expected]);
            },
        );
    }

    #[test]
    fn any_is_not_identical_to_a_specific_global_contract() {
        Engine::new().with_native_provider(
            || Globals::new(vec![Some(Type::Any), Some(Type::Number)], vec![vec![0, 1]]),
            |_, queries| {
                assert!(matches!(
                    queries.global_binding_type(binding(0, 0)),
                    Err(GlobalQueryError::ConflictingDeclarations { .. })
                ));
            },
        );
    }

    #[test]
    fn written_union_contract_is_authoritative_over_narrower_inferred_introductions() {
        Engine::new().with_native_provider(
            || {
                let mut globals = Globals::new(
                    vec![Some(Type::Union(vec![Type::Number, Type::String])), None],
                    vec![vec![1, 0]],
                );
                globals.roles[1] = GlobalSourceRole::Implementation;
                globals
            },
            |provider, queries| {
                let schema = queries.global_binding_type(binding(0, 0)).unwrap();
                assert!(queries.is_assignable(&Type::Number, &schema));
                assert!(queries.is_assignable(&Type::String, &schema));
                assert!(!queries.is_assignable(&Type::Boolean, &schema));
                assert_eq!(provider.initializers.get(), 0);
                let inferred = queries.value_declaration_type(declaration(1)).unwrap();
                assert!(queries.is_assignable(&inferred, &schema));
                assert_eq!(provider.initializers.get(), 1);
                assert_eq!(
                    provider.reads.iter().map(Cell::get).collect::<Vec<_>>(),
                    [1, 1]
                );
                assert!(provider.errors.borrow().is_empty());
            },
        );
    }

    #[test]
    fn missing_ambient_contract_is_an_error_while_implementation_values_can_be_inferred() {
        Engine::new().with_native_provider(
            || {
                let mut globals = Globals::new(vec![None, None], vec![vec![0], vec![1]]);
                globals.roles[1] = GlobalSourceRole::Implementation;
                globals
            },
            |provider, queries| {
                assert_eq!(
                    queries.value_declaration_type(declaration(0)),
                    Err(GlobalQueryError::MissingDeclarationContract(declaration(0)))
                );
                assert_eq!(provider.initializers.get(), 0);
                let ty = queries.global_binding_type(binding(0, 1)).unwrap();
                assert!(queries.is_assignable(&ty, &Type::String));
                assert!(queries.is_assignable(&Type::String, &ty));
                assert_eq!(provider.initializers.get(), 1);
            },
        );
    }

    #[test]
    fn foreign_environment_binding_and_declaration_reject_before_type_callbacks() {
        Engine::new().with_native_provider(
            || Globals::new(vec![Some(Type::Number)], vec![vec![0]]),
            |provider, queries| {
                let foreign = NativeEnvironmentKey {
                    snapshot: SNAPSHOT + 1,
                    index: 0,
                };
                let invalid = NativeGlobalBindingKey {
                    environment: foreign,
                    index: 0,
                };
                assert_eq!(
                    queries.global_binding_type(invalid),
                    Err(GlobalQueryError::InvalidEnvironment(foreign))
                );
                assert_eq!(
                    queries.global_binding_type(binding(0, 9)),
                    Err(GlobalQueryError::InvalidBinding(binding(0, 9)))
                );
                let foreign = NativeValueDeclarationKey {
                    snapshot: SNAPSHOT + 1,
                    index: 0,
                };
                assert_eq!(
                    queries.value_declaration_type(foreign),
                    Err(GlobalQueryError::InvalidDeclaration(foreign))
                );
                assert_eq!(provider.reads[0].get(), 0);
                assert_eq!(provider.initializers.get(), 0);
            },
        );
    }

    #[test]
    fn environment_identity_partitions_binding_queries() {
        Engine::new().with_native_provider(
            || {
                Globals::new(
                    vec![Some(Type::Number), Some(Type::String)],
                    vec![vec![0], vec![1]],
                )
            },
            |_, queries| {
                for (env, expected) in [(0, Type::Number), (1, Type::String)] {
                    let ty = queries.global_binding_type(binding(env, 0)).unwrap();
                    assert!(queries.is_assignable(&ty, &expected));
                }
            },
        );
    }

    #[test]
    fn recursive_global_contract_has_a_native_query_cycle_error() {
        Engine::new().with_native_provider(
            || {
                let mut provider = Globals::new(vec![None], vec![vec![0]]);
                provider.cycle = true;
                provider
            },
            |provider, queries| {
                assert!(matches!(
                    queries.value_declaration_type(declaration(0)),
                    Err(GlobalQueryError::CircularQuery(_))
                ));
                assert!(
                    provider
                        .errors
                        .borrow()
                        .iter()
                        .any(|error| matches!(error, GlobalQueryError::CircularQuery(_)))
                );
                assert_eq!(provider.initializers.get(), 0);
            },
        );
    }

    #[test]
    fn ambient_callable_contract_uses_native_calls_without_runtime_function_identity() {
        Engine::new().with_native_provider(
            || {
                Globals::new(
                    vec![Some(Type::Function(FunctionType {
                        params: vec![Parameter {
                            name: "value".into(),
                            ty: Type::Number,
                            optional: false,
                        }],
                        returns: Box::new(Type::String),
                    }))],
                    vec![vec![0]],
                )
            },
            |provider, queries| {
                let ty = queries.global_binding_type(binding(0, 0)).unwrap();
                let result = queries.call_native_arguments_scalar(
                    &ty,
                    None,
                    &[PackArgument::Resolved(Type::Number)],
                    &[],
                    None,
                );
                assert!(result.errors.is_empty());
                assert_eq!(result.return_type, Type::String);
                let result = queries.call_native_arguments_scalar(
                    &ty,
                    None,
                    &[PackArgument::Resolved(Type::String)],
                    &[],
                    None,
                );
                assert!(!result.errors.is_empty());
                assert_eq!(provider.initializers.get(), 0);
            },
        );
    }

    #[test]
    fn opaque_global_runtime_contracts_keep_native_kind_through_metadata_union_reduction() {
        Engine::new().with_native_provider(
            || {
                Globals::new(
                    vec![Some(Type::RuntimeFunction), Some(Type::RuntimeTable)],
                    vec![vec![0], vec![1]],
                )
            },
            |provider, queries| {
                for (index, expected) in [(0, Type::RuntimeFunction), (1, Type::RuntimeTable)] {
                    let owned = queries.global_binding_type(binding(0, index)).unwrap();
                    assert!(matches!(owned, Type::SessionType(_)));
                    assert_eq!(queries.inspect_type(&owned), expected);
                    let reduced = queries.union_reduced(&[owned, Type::Never]);
                    assert_eq!(queries.inspect_type(&reduced), expected);
                }
                let callable = queries.global_binding_type(binding(0, 0)).unwrap();
                assert_eq!(queries.function_implementation_type(&callable), None);
                let called = queries.call_pack(&callable, &[Type::Number], &[], None);
                assert!(called.errors.is_empty());
                assert_eq!(queries.pack_slot(&called.return_pack, 1), Type::Any);
                assert_eq!(provider.initializers.get(), 0);
                assert_eq!(
                    provider.reads.iter().map(Cell::get).collect::<Vec<_>>(),
                    [1, 1]
                );
            },
        );
    }
}
