use crate::*;
use std::cell::{Cell, RefCell};

const SNAPSHOT: u64 = 95631;

fn declaration(index: u32) -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey {
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
fn member(index: u32) -> NativeTypeMemberKey {
    NativeTypeMemberKey {
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
fn function(index: u32) -> NativeFunctionKey {
    NativeFunctionKey {
        snapshot: SNAPSHOT,
        index,
    }
}
fn owner_input() -> NativeFlowKey {
    NativeFlowKey {
        graph: NativeGraphKey {
            snapshot: SNAPSHOT,
            region: 0,
            context: 0,
        },
        node: 0,
    }
}
fn formal(key: NativeTypeParameterKey) -> Type {
    Type::NativeTypeParameter(key)
}
fn reference(owner: NativeTypeDeclarationKey, arguments: Vec<Type>) -> Type {
    Type::NativeNamedReference {
        declaration: owner,
        arguments,
    }
}
fn array(element: Type) -> Type {
    Type::Table(TableType {
        indexers: vec![TableIndexer::new(Type::Number, element)],
        ..Default::default()
    })
}
fn literal(value: &str) -> Type {
    Type::StringLiteral(value.as_bytes().to_vec())
}

struct Declaration {
    parameters: Vec<NativeTypeParameterKey>,
    outer: Vec<NativeTypeParameterKey>,
    alias: Option<(NativeTypeDeclarationKey, Type)>,
    self_parameter: Option<NativeTypeParameterKey>,
    bases: Vec<Type>,
}
struct ParameterSource {
    owner: TypeDeclarationOwner,
    constraint: Option<Type>,
}
struct SignatureSource {
    parameters: Vec<Type>,
    returns: Type,
    own: Vec<NativeTypeParameterKey>,
    outer: Vec<NativeTypeParameterKey>,
    applied_self_owner: Option<Type>,
    returns_self: bool,
}
struct MemberSource {
    owner: TypeMemberOwner,
    application: Option<MemberOwnerApplication>,
    key: TypeMemberKeyRecipe,
    value: Type,
    indexer_key: Option<Type>,
    signatures: Option<Vec<NativeSignatureKey>>,
}
struct OwnerDependency {
    member: NativeTypeMemberKey,
    scope: NativeFunctionKey,
    settled_owner: Type,
}
struct Provider {
    declarations: Vec<Declaration>,
    parameters: Vec<ParameterSource>,
    signatures: Vec<SignatureSource>,
    functions: Vec<SignatureSource>,
    members: Vec<MemberSource>,
    reads: Cell<usize>,
    errors: RefCell<Vec<NamedTypeError>>,
    owner_dependency: Option<OwnerDependency>,
    owner_pending: Cell<bool>,
    owner_calls: Cell<usize>,
    application_calls: Cell<usize>,
    invalid_outer_instance: RefCell<Option<Type>>,
    unrelated_inference_function: Option<NativeFunctionKey>,
    inferred_return_demands: Cell<usize>,
    assignments: Vec<NativeTypeMemberKey>,
}

impl Provider {
    fn new() -> Self {
        Self {
            declarations: vec![
                Declaration {
                    parameters: vec![parameter(0)],
                    outer: vec![],
                    alias: None,
                    self_parameter: None,
                    bases: vec![],
                },
                Declaration {
                    parameters: vec![parameter(1), parameter(2)],
                    outer: vec![],
                    alias: None,
                    self_parameter: None,
                    bases: vec![],
                },
            ],
            parameters: (0..3)
                .map(|index| ParameterSource {
                    owner: TypeDeclarationOwner::NamedType(declaration(if index == 0 {
                        0
                    } else {
                        1
                    })),
                    constraint: None,
                })
                .collect(),
            signatures: vec![],
            functions: vec![],
            members: vec![],
            reads: Cell::new(0),
            errors: RefCell::new(vec![]),
            owner_dependency: None,
            owner_pending: Cell::new(false),
            owner_calls: Cell::new(0),
            application_calls: Cell::new(0),
            invalid_outer_instance: RefCell::new(None),
            unrelated_inference_function: None,
            inferred_return_demands: Cell::new(0),
            assignments: vec![],
        }
    }

    /// A lexical generic belongs to a real pure signature scope. No runtime
    /// function identity is needed to declare or infer the parameter.
    fn lexical(&mut self, constraint: Option<Type>) -> NativeTypeParameterKey {
        let key = parameter(self.parameters.len() as u32);
        let scope = signature(self.signatures.len() as u32);
        self.parameters.push(ParameterSource {
            owner: TypeDeclarationOwner::Signature(scope),
            constraint,
        });
        self.signatures.push(SignatureSource {
            parameters: vec![],
            returns: Type::Nil,
            own: vec![key],
            outer: vec![],
            applied_self_owner: None,
            returns_self: false,
        });
        key
    }

    fn alias(
        &mut self,
        target: Type,
        outer: Vec<NativeTypeParameterKey>,
    ) -> NativeTypeDeclarationKey {
        let key = declaration(self.declarations.len() as u32);
        self.declarations.push(Declaration {
            parameters: vec![],
            outer,
            alias: Some((declaration(0), target)),
            self_parameter: None,
            bases: vec![],
        });
        key
    }

    fn field(
        &mut self,
        owner: NativeTypeDeclarationKey,
        name: &str,
        value: Type,
        application: Option<MemberOwnerApplication>,
    ) -> NativeTypeMemberKey {
        let key = member(self.members.len() as u32);
        self.members.push(MemberSource {
            owner: TypeMemberOwner::Declared(owner),
            application,
            key: TypeMemberKeyRecipe::Fixed(LuaKey::String(name.as_bytes().to_vec())),
            value,
            indexer_key: None,
            signatures: None,
        });
        key
    }

    fn method(
        &mut self,
        name: &str,
        inputs: Vec<Type>,
        output: Type,
        outer: Vec<NativeTypeParameterKey>,
        application: MemberOwnerApplication,
    ) -> NativeTypeMemberKey {
        let head = signature(self.signatures.len() as u32);
        self.signatures.push(SignatureSource {
            parameters: inputs,
            returns: output,
            own: vec![],
            outer,
            applied_self_owner: None,
            returns_self: false,
        });
        let key = self.field(declaration(0), name, Type::Never, Some(application));
        self.members[key.index as usize].signatures = Some(vec![head]);
        key
    }

    fn semantic_self(&mut self, owner: NativeTypeDeclarationKey) {
        let key = parameter(self.parameters.len() as u32);
        self.parameters.push(ParameterSource {
            owner: TypeDeclarationOwner::NamedType(owner),
            constraint: None,
        });
        self.declarations[owner.index as usize].self_parameter = Some(key);
    }

    fn deferred_owner_field(&mut self) -> NativeTypeMemberKey {
        let scope = function(self.functions.len() as u32);
        let settled_owner = reference(declaration(0), vec![Type::String]);
        self.functions.push(SignatureSource {
            parameters: vec![],
            returns: settled_owner.clone(),
            own: vec![],
            outer: vec![],
            applied_self_owner: None,
            returns_self: false,
        });
        let member = self.field(declaration(0), "deferred", Type::Boolean, None);
        self.owner_dependency = Some(OwnerDependency {
            member,
            scope,
            settled_owner,
        });
        self.owner_pending.set(true);
        member
    }

    fn derived(&mut self, argument: Type) -> NativeTypeDeclarationKey {
        let key = declaration(self.declarations.len() as u32);
        self.declarations.push(Declaration {
            parameters: vec![],
            outer: vec![],
            alias: None,
            self_parameter: None,
            bases: vec![reference(declaration(0), vec![argument])],
        });
        self.field(key, "extra", Type::String, None);
        key
    }

    fn applied_self_method(
        &mut self,
        runtime: bool,
        argument: Type,
        captures: Vec<NativeTypeParameterKey>,
        own_generic_return: bool,
    ) {
        let owner = reference(declaration(0), vec![argument.clone()]);
        let own = parameter(self.parameters.len() as u32);
        let (scope, value, heads) = if runtime {
            let key = function(self.functions.len() as u32);
            (
                TypeDeclarationOwner::Function(key),
                Type::NativeFunction(key),
                None,
            )
        } else {
            let key = signature(self.signatures.len() as u32);
            (
                TypeDeclarationOwner::Signature(key),
                Type::Never,
                Some(vec![key]),
            )
        };
        if own_generic_return {
            self.parameters.push(ParameterSource {
                owner: scope,
                constraint: None,
            });
        }
        let callable = SignatureSource {
            // The exact mapped self instance is obtained lazily from the
            // original owner recipe; it is deliberately absent from outer IDs.
            parameters: if own_generic_return {
                vec![formal(own)]
            } else {
                vec![argument]
            },
            returns: if own_generic_return {
                formal(own)
            } else {
                Type::Never
            },
            own: if own_generic_return {
                vec![own]
            } else {
                vec![]
            },
            outer: captures.clone(),
            applied_self_owner: Some(owner.clone()),
            returns_self: !own_generic_return,
        };
        if runtime {
            self.functions.push(callable);
        } else {
            self.signatures.push(callable);
        }
        let key = self.field(
            declaration(0),
            "fluent",
            value,
            Some(MemberOwnerApplication {
                reference: owner,
                parameters: captures,
            }),
        );
        self.members[key.index as usize].signatures = heads;
    }

    fn source_parameters(
        source: &SignatureSource,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        let mut parameters = Vec::new();
        if let Some(owner) = &source.applied_self_owner {
            parameters.push(Parameter {
                name: "self".into(),
                ty: queries
                    .contextual_owner_self_type(owner)
                    .expect("original mapped semantic self"),
                optional: false,
            });
        }
        parameters.extend(
            source
                .parameters
                .iter()
                .enumerate()
                .map(|(index, ty)| Parameter {
                    name: format!("arg{index}"),
                    ty: ty.clone(),
                    optional: false,
                }),
        );
        parameters
    }

    fn source_return(source: &SignatureSource, queries: &mut dyn TypeQueries) -> Type {
        if source.returns_self {
            queries
                .contextual_owner_self_type(
                    source.applied_self_owner.as_ref().expect("applied owner"),
                )
                .expect("original mapped semantic self")
        } else {
            source.returns.clone()
        }
    }

    fn source_outer_instances(
        source: &SignatureSource,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Type> {
        source
            .applied_self_owner
            .as_ref()
            .map(|owner| {
                queries
                    .contextual_owner_self_type(owner)
                    .expect("original mapped semantic self")
            })
            .into_iter()
            .collect()
    }
}

impl NativeTypeProvider for Provider {
    fn contextual_type_owner(
        &self,
        scope: NativeFunctionKey,
        input: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> ContextualTypeOwner {
        let Some(source) = self
            .owner_dependency
            .as_ref()
            .filter(|source| source.scope == scope)
        else {
            return ContextualTypeOwner::Absent;
        };
        assert_eq!(input, owner_input());
        self.owner_calls.set(self.owner_calls.get() + 1);
        if self.owner_pending.get() {
            ContextualTypeOwner::Pending
        } else {
            ContextualTypeOwner::Owner(source.settled_owner.clone())
        }
    }
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool {
        key.snapshot == SNAPSHOT && (key.index as usize) < self.declarations.len()
    }
    fn type_declaration_self_parameter(
        &self,
        key: NativeTypeDeclarationKey,
    ) -> Option<NativeTypeParameterKey> {
        self.declarations[key.index as usize].self_parameter
    }
    fn type_declaration_bases(
        &self,
        key: NativeTypeDeclarationKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Type> {
        self.declarations[key.index as usize].bases.clone()
    }
    fn type_declaration_owner_scope(&self, key: NativeTypeDeclarationKey) -> OwnerScopeRecipe {
        match &self.declarations[key.index as usize].alias {
            Some((owner, _)) => OwnerScopeRecipe::AliasReference {
                candidates: vec![*owner],
            },
            None => OwnerScopeRecipe::Original(TypeDeclarationKind::Class),
        }
    }
    fn type_declaration_header(&self, key: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        let source = &self.declarations[key.index as usize];
        TypeDeclarationHeader {
            kind: if source.alias.is_some() {
                TypeDeclarationKind::Alias
            } else {
                TypeDeclarationKind::Class
            },
            name: format!("Owner{}", key.index),
            object_form: source
                .alias
                .is_none()
                .then_some(ObjectDeclarationForm::Record),
            parameters: source.parameters.clone(),
            outer_parameters: source.outer.clone(),
        }
    }
    fn type_alias_target(&self, key: NativeTypeDeclarationKey, _: &mut dyn TypeQueries) -> Type {
        self.declarations[key.index as usize]
            .alias
            .as_ref()
            .expect("original alias")
            .1
            .clone()
    }
    fn type_declaration_members(&self, _: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        // Original augmentations are candidates. The native query decides
        // canonical ownership and argument applicability before demanding them.
        (0..self.members.len())
            .map(|index| member(index as u32))
            .collect()
    }
    fn contains_type_member(&self, key: NativeTypeMemberKey) -> bool {
        key.snapshot == SNAPSHOT && (key.index as usize) < self.members.len()
    }
    fn type_member_header(&self, key: NativeTypeMemberKey) -> TypeMemberHeader {
        let source = &self.members[key.index as usize];
        TypeMemberHeader {
            owner: source.owner.clone(),
            key: source.key.clone(),
            visibility: TypeMemberVisibility::Public,
            optional: false,
            readonly: false,
        }
    }
    fn type_member_is_assignment(&self, key: NativeTypeMemberKey) -> bool {
        self.assignments.contains(&key)
    }
    fn type_member_origin(&self, key: NativeTypeMemberKey) -> TableFieldOrigin {
        if self.assignments.contains(&key) { TableFieldOrigin::Inferred } else { TableFieldOrigin::Declared }
    }
    fn type_member_owner_application(
        &self,
        key: NativeTypeMemberKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<MemberOwnerApplication> {
        if let Some(source) = &self.owner_dependency
            && key == source.member
        {
            self.application_calls.set(self.application_calls.get() + 1);
            // Deliberately return None after observing Pending. The native
            // query must preserve that dependency's incompleteness.
            return match queries.contextual_type_owner(source.scope, owner_input()) {
                ContextualTypeOwner::Pending | ContextualTypeOwner::Absent => None,
                ContextualTypeOwner::Owner(reference) => Some(MemberOwnerApplication {
                    reference,
                    parameters: vec![],
                }),
            };
        }
        self.members[key.index as usize].application.clone()
    }
    fn type_member_type(&self, key: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        let source = &self.members[key.index as usize];
        assert!(
            source.signatures.is_none(),
            "pure heads use their original signature keys"
        );
        self.reads.set(self.reads.get() + 1);
        source.value.clone()
    }
    fn type_member_signatures(&self, key: NativeTypeMemberKey) -> Option<Vec<NativeSignatureKey>> {
        self.members[key.index as usize].signatures.clone()
    }
    fn type_indexer_key(&self, key: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        self.members[key.index as usize]
            .indexer_key
            .clone()
            .expect("original indexer key")
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        key.snapshot == SNAPSHOT && (key.index as usize) < self.parameters.len()
    }
    fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String {
        // Equal spelling must never merge the class and lexical identities.
        "T".into()
    }
    fn type_parameter_owner(&self, key: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> {
        Some(self.parameters[key.index as usize].owner)
    }
    fn has_type_parameter_constraint(&self, key: NativeTypeParameterKey) -> bool {
        self.parameters[key.index as usize].constraint.is_some()
            || self
                .declarations
                .iter()
                .any(|source| source.self_parameter == Some(key))
    }
    fn type_parameter_constraint(
        &self,
        key: NativeTypeParameterKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        if let Some(index) = self
            .declarations
            .iter()
            .position(|source| source.self_parameter == Some(key))
        {
            return Some(
                queries
                    .named_declared_type(declaration(index as u32))
                    .unwrap(),
            );
        }
        self.parameters[key.index as usize].constraint.clone()
    }
    fn contains_signature(&self, key: NativeSignatureKey) -> bool {
        key.snapshot == SNAPSHOT && (key.index as usize) < self.signatures.len()
    }
    fn signature_header(&self, key: NativeSignatureKey) -> SignatureHeader {
        SignatureHeader {
            owner: SignatureOwner::Standalone,
            site: SignatureSite {
                snapshot: key.snapshot,
                index: key.index,
            },
            parent: None,
            has_direct_literal_parameter: false,
        }
    }
    fn signature_type_parameters(&self, key: NativeSignatureKey) -> Vec<NativeTypeParameterKey> {
        self.signatures[key.index as usize].own.clone()
    }
    fn signature_outer_type_parameters(
        &self,
        key: NativeSignatureKey,
    ) -> Vec<NativeTypeParameterKey> {
        self.signatures[key.index as usize].outer.clone()
    }
    fn signature_outer_type_parameter_instances(
        &self,
        key: NativeSignatureKey,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Type> {
        Self::source_outer_instances(&self.signatures[key.index as usize], queries)
    }
    fn signature_parameters(
        &self,
        key: NativeSignatureKey,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        Self::source_parameters(&self.signatures[key.index as usize], queries)
    }
    fn signature_declared_return_pack(
        &self,
        key: NativeSignatureKey,
        queries: &mut dyn TypeQueries,
    ) -> ValuePack {
        ValuePack::single(Self::source_return(
            &self.signatures[key.index as usize],
            queries,
        ))
    }
    fn type_declaration_error(&self, error: NamedTypeError) {
        self.errors.borrow_mut().push(error);
    }
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        key.snapshot == SNAPSHOT && (key.index as usize) < self.functions.len()
    }
    fn function_parameters(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        Self::source_parameters(&self.functions[key.index as usize], queries)
    }
    fn function_declared_return(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        if self.unrelated_inference_function == Some(key) {
            return None;
        }
        Some(Self::source_return(
            &self.functions[key.index as usize],
            queries,
        ))
    }
    fn function_type_parameters(
        &self,
        key: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<NativeTypeParameterKey> {
        self.functions[key.index as usize].own.clone()
    }
    fn function_outer_type_parameters(
        &self,
        key: NativeFunctionKey,
    ) -> Vec<NativeTypeParameterKey> {
        self.functions[key.index as usize].outer.clone()
    }
    fn function_outer_type_parameter_instances(
        &self,
        key: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Type> {
        if let Some(instance) = self.invalid_outer_instance.borrow().as_ref() {
            return vec![instance.clone()];
        }
        Self::source_outer_instances(&self.functions[key.index as usize], queries)
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        self.inferred_return_demands
            .set(self.inferred_return_demands.get() + 1);
        panic!("no runtime body")
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("no runtime function")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("no runtime function")
    }
}

fn application(
    owner: NativeTypeDeclarationKey,
    arguments: Vec<Type>,
    parameters: Vec<NativeTypeParameterKey>,
) -> MemberOwnerApplication {
    MemberOwnerApplication {
        reference: reference(owner, arguments),
        parameters,
    }
}
fn call(queries: &mut dyn TypeQueries, callable: &Type, arguments: &[Type]) -> Type {
    let result = queries.call(callable, arguments);
    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    result.return_type
}

#[test]
fn direct_indexer_store_context_maps_the_original_captured_key_before_matching() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            let lexical = provider.lexical(None);
            let indexer = provider.field(
                declaration(0), "indexer", Type::String,
                Some(application(declaration(0), vec![formal(lexical)], vec![lexical])),
            );
            provider.members[indexer.index as usize].key = TypeMemberKeyRecipe::Indexer { sparse: false };
            provider.members[indexer.index as usize].indexer_key = Some(formal(lexical));
            let numeric = provider.field(
                declaration(0), "numeric", Type::Never,
                Some(application(declaration(0), vec![Type::Number], vec![])),
            );
            provider.members[numeric.index as usize].key = TypeMemberKeyRecipe::Fixed(LuaKey::Integer(3));
            let outside = provider.field(
                declaration(0), "outside", Type::Never,
                Some(application(declaration(0), vec![Type::Number], vec![])),
            );
            provider.assignments.extend([numeric, outside]);
            provider
        },
        |provider, queries| {
            assert_eq!(queries.named_member_context(member(1)), Ok(Some(Type::String)));
            assert_eq!(queries.named_member_context(member(2)), Ok(None));
            let numeric = queries.named_type_reference(declaration(0), &[Type::Number]).unwrap();
            assert_eq!(queries.table_lookup(&numeric, &TableKey::Literal(LuaKey::Integer(3))), Ok(Type::String));
            assert_eq!(provider.reads.get(), 1, "the assignment RHS cannot define its written indexer contract");
            assert!(provider.errors.borrow().is_empty(), "{:#?}", provider.errors.borrow());
        },
    );
}

#[test]
fn scalar_owner_application_maps_lexical_parameter_without_changing_class_formal() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            let lexical = provider.lexical(None);
            provider.field(declaration(0), "declared", formal(parameter(0)), None);
            provider.field(
                declaration(0),
                "captured",
                formal(lexical),
                Some(application(
                    declaration(0),
                    vec![formal(lexical)],
                    vec![lexical],
                )),
            );
            provider
        },
        |provider, queries| {
            for argument in [Type::Number, Type::String] {
                let owner = queries
                    .named_type_reference(declaration(0), &[argument.clone()])
                    .unwrap();
                assert_eq!(
                    queries.property_type(&owner, "captured"),
                    Some(argument.clone())
                );
                assert_eq!(queries.property_type(&owner, "declared"), Some(argument));
            }
            assert_eq!(
                provider.reads.get(),
                2,
                "raw members stay cached in their original scopes"
            );
            assert!(
                provider.errors.borrow().is_empty(),
                "{:#?}",
                provider.errors.borrow()
            );
        },
    );
}

#[test]
fn pure_method_owner_application_maps_parameters_and_returns_per_accessed_owner() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            let lexical = provider.lexical(None);
            provider.method(
                "echo",
                vec![formal(lexical)],
                formal(lexical),
                vec![lexical],
                application(declaration(0), vec![formal(lexical)], vec![lexical]),
            );
            provider
        },
        |_, queries| {
            for argument in [Type::Number, Type::String] {
                let owner = queries
                    .named_type_reference(declaration(0), &[argument.clone()])
                    .unwrap();
                let method = queries.property_type(&owner, "echo").unwrap();
                assert_eq!(queries.function_implementation_type(&method), None);
                assert_eq!(call(queries, &method, &[argument.clone()]), argument);
                assert!(!queries.call(&method, &[Type::Boolean]).errors.is_empty());
            }
        },
    );
}

#[test]
fn owner_capture_mapper_preserves_the_method_own_generic_parameter() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            let lexical = provider.lexical(None);
            let key = provider.method(
                "select",
                vec![formal(lexical)],
                Type::Never,
                vec![lexical],
                application(declaration(0), vec![formal(lexical)], vec![lexical]),
            );
            let head = provider.members[key.index as usize]
                .signatures
                .as_ref()
                .unwrap()[0];
            let own = parameter(provider.parameters.len() as u32);
            provider.parameters.push(ParameterSource {
                owner: TypeDeclarationOwner::Signature(head),
                constraint: None,
            });
            let head = &mut provider.signatures[head.index as usize];
            head.own.push(own);
            head.parameters.push(formal(own));
            head.returns = formal(own);
            provider
        },
        |_, queries| {
            for argument in [Type::Number, Type::String] {
                let owner = queries
                    .named_type_reference(declaration(0), &[argument.clone()])
                    .unwrap();
                let method = queries.property_type(&owner, "select").unwrap();
                assert_eq!(
                    call(queries, &method, &[argument, Type::Boolean]),
                    Type::Boolean
                );
            }
        },
    );
}

