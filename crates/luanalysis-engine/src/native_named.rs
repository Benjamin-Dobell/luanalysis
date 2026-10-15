//! Original declaration/member recipes transported into the native lazy
//! declaration queries. This module never expands aliases or compares shapes.
use super::{
    Checker, LuaKey, NativeFunctionKey, NativeTypeParameterKey, NativeTypeProvider,
    ProviderAdapter, SessionQueries, TableError, Type, TypeHandles, core_key, core_parameter_key,
    export_type, import_type, source_parameter_key,
};
use bun_sema::check::native_named::{
    NativeDefinitionHeader, NativeDefinitionId,
    NativeNamedTypeError, NativeObjectDeclarationForm, NativeTypeDeclarationHeader,
    NativeTypeDeclarationId, NativeTypeDeclarationKind, NativeTypeDeclarationOwner,
    NativeTypeDeclarationProvider, NativeTypeMemberHeader, NativeTypeMemberId,
    NativeTypeMemberKey as CoreMemberKey, NativeTypeMemberOwner, NativeTypeMemberVisibility,
};
use bun_sema::types::{NativeTableKey, TypeId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeTypeDeclarationKey {
    pub snapshot: u64,
    pub index: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeTypeMemberKey {
    pub snapshot: u64,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeClassDefinitionKey { pub snapshot: u64, pub index: u32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassDefinitionHeader { pub declaration: NativeTypeDeclarationKey }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeDeclarationKind {
    Alias,
    Shape,
    Class,
    Module,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectDeclarationForm {
    Record,
    Array,
    Table { type_argument_count: usize },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeDeclarationOwner {
    Function(NativeFunctionKey),
    NamedType(NativeTypeDeclarationKey),
    Signature(super::NativeSignatureKey),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeDeclarationHeader {
    pub kind: TypeDeclarationKind,
    pub name: String,
    pub object_form: Option<ObjectDeclarationForm>,
    pub parameters: Vec<NativeTypeParameterKey>,
    pub outer_parameters: Vec<NativeTypeParameterKey>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeMemberVisibility {
    Public,
    Protected,
    Private,
}
#[derive(Clone, Debug, PartialEq)]
pub enum TypeMemberKeyRecipe {
    Fixed(LuaKey),
    Indexer { sparse: bool },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeMemberOwner {
    Declared(NativeTypeDeclarationKey),
    Reference {
        name: String,
        candidates: Vec<NativeTypeDeclarationKey>,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct TypeMemberHeader {
    pub owner: TypeMemberOwner,
    pub key: TypeMemberKeyRecipe,
    pub visibility: TypeMemberVisibility,
    pub optional: bool,
    pub readonly: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedTypeQuery {
    Head(NativeTypeDeclarationKey),
    Member(NativeTypeMemberKey),
    Indexer(NativeTypeMemberKey),
    Bases(NativeTypeDeclarationKey),
    ObjectMembers(NativeTypeDeclarationKey),
}
#[derive(Clone, Debug, PartialEq)]
pub enum NamedTypeError {
    InvalidDefinition(NativeClassDefinitionKey),
    InvalidReceiver { declaration: NativeTypeDeclarationKey, expected: Type, actual: Type },
    UnsupportedReceiverType { actual: Type },
    DefinitionOwner { owner: NativeClassDefinitionKey, expected: NativeTypeDeclarationKey, actual: Type },
    DefinitionInitializer { owner: NativeClassDefinitionKey, actual: Type },
    DefinitionField { owner: NativeClassDefinitionKey, error: TableError },
    InvalidDeclaration(NativeTypeDeclarationKey),
    InvalidMember(NativeTypeMemberKey),
    InvalidTypeParameter(NativeTypeParameterKey),
    UnknownOwner {
        member: NativeTypeMemberKey,
        name: String,
    },
    AmbiguousOwner {
        member: NativeTypeMemberKey,
        name: String,
    },
    TypeArgumentArity {
        declaration: NativeTypeDeclarationKey,
        actual: usize,
        minimum: usize,
        maximum: usize,
    },
    TypeArgumentConstraint {
        declaration: NativeTypeDeclarationKey,
        index: usize,
        actual: Type,
        expected: Type,
    },
    CircularAlias(NativeTypeDeclarationKey),
    CircularBase(NativeTypeDeclarationKey),
    InvalidBase { declaration: NativeTypeDeclarationKey, index: usize, base: Type },
    IncompatibleOverride { declaration: NativeTypeDeclarationKey, member: NativeTypeMemberKey, base: Type, expected: Type, actual: Type },
    IncompatibleBase { declaration: NativeTypeDeclarationKey, index: usize, base: Type, expected: Type, actual: Type },
    TooDeep(NamedTypeQuery),
    UnsupportedKind(NativeTypeDeclarationKey, TypeDeclarationKind),
    UnsupportedBases(NativeTypeDeclarationKey),
    UnsupportedVisibility(NativeTypeMemberKey),
    UnsupportedOwnerInstantiation(NativeTypeMemberKey),
    Table {
        declaration: NativeTypeDeclarationKey,
        error: TableError,
    },
}

fn core_definition(key: NativeClassDefinitionKey) -> NativeDefinitionId { NativeDefinitionId { snapshot:key.snapshot,index:key.index } }
fn source_definition(key: NativeDefinitionId) -> NativeClassDefinitionKey { NativeClassDefinitionKey {snapshot:key.snapshot,index:key.index} }

pub(super) fn definition_context(queries: &mut SessionQueries<'_, '_, '_>, owner: NativeClassDefinitionKey, class: &Type) -> Result<Type, NamedTypeError> {
    let class = import_type(queries.checker, queries.handles, class);
    queries.checker.native_definition_context(core_definition(owner), class)
        .map(|ty| export_type(queries.checker,queries.handles,ty))
        .map_err(|error| export_error(queries.checker,queries.handles,error))
}
pub(super) fn definition_value(queries: &mut SessionQueries<'_, '_, '_>, owner: NativeClassDefinitionKey, class: &Type, actual: &Type) -> Result<Type, NamedTypeError> {
    let class = import_type(queries.checker, queries.handles, class);
    let actual = import_type(queries.checker, queries.handles, actual);
    queries.checker.native_definition_value(core_definition(owner), class, actual)
        .map(|ty| export_type(queries.checker,queries.handles,ty))
        .map_err(|error| export_error(queries.checker,queries.handles,error))
}

pub(super) fn core_declaration(key: NativeTypeDeclarationKey) -> NativeTypeDeclarationId {
    NativeTypeDeclarationId {
        snapshot: key.snapshot,
        index: key.index,
    }
}
pub(super) fn source_declaration(key: NativeTypeDeclarationId) -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn core_member(key: NativeTypeMemberKey) -> NativeTypeMemberId {
    NativeTypeMemberId {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn source_member(key: NativeTypeMemberId) -> NativeTypeMemberKey {
    NativeTypeMemberKey {
        snapshot: key.snapshot,
        index: key.index,
    }
}
fn source_kind(kind: NativeTypeDeclarationKind) -> TypeDeclarationKind {
    match kind {
        NativeTypeDeclarationKind::Alias => TypeDeclarationKind::Alias,
        NativeTypeDeclarationKind::Shape => TypeDeclarationKind::Shape,
        NativeTypeDeclarationKind::Class => TypeDeclarationKind::Class,
        NativeTypeDeclarationKind::Module => TypeDeclarationKind::Module,
    }
}

impl<P: NativeTypeProvider> NativeTypeDeclarationProvider for ProviderAdapter<'_, P> {
    fn type_declaration_self_parameter(&self, key: NativeTypeDeclarationId) -> Option<bun_sema::types::NativeTypeParamId> { self.provider.type_declaration_self_parameter(source_declaration(key)).map(super::core_parameter_key) }
    fn contains_class_definition(&self, key: NativeDefinitionId) -> bool {
        self.provider.contains_class_definition(source_definition(key))
    }
    fn class_definition_header(&self, key: NativeDefinitionId) -> NativeDefinitionHeader {
        NativeDefinitionHeader { declaration: core_declaration(self.provider.class_definition_header(source_definition(key)).declaration) }
    }
    fn type_declaration_call_signatures(&self, key: NativeTypeDeclarationId) -> Vec<bun_sema::types::NativeSignatureId> {
        self.provider.type_declaration_call_signatures(source_declaration(key)).into_iter().map(super::core_signature_key).collect()
    }
    fn contains_type_declaration(&self, key: NativeTypeDeclarationId) -> bool {
        self.provider
            .contains_type_declaration(source_declaration(key))
    }
    fn type_declaration_header(
        &self,
        checker: &Checker<'_, '_>,
        key: NativeTypeDeclarationId,
    ) -> NativeTypeDeclarationHeader {
        let header = self
            .provider
            .type_declaration_header(source_declaration(key));
        NativeTypeDeclarationHeader {
            kind: match header.kind {
                TypeDeclarationKind::Alias => NativeTypeDeclarationKind::Alias,
                TypeDeclarationKind::Shape => NativeTypeDeclarationKind::Shape,
                TypeDeclarationKind::Class => NativeTypeDeclarationKind::Class,
                TypeDeclarationKind::Module => NativeTypeDeclarationKind::Module,
            },
            name: checker.atoms().intern(header.name.as_bytes()),
            object_form: header.object_form.map(|form| match form {
                ObjectDeclarationForm::Record => NativeObjectDeclarationForm::Record,
                ObjectDeclarationForm::Array => NativeObjectDeclarationForm::Array,
                ObjectDeclarationForm::Table {
                    type_argument_count,
                } => NativeObjectDeclarationForm::Table {
                    type_argument_count,
                },
            }),
            parameters: header
                .parameters
                .into_iter()
                .map(core_parameter_key)
                .collect(),
            outer_parameters: header
                .outer_parameters
                .into_iter()
                .map(core_parameter_key)
                .collect(),
        }
    }
    fn contains_type_member(&self, key: NativeTypeMemberId) -> bool {
        self.provider.contains_type_member(source_member(key))
    }
    fn type_member_origin(&self, key: NativeTypeMemberId) -> bun_sema::types::NativeTableFieldOrigin {
        match self.provider.type_member_origin(source_member(key)) {
            super::TableFieldOrigin::Inferred => bun_sema::types::NativeTableFieldOrigin::Inferred,
            super::TableFieldOrigin::Declared => bun_sema::types::NativeTableFieldOrigin::Declared,
        }
    }
    fn type_member_header(
        &self,
        checker: &Checker<'_, '_>,
        key: NativeTypeMemberId,
    ) -> NativeTypeMemberHeader {
        let header = self.provider.type_member_header(source_member(key));
        NativeTypeMemberHeader {
            owner: match header.owner {
                TypeMemberOwner::Declared(key) => {
                    NativeTypeMemberOwner::Declared(core_declaration(key))
                }
                TypeMemberOwner::Reference { name, candidates } => {
                    NativeTypeMemberOwner::Reference {
                        name: checker.atoms().intern(name.as_bytes()),
                        candidates: candidates.into_iter().map(core_declaration).collect(),
                    }
                }
            },
            key: match header.key {
                TypeMemberKeyRecipe::Fixed(key) => CoreMemberKey::Fixed(match key {
                    LuaKey::String(bytes) => NativeTableKey::String(checker.atoms().intern(&bytes)),
                    LuaKey::Integer(value) => NativeTableKey::Integer(value),
                    LuaKey::Number(value) => NativeTableKey::Float(value.to_bits()),
                    LuaKey::Boolean(value) => NativeTableKey::Boolean(value),
                }),
                TypeMemberKeyRecipe::Indexer { sparse } => CoreMemberKey::Indexer { sparse },
            },
            visibility: match header.visibility {
                TypeMemberVisibility::Public => NativeTypeMemberVisibility::Public,
                TypeMemberVisibility::Protected => NativeTypeMemberVisibility::Protected,
                TypeMemberVisibility::Private => NativeTypeMemberVisibility::Private,
            },
            optional: header.optional,
            readonly: header.readonly,
        }
    }
    fn type_declaration_members(&self, key: NativeTypeDeclarationId) -> Vec<NativeTypeMemberId> {
        self.provider
            .type_declaration_members(source_declaration(key))
            .into_iter()
            .map(core_member)
            .collect()
    }
    fn type_alias_target(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeTypeDeclarationId,
    ) -> TypeId {
        let ty = self.provider.type_alias_target(
            source_declaration(key),
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        import_type(checker, self.handles, &ty)
    }
    fn type_member_type(&self, checker: &mut Checker<'_, '_>, key: NativeTypeMemberId) -> TypeId {
        let ty = self.provider.type_member_type(
            source_member(key),
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        import_type(checker, self.handles, &ty)
    }
    fn type_indexer_key(&self, checker: &mut Checker<'_, '_>, key: NativeTypeMemberId) -> TypeId {
        let ty = self.provider.type_indexer_key(
            source_member(key),
            &mut SessionQueries {
                checker,
                handles: self.handles,
            },
        );
        import_type(checker, self.handles, &ty)
    }
    fn type_declaration_bases(
        &self,
        checker: &mut Checker<'_, '_>,
        key: NativeTypeDeclarationId,
    ) -> Vec<TypeId> {
        self.provider
            .type_declaration_bases(
                source_declaration(key),
                &mut SessionQueries {
                    checker,
                    handles: self.handles,
                },
            )
            .iter()
            .map(|ty| import_type(checker, self.handles, ty))
            .collect()
    }
    fn type_parameter_owner(
        &self,
        key: bun_sema::types::NativeTypeParamId,
    ) -> Option<NativeTypeDeclarationOwner> {
        self.provider
            .type_parameter_owner(source_parameter_key(key))
            .map(|owner| match owner {
                TypeDeclarationOwner::Function(key) => {
                    NativeTypeDeclarationOwner::Function(core_key(key))
                }
                TypeDeclarationOwner::NamedType(key) => {
                    NativeTypeDeclarationOwner::NamedType(core_declaration(key))
                }
                TypeDeclarationOwner::Signature(key) => NativeTypeDeclarationOwner::Signature(super::core_signature_key(key)),
            })
    }
    fn type_declaration_error(&self, checker: &mut Checker<'_, '_>, error: NativeNamedTypeError) {
        self.provider
            .type_declaration_error(export_error(checker, self.handles, error))
    }
}

pub(super) fn receiver(queries: &mut SessionQueries<'_, '_, '_>, class: &Type, receiver: &Type) -> Result<Type, NamedTypeError> {
    let class = import_type(queries.checker, queries.handles, class);
    let receiver = import_type(queries.checker, queries.handles, receiver);
    queries.checker.native_receiver_type(class, receiver)
        .map(|ty| export_type(queries.checker, queries.handles, ty))
        .map_err(|error| export_error(queries.checker, queries.handles, error))
}

pub(super) fn resolve(
    queries: &mut SessionQueries<'_, '_, '_>,
    declaration: NativeTypeDeclarationKey,
    arguments: &[Type],
) -> Result<Type, NamedTypeError> {
    let arguments: Vec<_> = arguments
        .iter()
        .map(|ty| import_type(queries.checker, queries.handles, ty))
        .collect();
    queries
        .checker
        .native_type_reference(core_declaration(declaration), &arguments)
        .map(|ty| export_type(queries.checker, queries.handles, ty))
        .map_err(|error| export_error(queries.checker, queries.handles, error))
}

pub(super) fn declared(
    queries: &mut SessionQueries<'_, '_, '_>,
    declaration: NativeTypeDeclarationKey,
    validate: bool,
) -> Result<Type, NamedTypeError> {
    let result = if validate {
        queries
            .checker
            .validate_native_type_declaration(core_declaration(declaration))
    } else {
        queries
            .checker
            .native_declared_head(core_declaration(declaration))
    };
    result
        .map(|ty| export_type(queries.checker, queries.handles, ty))
        .map_err(|error| export_error(queries.checker, queries.handles, error))
}

pub(super) fn validate_member(
    queries: &mut SessionQueries<'_, '_, '_>,
    member: NativeTypeMemberKey,
) -> Result<Type, NamedTypeError> {
    queries
        .checker
        .validate_native_type_member(core_member(member))
        .map(|ty| export_type(queries.checker, queries.handles, ty))
        .map_err(|error| export_error(queries.checker, queries.handles, error))
}

pub(super) fn export_error(
    checker: &mut Checker<'_, '_>,
    handles: &TypeHandles,
    error: NativeNamedTypeError,
) -> NamedTypeError {
    use bun_sema::check::native_named::NativeTypeDeclarationQuery as Q;
    match error {
        NativeNamedTypeError::InvalidReceiver { declaration, expected, actual } => NamedTypeError::InvalidReceiver { declaration: source_declaration(declaration), expected: export_type(checker, handles, expected), actual: export_type(checker, handles, actual) },
        NativeNamedTypeError::UnsupportedReceiverType { actual } => NamedTypeError::UnsupportedReceiverType { actual: export_type(checker, handles, actual) },
        NativeNamedTypeError::InvalidDefinition(owner) => NamedTypeError::InvalidDefinition(source_definition(owner)),
        NativeNamedTypeError::DefinitionOwner { owner,expected,actual } => NamedTypeError::DefinitionOwner { owner:source_definition(owner),expected:source_declaration(expected),actual:export_type(checker,handles,actual) },
        NativeNamedTypeError::DefinitionInitializer { owner,actual } => NamedTypeError::DefinitionInitializer { owner:source_definition(owner),actual:export_type(checker,handles,actual) },
        NativeNamedTypeError::DefinitionField { owner,error } => NamedTypeError::DefinitionField { owner:source_definition(owner),error:super::native_tables::export_error(checker,handles,error) },

        NativeNamedTypeError::InvalidDeclaration(key) => {
            NamedTypeError::InvalidDeclaration(source_declaration(key))
        }
        NativeNamedTypeError::InvalidMember(key) => {
            NamedTypeError::InvalidMember(source_member(key))
        }
        NativeNamedTypeError::InvalidTypeParameter(key) => {
            NamedTypeError::InvalidTypeParameter(source_parameter_key(key))
        }
        NativeNamedTypeError::UnknownOwner { member, name } => NamedTypeError::UnknownOwner {
            member: source_member(member),
            name: String::from_utf8_lossy(checker.atoms().bytes(name)).into_owned(),
        },
        NativeNamedTypeError::AmbiguousOwner { member, name } => NamedTypeError::AmbiguousOwner {
            member: source_member(member),
            name: String::from_utf8_lossy(checker.atoms().bytes(name)).into_owned(),
        },
        NativeNamedTypeError::TypeArgumentArity {
            declaration,
            actual,
            minimum,
            maximum,
        } => NamedTypeError::TypeArgumentArity {
            declaration: source_declaration(declaration),
            actual,
            minimum,
            maximum,
        },
        NativeNamedTypeError::TypeArgumentConstraint {
            declaration,
            index,
            actual,
            expected,
        } => NamedTypeError::TypeArgumentConstraint {
            declaration: source_declaration(declaration),
            index,
            actual: export_type(checker, handles, actual),
            expected: export_type(checker, handles, expected),
        },
        NativeNamedTypeError::CircularAlias(key) => {
            NamedTypeError::CircularAlias(source_declaration(key))
        }
        NativeNamedTypeError::CircularBase(key) => {
            NamedTypeError::CircularBase(source_declaration(key))
        }
        NativeNamedTypeError::InvalidBase { declaration, index, base } => NamedTypeError::InvalidBase { declaration: source_declaration(declaration), index, base: export_type(checker, handles, base) },
        NativeNamedTypeError::IncompatibleOverride { declaration, member, base, expected, actual } => NamedTypeError::IncompatibleOverride { declaration: source_declaration(declaration), member: source_member(member), base: export_type(checker, handles, base), expected: export_type(checker, handles, expected), actual: export_type(checker, handles, actual) },
        NativeNamedTypeError::IncompatibleBase { declaration, index, base, expected, actual } => NamedTypeError::IncompatibleBase { declaration: source_declaration(declaration), index, base: export_type(checker, handles, base), expected: export_type(checker, handles, expected), actual: export_type(checker, handles, actual) },
        NativeNamedTypeError::TooDeep(query) => NamedTypeError::TooDeep(match query {
            Q::DeclaredHead(key) => NamedTypeQuery::Head(source_declaration(key)),
            Q::MemberType(key) => NamedTypeQuery::Member(source_member(key)),
            Q::IndexerKey(key) => NamedTypeQuery::Indexer(source_member(key)),
            Q::Bases(key) => NamedTypeQuery::Bases(source_declaration(key)),
            Q::ObjectMembers(ty) => {
                let bun_sema::types::TypeData::NativeObject(reference) = checker.data(ty) else {
                    unreachable!("native object-members query owner")
                };
                NamedTypeQuery::ObjectMembers(source_declaration(reference.declaration))
            }
        }),
        NativeNamedTypeError::UnsupportedKind { declaration, kind } => {
            NamedTypeError::UnsupportedKind(source_declaration(declaration), source_kind(kind))
        }
        NativeNamedTypeError::UnsupportedBases(key) => {
            NamedTypeError::UnsupportedBases(source_declaration(key))
        }
        NativeNamedTypeError::UnsupportedVisibility(key) => {
            NamedTypeError::UnsupportedVisibility(source_member(key))
        }
        NativeNamedTypeError::UnsupportedOwnerInstantiation(key) => {
            NamedTypeError::UnsupportedOwnerInstantiation(source_member(key))
        }
        NativeNamedTypeError::Table { declaration, error } => NamedTypeError::Table {
            declaration: source_declaration(declaration),
            error: super::native_tables::export_error(checker, handles, error),
        },
    }
}