#[test]
fn fixed_owner_application_keeps_augmentation_on_its_original_specialization() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            let alias = provider.alias(reference(declaration(0), vec![Type::String]), vec![]);
            let key = provider.field(
                declaration(0),
                "textOnly",
                Type::Boolean,
                Some(application(alias, vec![], vec![])),
            );
            provider.members[key.index as usize].owner = TypeMemberOwner::Reference {
                name: "TextBox".into(),
                candidates: vec![alias],
            };
            provider
        },
        |provider, queries| {
            let numeric = queries
                .named_type_reference(declaration(0), &[Type::Number])
                .unwrap();
            assert_eq!(queries.property_type(&numeric, "textOnly"), None);
            assert_eq!(
                provider.reads.get(),
                0,
                "a rejected specialization does not demand its field"
            );
            let textual = queries
                .named_type_reference(declaration(0), &[Type::String])
                .unwrap();
            assert_eq!(
                queries.property_type(&textual, "textOnly"),
                Some(Type::Boolean)
            );
            assert!(provider.errors.borrow().is_empty());
        },
    );
}

#[test]
fn captured_alias_owner_uses_its_original_outer_parameters_without_an_application_override() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            let lexical = provider.lexical(None);
            let alias = provider.alias(
                reference(declaration(0), vec![formal(lexical)]),
                vec![lexical],
            );
            let key = provider.field(declaration(0), "captured", formal(lexical), None);
            provider.members[key.index as usize].owner = TypeMemberOwner::Reference {
                name: "CapturedBox".into(),
                candidates: vec![alias],
            };
            provider
        },
        |_, queries| {
            for argument in [Type::Number, Type::String] {
                let owner = queries
                    .named_type_reference(declaration(0), &[argument.clone()])
                    .unwrap();
                assert_eq!(queries.property_type(&owner, "captured"), Some(argument));
            }
        },
    );
}

#[test]
fn constrained_lexical_owner_parameter_does_not_widen_augmentation_to_other_arguments() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            let lexical = provider.lexical(Some(Type::String));
            provider.field(
                declaration(0),
                "text",
                formal(lexical),
                Some(application(
                    declaration(0),
                    vec![formal(lexical)],
                    vec![lexical],
                )),
            );
            provider
        },
        |_, queries| {
            let numeric = queries
                .named_type_reference(declaration(0), &[Type::Number])
                .unwrap();
            assert_eq!(queries.property_type(&numeric, "text"), None);
            for argument in [Type::String, literal("word")] {
                let textual = queries
                    .named_type_reference(declaration(0), &[argument.clone()])
                    .unwrap();
                let value = queries.property_type(&textual, "text").unwrap();
                assert!(queries.is_assignable(&value, &argument));
                assert!(queries.is_assignable(&argument, &value));
                assert!(!queries.is_assignable(&Type::Number, &value));
                if matches!(argument, Type::StringLiteral(_)) {
                    assert!(!queries.is_assignable(&Type::String, &value));
                }
            }
        },
    );
}

#[test]
fn nested_array_owner_pattern_infers_element_and_rejects_scalar_arguments() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            let lexical = provider.lexical(None);
            provider.field(
                declaration(0),
                "element",
                formal(lexical),
                Some(application(
                    declaration(0),
                    vec![array(formal(lexical))],
                    vec![lexical],
                )),
            );
            provider
        },
        |_, queries| {
            let scalar = queries
                .named_type_reference(declaration(0), &[Type::Number])
                .unwrap();
            assert_eq!(queries.property_type(&scalar, "element"), None);
            for element in [Type::Number, Type::String] {
                let owner = queries
                    .named_type_reference(declaration(0), &[array(element.clone())])
                    .unwrap();
                assert_eq!(queries.property_type(&owner, "element"), Some(element));
            }
        },
    );
}

#[test]
fn repeated_owner_parameter_requires_all_argument_equations_to_agree() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            let lexical = provider.lexical(None);
            provider.field(
                declaration(1),
                "shared",
                formal(lexical),
                Some(application(
                    declaration(1),
                    vec![formal(lexical), formal(lexical)],
                    vec![lexical],
                )),
            );
            provider
        },
        |_, queries| {
            let mixed = queries
                .named_type_reference(declaration(1), &[Type::Number, Type::String])
                .unwrap();
            assert_eq!(queries.property_type(&mixed, "shared"), None);
            for argument in [Type::Number, Type::String] {
                let same = queries
                    .named_type_reference(declaration(1), &[argument.clone(), argument.clone()])
                    .unwrap();
                assert_eq!(queries.property_type(&same, "shared"), Some(argument));
            }
        },
    );
}

#[test]
fn grouped_pure_heads_preserve_each_alias_lexical_capture_mapper() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            for label in ["first", "second"] {
                let lexical = provider.lexical(None);
                let alias = provider.alias(
                    reference(declaration(0), vec![formal(lexical)]),
                    vec![lexical],
                );
                let key = provider.method(
                    "choose",
                    vec![literal(label)],
                    formal(lexical),
                    vec![lexical],
                    application(alias, vec![], vec![lexical]),
                );
                provider.members[key.index as usize].owner = TypeMemberOwner::Reference {
                    name: label.into(),
                    candidates: vec![alias],
                };
            }
            provider
        },
        |_, queries| {
            for argument in [Type::Number, Type::String] {
                let owner = queries
                    .named_type_reference(declaration(0), &[argument.clone()])
                    .unwrap();
                let method = queries.property_type(&owner, "choose").unwrap();
                assert_eq!(queries.call_signature_types(&method).len(), 2);
                assert_eq!(queries.function_implementation_type(&method), None);
                for label in ["first", "second"] {
                    assert_eq!(call(queries, &method, &[literal(label)]), argument);
                }
            }
        },
    );
}

#[test]
fn owner_application_maps_indexer_key_and_value_in_the_same_lexical_scope() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            let lexical = provider.lexical(Some(Type::String));
            let key = provider.field(
                declaration(0),
                "unused",
                formal(lexical),
                Some(application(
                    declaration(0),
                    vec![formal(lexical)],
                    vec![lexical],
                )),
            );
            let source = &mut provider.members[key.index as usize];
            source.key = TypeMemberKeyRecipe::Indexer { sparse: false };
            source.indexer_key = Some(formal(lexical));
            provider
        },
        |_, queries| {
            let owner = queries
                .named_type_reference(declaration(0), &[literal("only")])
                .unwrap();
            let key = TableKey::Literal(LuaKey::String(b"only".to_vec()));
            let value = queries.table_lookup(&owner, &key).unwrap();
            assert!(queries.is_assignable(&value, &literal("only")));
            assert!(queries.is_assignable(&literal("only"), &value));
            assert!(!queries.is_assignable(&literal("else"), &value));
            let wrong = TableKey::Literal(LuaKey::String(b"else".to_vec()));
            assert!(queries.table_lookup(&owner, &wrong).is_err());
        },
    );
}

#[test]
fn mapped_self_parameter_and_result_survive_capture_concrete_and_derived_receivers() {
    for runtime in [false, true] {
        Engine::new().with_native_provider(
            move || {
                let mut provider = Provider::new();
                provider.semantic_self(declaration(0));
                let lexical = provider.lexical(None);
                provider.applied_self_method(runtime, formal(lexical), vec![lexical], false);
                provider.field(declaration(0), "value", formal(parameter(0)), None);
                provider.derived(Type::Number);
                provider.derived(Type::String);
                provider
            },
            |_, queries| {
                for (index, argument) in [Type::Number, Type::String].into_iter().enumerate() {
                    let concrete = queries
                        .named_type_reference(declaration(0), &[argument.clone()])
                        .unwrap();
                    let derived = queries
                        .named_declared_type(declaration(2 + index as u32))
                        .unwrap();
                    for receiver in [concrete, derived.clone()] {
                        let method = queries.property_type(&receiver, "fluent").unwrap();
                        let result = call(queries, &method, &[receiver.clone(), argument.clone()]);
                        assert_eq!(result, receiver, "runtime={runtime}");
                        assert_eq!(
                            queries.property_type(&result, "value"),
                            Some(argument.clone())
                        );
                        assert!(
                            !queries
                                .call(&method, &[receiver, Type::Boolean])
                                .errors
                                .is_empty()
                        );
                    }
                    let method = queries.property_type(&derived, "fluent").unwrap();
                    let result = call(queries, &method, &[derived, argument]);
                    assert_eq!(queries.property_type(&result, "extra"), Some(Type::String));
                }
            },
        );
    }
}

#[test]
fn fixed_applied_self_keeps_derived_receiver_and_owner_specialization() {
    for runtime in [false, true] {
        Engine::new().with_native_provider(
            move || {
                let mut provider = Provider::new();
                provider.semantic_self(declaration(0));
                provider.applied_self_method(runtime, Type::String, vec![], false);
                provider.derived(Type::String);
                provider
            },
            |_, queries| {
                let numeric = queries
                    .named_type_reference(declaration(0), &[Type::Number])
                    .unwrap();
                assert_eq!(queries.property_type(&numeric, "fluent"), None);
                let textual = queries
                    .named_type_reference(declaration(0), &[Type::String])
                    .unwrap();
                let derived = queries.named_declared_type(declaration(2)).unwrap();
                for receiver in [textual, derived.clone()] {
                    let method = queries.property_type(&receiver, "fluent").unwrap();
                    assert_eq!(
                        call(queries, &method, &[receiver.clone(), Type::String]),
                        receiver
                    );
                }
                let method = queries.property_type(&derived, "fluent").unwrap();
                let result = call(queries, &method, &[derived, Type::String]);
                assert_eq!(queries.property_type(&result, "extra"), Some(Type::String));
            },
        );
    }
}

#[test]
fn mapped_self_outer_instance_keeps_callable_own_generic_inference_independent() {
    for runtime in [false, true] {
        Engine::new().with_native_provider(
            move || {
                let mut provider = Provider::new();
                provider.semantic_self(declaration(0));
                let lexical = provider.lexical(None);
                provider.applied_self_method(runtime, formal(lexical), vec![lexical], true);
                provider.derived(Type::Number);
                provider
            },
            |_, queries| {
                let owner = queries
                    .named_type_reference(declaration(0), &[Type::Number])
                    .unwrap();
                let derived = queries.named_declared_type(declaration(2)).unwrap();
                for receiver in [owner, derived] {
                    let method = queries.property_type(&receiver, "fluent").unwrap();
                    assert_eq!(
                        call(queries, &method, &[receiver.clone(), Type::Boolean]),
                        Type::Boolean
                    );
                    assert_eq!(
                        call(queries, &method, &[receiver, Type::String]),
                        Type::String
                    );
                }
            },
        );
    }
}

#[test]
fn owner_application_none_after_pending_does_not_fall_back_to_a_raw_member() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            provider.deferred_owner_field();
            provider
        },
        |provider, queries| {
            let key = provider.owner_dependency.as_ref().unwrap().member;
            let result = queries.validate_named_member(key);
            assert!(
                matches!(&result, Err(NamedTypeError::PendingContext { member }) if *member == key),
                "{result:#?}"
            );
            assert_eq!(
                provider.reads.get(),
                0,
                "an incomplete owner must not expose the fallback field"
            );
            assert!(provider.owner_calls.get() > 0);
        },
    );
}

#[test]
fn owner_application_none_after_pending_retries_the_settled_specialization() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            provider.deferred_owner_field();
            provider
        },
        |provider, queries| {
            let numeric = queries
                .named_type_reference(declaration(0), &[Type::Number])
                .unwrap();
            let access = TableKey::Literal(LuaKey::String(b"deferred".to_vec()));
            assert_eq!(
                queries.table_lookup(&numeric, &access),
                Err(TableError::PendingContext)
            );
            assert_eq!(provider.reads.get(), 0);
            let pending_applications = provider.application_calls.get();
            let pending_owners = provider.owner_calls.get();
            provider.owner_pending.set(false);

            assert!(matches!(
                queries.table_lookup(&numeric, &access),
                Err(TableError::MissingKey { .. })
            ));
            assert_eq!(
                provider.reads.get(),
                0,
                "the settled owner excludes numeric boxes"
            );
            let textual = queries
                .named_type_reference(declaration(0), &[Type::String])
                .unwrap();
            assert_eq!(queries.table_lookup(&textual, &access), Ok(Type::Boolean));
            assert!(
                provider.application_calls.get() > pending_applications,
                "Pending must not become cached absence"
            );
            assert!(
                provider.owner_calls.get() > pending_owners,
                "settlement retries the original owner dependency"
            );
            assert!(
                provider
                    .errors
                    .borrow()
                    .iter()
                    .all(|error| matches!(error, NamedTypeError::PendingContext { .. }))
            );
        },
    );
}

fn register_invalid_own_outer_instance(mapped: bool) {
    with_program(|program, handles| {
        let mut provider = Provider::new();
        let scope = function(0);
        let own = parameter(provider.parameters.len() as u32);
        provider.parameters.push(ParameterSource {
            owner: TypeDeclarationOwner::Function(scope),
            constraint: None,
        });
        provider.functions.push(SignatureSource {
            parameters: vec![formal(own)],
            returns: formal(own),
            own: vec![own],
            outer: vec![],
            applied_self_owner: None,
            returns_self: false,
        });
        let adapter = ProviderAdapter {
            provider: &provider,
            handles,
        };
        let mut checker = program.checker_with_native_provider(&adapter);
        checker.set_native_type_declaration_provider(&adapter);
        let declared = checker
            .native_type_parameter(core_parameter_key(own))
            .unwrap();
        let instance = if mapped {
            let outer = checker
                .native_type_parameter(core_parameter_key(parameter(0)))
                .unwrap();
            let mapper = checker.mapper_from(&[outer], &[TypeId::NUMBER]);
            let instance = checker.cloned_native_type_param(core_parameter_key(own), mapper);
            assert_ne!(
                instance, declared,
                "the rejection must cover an exact mapped clone"
            );
            instance
        } else {
            declared
        };
        // Intentionally violate the provider's free-outer-only contract using
        // a capability from this checker session, without fabricating IDs.
        *provider.invalid_outer_instance.borrow_mut() =
            Some(export_type(&mut checker, handles, instance));
        checker.native_function_type(core_key(scope));
    });
}

#[test]
#[should_panic(expected = "native outer instance must not bind a callable's own parameter")]
fn function_outer_instance_callback_rejects_its_own_generic_parameter() {
    register_invalid_own_outer_instance(false);
}

#[test]
#[should_panic(expected = "native outer instance must not bind a callable's own parameter")]
fn function_outer_instance_callback_rejects_a_mapped_clone_of_its_own_generic_parameter() {
    register_invalid_own_outer_instance(true);
}

#[test]
fn declared_pure_member_uses_class_metadata_without_unrelated_runtime_inference() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            let key = provider.method(
                "convert",
                vec![formal(parameter(0))],
                formal(parameter(0)),
                vec![parameter(0)],
                application(declaration(0), vec![formal(parameter(0))], vec![]),
            );
            // A written class-owned contract has no applied-owner recipe.
            provider.members[key.index as usize].application = None;

            let unrelated = function(0);
            provider.functions.push(SignatureSource {
                parameters: vec![],
                returns: Type::Never,
                own: vec![],
                outer: vec![],
                applied_self_owner: None,
                returns_self: false,
            });
            provider.unrelated_inference_function = Some(unrelated);
            provider.field(
                declaration(0),
                "unrelated",
                Type::NativeFunction(unrelated),
                None,
            );
            provider
        },
        |provider, queries| {
            let owner = queries
                .named_type_reference(declaration(0), &[Type::Number])
                .unwrap();
            let contract = queries.property_type(&owner, "convert").unwrap();
            assert_eq!(queries.function_implementation_type(&contract), None);
            assert_eq!(call(queries, &contract, &[Type::Number]), Type::Number);
            assert!(!queries.call(&contract, &[Type::String]).errors.is_empty());
            assert_eq!(
                provider.reads.get(),
                0,
                "the written pure contract does not demand runtime sibling values"
            );
            assert_eq!(
                provider.inferred_return_demands.get(),
                0,
                "written member substitution must bypass runtime body inference"
            );
        },
    );
}
