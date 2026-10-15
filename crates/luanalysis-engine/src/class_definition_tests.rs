use super::*;

fn owner(index: u32) -> NativeClassDefinitionKey {
    NativeClassDefinitionKey {
        snapshot: 91001,
        index,
    }
}
struct DefinitionHead {
    owner: SignatureOwner,
    input: Type,
    returned: Type,
}
struct Definitions {
    base: Provider,
    heads: Vec<DefinitionHead>,
}
fn signature(index: u32) -> NativeSignatureKey { NativeSignatureKey { snapshot: 91001, index } }
fn runtime(index: u32) -> NativeFunctionKey { NativeFunctionKey { snapshot: 91001, index } }
impl NativeTypeProvider for Definitions {
    fn contains_class_definition(&self, id: NativeClassDefinitionKey) -> bool {
        id.snapshot == 91001
            && self
                .base
                .declarations
                .iter()
                .any(|decl| decl.key == key(id.index) && decl.kind == TypeDeclarationKind::Class)
    }
    fn class_definition_header(&self, id: NativeClassDefinitionKey) -> ClassDefinitionHeader {
        ClassDefinitionHeader {
            declaration: key(id.index),
        }
    }
    fn contains_type_declaration(&self, id: NativeTypeDeclarationKey) -> bool {
        self.base.contains_type_declaration(id)
    }
    fn type_declaration_header(&self, id: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        self.base.type_declaration_header(id)
    }
    fn contains_type_member(&self, id: NativeTypeMemberKey) -> bool {
        self.base.contains_type_member(id)
    }
    fn type_member_header(&self, id: NativeTypeMemberKey) -> TypeMemberHeader {
        let mut header = self.base.type_member_header(id);
        if id.index / 100 == 7 { header.key = TypeMemberKeyRecipe::Fixed(LuaKey::Integer(1)); }
        header
    }
    fn type_declaration_members(&self, id: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        self.base.type_declaration_members(id)
    }
    fn type_member_type(&self, id: NativeTypeMemberKey, q: &mut dyn TypeQueries) -> Type {
        self.base.type_member_type(id, q)
    }
    fn type_indexer_key(&self, id: NativeTypeMemberKey, q: &mut dyn TypeQueries) -> Type {
        self.base.type_indexer_key(id, q)
    }
    fn type_alias_target(&self, id: NativeTypeDeclarationKey, q: &mut dyn TypeQueries) -> Type {
        self.base.type_alias_target(id, q)
    }
    fn type_declaration_bases(
        &self,
        id: NativeTypeDeclarationKey,
        q: &mut dyn TypeQueries,
    ) -> Vec<Type> {
        self.base.type_declaration_bases(id, q)
    }
    fn contains_type_parameter(&self, id: NativeTypeParameterKey) -> bool {
        self.base.contains_type_parameter(id)
    }
    fn type_parameter_name(&self, id: NativeTypeParameterKey) -> String {
        self.base.type_parameter_name(id)
    }
    fn type_parameter_owner(&self, id: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> {
        self.base.type_parameter_owner(id)
    }
    fn has_type_parameter_constraint(&self, id: NativeTypeParameterKey) -> bool { self.base.has_type_parameter_constraint(id) }
    fn type_parameter_constraint(&self, id: NativeTypeParameterKey, q: &mut dyn TypeQueries) -> Option<Type> { self.base.type_parameter_constraint(id, q) }
    fn type_declaration_call_signatures(&self, id: NativeTypeDeclarationKey) -> Vec<NativeSignatureKey> {
        self.heads.iter().enumerate().filter(|(_, head)| head.owner == SignatureOwner::NamedType(id)).map(|(index, _)| signature(index as u32)).collect()
    }
    fn contains_signature(&self, id: NativeSignatureKey) -> bool { id.snapshot == 91001 && (id.index as usize) < self.heads.len() }
    fn signature_header(&self, id: NativeSignatureKey) -> SignatureHeader {
        SignatureHeader { owner: self.heads[id.index as usize].owner, site: SignatureSite { snapshot: id.snapshot, index: id.index }, parent: None, has_direct_literal_parameter: false }
    }
    fn signature_outer_type_parameters(&self, id: NativeSignatureKey) -> Vec<NativeTypeParameterKey> {
        match self.heads[id.index as usize].owner {
            SignatureOwner::NamedType(id) if self.base.decl(id).generic => vec![parameter()],
            _ => vec![],
        }
    }
    fn signature_parameters(&self, id: NativeSignatureKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        vec![Parameter { name: "arg".into(), ty: self.heads[id.index as usize].input.clone(), optional: false }]
    }
    fn signature_declared_return_pack(&self, id: NativeSignatureKey, _: &mut dyn TypeQueries) -> ValuePack {
        ValuePack::single(self.heads[id.index as usize].returned.clone())
    }
    fn function_overload_signatures(&self, id: NativeFunctionKey) -> Vec<NativeSignatureKey> {
        self.heads.iter().enumerate().filter(|(_, head)| head.owner == SignatureOwner::Function(id)).map(|(index, _)| signature(index as u32)).collect()
    }
    fn contains_function(&self, id: NativeFunctionKey) -> bool {
        id.snapshot == 91001 && id.index < 2 && self.heads.iter().any(|head| matches!(head.owner, SignatureOwner::Function(_)))
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { vec![] }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { None }
    fn function_inferred_return(&self, id: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        if id.index == 1 { Type::NativeFunction(runtime(0)) } else { Type::Any }
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("no runtime functions")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("no runtime functions")
    }
}
fn definitions() -> Definitions {
    Definitions { base: basic(), heads: vec![] }
}
fn value_key() -> TableKey {
    TableKey::Literal(LuaKey::String(b"value".to_vec()))
}

#[test]
fn definition_factory_requires_explicit_owner_and_matching_class_table() {
    Engine::new().with_native_provider(definitions, |_, q| {
        let class = q.named_type_reference(key(2), &[]).unwrap();
        assert!(matches!(
            q.class_definition_value(
                NativeClassDefinitionKey {
                    snapshot: 1,
                    index: 2
                },
                &class,
                &empty()
            ),
            Err(NamedTypeError::InvalidDefinition(_))
        ));
        assert!(matches!(
            q.class_definition_context(owner(0), &class),
            Err(NamedTypeError::DefinitionOwner { .. })
        ));
        assert!(matches!(
            q.class_definition_value(owner(2), &class, &Type::Number),
            Err(NamedTypeError::DefinitionInitializer { .. })
        ));
        assert!(
            matches!(
                q.class_definition_value(owner(2), &class, &class),
                Err(NamedTypeError::DefinitionInitializer { .. })
            ),
            "ordinary instances cannot be relabeled definitionvalues"
        );
        assert!(!q.is_assignable(&empty(), &class));
    });
}

#[test]
fn optional_initializer_context_is_structural_and_never_grants_origin() {
    Engine::new().with_native_provider(definitions, |_, q| {
        let class = q.named_type_reference(key(2), &[]).unwrap();
        let context = q.class_definition_context(owner(2), &class).unwrap();
        assert!(q.is_assignable(&empty(), &context));
        assert!(!q.is_assignable(&context, &class));
        let read = q.property_type(&context, "value").unwrap();
        assert!(q.is_assignable(&Type::Nil, &read));
        assert!(q.is_assignable(&Type::Number, &read));
    });
}

#[test]
fn deferred_namespace_contract_requires_its_original_definition_and_keeps_actual_presence() {
    Engine::new().with_native_provider(definitions, |_, q| {
        let class = q.named_type_reference(key(2), &[]).unwrap();
        let definition = q.class_definition_value(owner(2), &class, &empty()).unwrap();
        let contract = q.class_definition_member_contract(owner(2), &definition).unwrap();
        assert_eq!(q.property_type(&contract, "value"), Some(Type::Number));
        let actual = q.property_type(&definition, "value").unwrap();
        assert!(q.is_assignable(&Type::Nil, &actual), "selecting a deferred contract does not populate the actual definition");
        assert!(q.class_definition_member_contract(owner(0), &definition).is_none(), "a reassigned namespace cannot borrow its previous owner's schema");
        assert!(q.class_definition_member_contract(owner(2), &class).is_none());
        assert!(q.class_definition_member_contract(owner(2), &empty()).is_none());
        let context = q.class_definition_context(owner(2), &class).unwrap();
        assert!(q.class_definition_member_contract(owner(2), &context).is_none(), "a structural initializer context has no definition capability");
    });
}

#[test]
fn deferred_namespace_contract_preserves_exact_generic_mapper_and_explicit_optional_members() {
    Engine::new().with_native_provider(|| {
        let mut p = definitions();
        p.base.declarations[0] = Decl::class(0, vec![Type::NativeTypeParameter(parameter())]);
        p.base.declarations[0].generic = true;
        p.base.declarations[2].optional = true;
        p
    }, |_, q| {
        for argument in [Type::Number, Type::String] {
            let class = q.named_type_reference(key(0), &[argument.clone()]).unwrap();
            let definition = q.class_definition_value(owner(0), &class, &empty()).unwrap();
            let contract = q.class_definition_member_contract(owner(0), &definition).unwrap();
            assert_eq!(q.property_type(&contract, "value"), Some(argument));
        }
        let class = q.named_type_reference(key(2), &[]).unwrap();
        let definition = q.class_definition_value(owner(2), &class, &empty()).unwrap();
        let contract = q.class_definition_member_contract(owner(2), &definition).unwrap();
        let optional = q.property_type(&contract, "value").unwrap();
        assert!(q.is_assignable(&Type::Number, &optional));
        assert!(q.is_assignable(&Type::Nil, &optional), "explicitly optional contracts are not strengthened");
    });
}

#[test]
fn missing_reads_include_nil_but_known_writes_always_require_original_t() {
    Engine::new().with_native_provider(definitions, |_, q| {
        let class = q.named_type_reference(key(2), &[]).unwrap();
        let definition = q
            .class_definition_value(owner(2), &class, &empty())
            .unwrap();
        let read = q.property_type(&definition, "value").unwrap();
        assert!(q.is_assignable(&Type::Nil, &read));
        assert!(q.is_assignable(&Type::Number, &read));
        assert!(!q.is_assignable(&Type::String, &read));
        let context = q.table_value_context(&definition, &value_key()).unwrap();
        assert_eq!(q.inspect_type(&context), Type::Number);
        assert!(!q.is_assignable(&Type::Nil, &context));
        assert!(
            q.table_write(&definition, &value_key(), &Type::Number)
                .is_ok()
        );
        assert!(
            q.table_write(&definition, &value_key(), &Type::String)
                .is_err()
        );
        assert!(
            q.table_write(&definition, &value_key(), &Type::Nil)
                .is_err()
        );
        assert!(
            q.table_write(
                &definition,
                &value_key(),
                &Type::Union(vec![Type::Number, Type::Nil])
            )
            .is_err()
        );
        assert!(q.format(&definition).starts_with("def "));
        assert_ne!(definition, class);
    });
}

#[test]
fn supplied_members_check_types_and_preserve_actual_extra_fields() {
    Engine::new().with_native_provider(definitions, |_, q| {
        let class = q.named_type_reference(key(2), &[]).unwrap();
        assert!(matches!(
            q.class_definition_value(owner(2), &class, &table(Type::String)),
            Err(NamedTypeError::DefinitionField { .. })
        ));
        let actual = Type::Table(TableType {
            fields: vec![
                TableField::new(LuaKey::String(b"value".to_vec()), Type::Number),
                TableField::new(LuaKey::String(b"extra".to_vec()), Type::String),
            ],
            fresh: true,
            ..TableType::default()
        });
        let definition = q.class_definition_value(owner(2), &class, &actual).unwrap();
        assert_eq!(q.property_type(&definition, "value"), Some(Type::Number));
        assert_eq!(q.property_type(&definition, "extra"), Some(Type::String));
        assert!(q.is_assignable(&definition, &class));
        assert!(
            q.table_write(
                &definition,
                &TableKey::Literal(LuaKey::String(b"extra".to_vec())),
                &Type::String
            )
            .is_ok()
        );
        assert!(
            q.table_write(
                &definition,
                &TableKey::Literal(LuaKey::String(b"extra".to_vec())),
                &Type::Number
            )
            .is_err()
        );
    });
}

#[test]
fn partial_definition_origin_does_not_erase_missing_required_fields_in_nested_relations() {
    Engine::new().with_native_provider(definitions, |_, q| {
        let class = q.named_type_reference(key(2), &[]).unwrap();
        let partial = q
            .class_definition_value(owner(2), &class, &empty())
            .unwrap();
        assert!(!q.is_assignable(&partial, &class));
        assert!(!q.is_assignable(&table(partial.clone()), &table(class.clone())));
        let a = q.array_type(&partial);
        let b = q.array_type(&class);
        assert!(!q.is_assignable(&a, &b));
        assert!(!q.is_assignable(
            &function(vec![], partial.clone()),
            &function(vec![], class.clone())
        ));
        assert!(!q.pack_assignable(&pack(vec![partial]), &pack(vec![class])));
        let empty_class = q.named_type_reference(key(0), &[]).unwrap();
        let empty_definition = q
            .class_definition_value(owner(0), &empty_class, &empty())
            .unwrap();
        assert!(q.is_assignable(&empty_definition, &empty_class));
    });
}

#[test]
fn same_owner_in_distinct_checked_contexts_has_distinct_canonical_views() {
    Engine::new().with_native_provider(definitions, |_, q| {
        let class = q.named_type_reference(key(0), &[]).unwrap();
        let number = q
            .class_definition_value(owner(0), &class, &table(Type::Number))
            .unwrap();
        let string = q
            .class_definition_value(owner(0), &class, &table(Type::String))
            .unwrap();
        assert_ne!(number, string);
        assert_eq!(q.property_type(&number, "value"), Some(Type::Number));
        assert_eq!(q.property_type(&string, "value"), Some(Type::String));
        let again = q
            .class_definition_value(owner(0), &class, &table(Type::Number))
            .unwrap();
        assert_eq!(number, again);
        let widened = q.widen(&number);
        assert!(q.format(&widened).starts_with("def "));
        assert_eq!(q.property_type(&widened, "value"), Some(Type::Number));
        assert!(matches!(q.inspect_type(&widened), Type::SessionType(_)));
    });
}

#[test]
fn original_member_t_is_checked_even_when_the_declared_field_is_optional() {
    Engine::new().with_native_provider(
        || {
            let mut p = basic();
            p.declarations[2].optional = true;
            Definitions { base: p, heads: vec![] }
        },
        |_, q| {
            let class = q.named_type_reference(key(2), &[]).unwrap();
            let definition = q
                .class_definition_value(owner(2), &class, &empty())
                .unwrap();
            assert!(
                q.table_write(&definition, &value_key(), &Type::Nil)
                    .is_err()
            );
        },
    );
}

#[test]
fn mapped_and_materialized_definitions_preserve_partial_role_and_origin() {
    with_program(|program, handles| {
        let mut class = Decl::class(0, vec![Type::NativeTypeParameter(parameter())]);
        class.generic = true;
        let provider = Definitions {
            base: Provider {
                declarations: vec![class],
                ..Provider::default()
            },
            heads: vec![],
        };
        let adapter = ProviderAdapter {
            provider: &provider,
            handles,
        };
        let mut checker = program.checker_with_native_provider(&adapter);
        checker.set_native_type_declaration_provider(&adapter);
        checker.enable_native_arrays();
        let class = checker
            .native_declared_head(native_named::core_declaration(key(0)))
            .unwrap();
        let actual = import_type(&mut checker, handles, &empty());
        let definition = checker
            .native_definition_value(
                bun_sema::types::NativeDefinitionId {
                    snapshot: 91001,
                    index: 0,
                },
                class,
                actual,
            )
            .unwrap();
        let param = checker
            .native_type_parameter(core_parameter_key(parameter()))
            .unwrap();
        let mapper = checker.mapper_from(&[param], &[TypeId::NUMBER]);
        let mapped = checker.instantiate(definition, mapper);
        let materialized = checker.native_object_resolved(mapped).unwrap();
        let class = checker.instantiate(class, mapper);
        assert!(!checker.is_assignable(mapped, class));
        assert!(!checker.is_assignable(materialized, class));
        let owned = export_type(&mut checker, handles, materialized);
        assert!(matches!(owned, Type::SessionType(_)));
        let mut q = SessionQueries {
            checker: &mut checker,
            handles,
        };
        assert!(q.format(&owned).starts_with("def "));
        assert!(q.table_write(&owned, &value_key(), &Type::Nil).is_err());
        assert!(q.table_write(&owned, &value_key(), &Type::Number).is_ok());
        let read = q.property_type(&owned, "value").unwrap();
        assert!(q.is_assignable(&Type::Nil, &read));
        assert!(q.is_assignable(&Type::Number, &read));
        assert!(!q.is_assignable(&Type::String, &read));
        let widened = q.widen(&owned);
        assert!(q.format(&widened).starts_with("def "));
        assert!(q.table_write(&widened, &value_key(), &Type::Nil).is_err());
    });
}

#[test]
fn dynamic_constructor_candidates_cannot_supply_wrong_fixed_member_types() {
    Engine::new().with_native_provider(definitions, |_, q| {
        let class = q.named_type_reference(key(2), &[]).unwrap();
        for fixed in [false, true] {
            let fields = if fixed { vec![TableField::new(LuaKey::String(b"value".to_vec()), Type::Number)] } else { vec![] };
            let actual = Type::Table(TableType { fields, indexers: vec![TableIndexer::new(Type::String, Type::String)], collision_candidates: true, ..TableType::default() });
            assert!(matches!(q.class_definition_value(owner(2), &class, &actual), Err(NamedTypeError::DefinitionField { .. })));
        }
        let valid = Type::Table(TableType { indexers: vec![TableIndexer::new(Type::String, Type::Union(vec![Type::Number, Type::Nil]))], collision_candidates: true, ..TableType::default() });
        assert!(q.class_definition_value(owner(2), &class, &valid).is_ok(), "constructor nil deletion is absence, not an invalid present numeric entry");
        let disjoint = Type::Table(TableType { indexers: vec![TableIndexer::new(Type::Number, Type::String)], collision_candidates: true, ..TableType::default() });
        assert!(q.class_definition_value(owner(2), &class, &disjoint).is_ok());
    });
}

#[test]
fn dynamic_domains_cover_union_generic_bounds_and_numeric_declared_keys() {
    Engine::new().with_native_provider(|| {
        let mut p=basic();p.constraint=Some(Type::String);p.declarations.push(Decl::class(7,vec![Type::Number]));Definitions{base:p,heads:vec![]}
    },|_,q| {
        let class=q.named_type_reference(key(2),&[]).unwrap();
        for domain in [Type::Union(vec![Type::StringLiteral(b"value".to_vec()),Type::StringLiteral(b"other".to_vec())]),Type::NativeTypeParameter(parameter())] {
            let actual=Type::Table(TableType{indexers:vec![TableIndexer::new(domain,Type::String)],collision_candidates:true,..TableType::default()});
            assert!(matches!(q.class_definition_value(owner(2),&class,&actual),Err(NamedTypeError::DefinitionField{..})));
        }
        let numeric=q.named_type_reference(key(7),&[]).unwrap();
        let bad=Type::Table(TableType{indexers:vec![TableIndexer::new(Type::Number,Type::String)],collision_candidates:true,..TableType::default()});
        assert!(matches!(q.class_definition_value(owner(7),&numeric,&bad),Err(NamedTypeError::DefinitionField{..})));
        let disjoint=Type::Table(TableType{indexers:vec![TableIndexer::new(Type::String,Type::String)],collision_candidates:true,..TableType::default()});
        assert!(q.class_definition_value(owner(7),&numeric,&disjoint).is_ok());
    });
}

#[test]
fn generic_dynamic_constructor_collision_preserves_present_and_deleted_candidates() {
    Engine::new().with_native_provider(|| {
        let mut p=basic();p.constraint=Some(Type::String);p.declarations[2].fields=vec![Type::Union(vec![Type::Number,Type::String])];Definitions{base:p,heads:vec![]}
    },|_,q| {
        let class=q.named_type_reference(key(2),&[]).unwrap();
        let actual=q.table_type(&TableType{fields:vec![TableField::new(LuaKey::String(b"value".to_vec()),Type::Number)],indexers:vec![TableIndexer{key:Type::NativeTypeParameter(parameter()),value:Type::String,sparse:true,readonly:false,origin:TableFieldOrigin::Inferred}],fresh:true,collision_candidates:true,..TableType::default()}).unwrap();
        let ordinary=q.property_type(&actual,"value").unwrap();
        assert!(q.is_assignable(&Type::Number,&ordinary));assert!(q.is_assignable(&Type::String,&ordinary));
        assert!(!q.is_assignable(&Type::Nil,&ordinary));
        let definition=q.class_definition_value(owner(2),&class,&actual).unwrap();
        let value=q.property_type(&definition,"value").unwrap();
        assert!(q.is_assignable(&Type::Number,&value));assert!(q.is_assignable(&Type::String,&value));
        assert!(!q.is_assignable(&value,&Type::Number));
        let deleted=q.table_type(&TableType{fields:vec![TableField::new(LuaKey::String(b"value".to_vec()),Type::Number)],indexers:vec![TableIndexer{key:Type::NativeTypeParameter(parameter()),value:Type::Nil,sparse:true,readonly:false,origin:TableFieldOrigin::Inferred}],fresh:true,collision_candidates:true,..TableType::default()}).unwrap();
        let ordinary=q.property_type(&deleted,"value").unwrap();
        assert!(q.is_assignable(&Type::Number,&ordinary));assert!(q.is_assignable(&Type::Nil,&ordinary));
        assert!(!q.is_assignable(&Type::String,&ordinary));
        let definition=q.class_definition_value(owner(2),&class,&deleted).unwrap();
        let value=q.property_type(&definition,"value").unwrap();
        assert!(q.is_assignable(&Type::Number,&value));assert!(q.is_assignable(&Type::Nil,&value));
    });
}

#[test]
fn dynamic_definition_writes_still_check_each_possible_declared_member() {
    Engine::new().with_native_provider(|| {
        let mut p=basic();p.constraint=Some(Type::String);Definitions{base:p,heads:vec![]}
    },|_,q| {
        let class=q.named_type_reference(key(2),&[]).unwrap();
        let actual=q.table_type(&TableType{indexers:vec![TableIndexer::new(Type::String,Type::Number)],fresh:true,collision_candidates:true,..TableType::default()}).unwrap();
        let definition=q.class_definition_value(owner(2),&class,&actual).unwrap();
        for domain in [Type::String,Type::NativeTypeParameter(parameter())] {
            let access=TableKey::Type(domain);
            assert!(q.table_write(&definition,&access,&Type::Nil).is_err());
            assert!(q.table_write(&definition,&access,&Type::String).is_err());
            let context=q.table_value_context(&definition,&access).unwrap();
            assert!(!q.is_assignable(&Type::Nil,&context));
            assert!(q.is_assignable(&Type::Number,&context));
        }
        assert!(q.table_write(&definition,&TableKey::Type(Type::String),&Type::Number).is_ok());
    });
}

#[test]
fn callable_definition_keeps_partial_members_and_returned_overloads_in_actual_view() {
    Engine::new().with_native_provider(|| {
        let mut p=definitions();
        for owner in [SignatureOwner::NamedType(key(2)),SignatureOwner::Function(runtime(0))] {
            p.heads.push(DefinitionHead{owner,input:Type::Number,returned:Type::String});
            p.heads.push(DefinitionHead{owner,input:Type::String,returned:Type::Number});
        }
        p
    },|_,q| {
        let class=q.named_type_reference(key(2),&[]).unwrap();
        let closure=q.call(&Type::NativeFunction(runtime(1)),&[]);
        assert!(closure.errors.is_empty());
        let actual=q.table_type(&TableType{fields:vec![TableField::new(LuaKey::String(b"helper".to_vec()),closure.return_type)],..TableType::default()}).unwrap();
        let context=q.class_definition_context(owner(2),&class).unwrap();assert!(!q.is_assignable(&actual,&context),"ordinary weak-object assignment retains its TS common-property rule");
        let definition=q.class_definition_value(owner(2),&class,&actual).unwrap();
        assert_eq!(q.call_signature_types(&definition).len(),2);
        assert!(q.function_implementation_type(&definition).is_none());
        for (input,returned) in [(Type::Number,Type::String),(Type::String,Type::Number)] {
            let call=q.call(&definition,&[input.clone()]);assert!(call.errors.is_empty(),"{:?}",call.errors);assert_eq!(call.return_type,returned);
            let helper=q.property_type(&definition,"helper").unwrap();
            let call=q.call(&helper,&[input]);assert!(call.errors.is_empty(),"{:?}",call.errors);assert_eq!(call.return_type,returned);
        }
        assert!(!q.call(&definition,&[Type::Boolean]).errors.is_empty());
        assert!(q.is_assignable(&definition,&function(vec![Type::Number],Type::String)));
        assert!(!q.is_assignable(&definition,&function(vec![Type::Boolean],Type::String)));
        assert!(!q.is_assignable(&definition,&class));
        let read=q.property_type(&definition,"value").unwrap();assert!(q.is_assignable(&Type::Nil,&read));
        assert!(q.table_write(&definition,&value_key(),&Type::Nil).is_err());
        let plain=q.named_type_reference(key(0),&[]).unwrap();let plain=q.class_definition_value(owner(0),&plain,&empty()).unwrap();
        assert!(!q.call(&plain,&[Type::Number]).errors.is_empty(),"another class's overloads cannot make this definition callable");
    });
}

#[test]
fn mapped_and_materialized_definition_calls_keep_class_mapper_and_partial_view() {
    with_program(|program,handles| {
        let mut p=definitions();p.base.declarations[0].generic=true;p.base.declarations[0].fields=vec![Type::NativeTypeParameter(parameter())];
        p.heads.push(DefinitionHead{owner:SignatureOwner::NamedType(key(0)),input:Type::NativeTypeParameter(parameter()),returned:Type::NativeTypeParameter(parameter())});
        let adapter=ProviderAdapter{provider:&p,handles};let mut checker=program.checker_with_native_provider(&adapter);checker.set_native_type_declaration_provider(&adapter);checker.enable_native_arrays();
        let class=checker.native_declared_head(native_named::core_declaration(key(0))).unwrap();
        let actual=import_type(&mut checker,handles,&empty());
        let definition=checker.native_definition_value(bun_sema::types::NativeDefinitionId{snapshot:91001,index:0},class,actual).unwrap();
        let formal=import_type(&mut checker,handles,&Type::NativeTypeParameter(parameter()));let mapper=checker.mapper_from(&[formal],&[TypeId::NUMBER]);
        let mapped=checker.instantiate(definition,mapper);let materialized=checker.native_object_resolved(mapped).unwrap();
        let raw_materialized=checker.native_object_resolved(definition).unwrap();let mapped_table=checker.instantiate(raw_materialized,mapper);
        let mapped=export_type(&mut checker,handles,mapped);let materialized=export_type(&mut checker,handles,materialized);let mapped_table=export_type(&mut checker,handles,mapped_table);
        let mut q=SessionQueries{checker:&mut checker,handles};
        for carrier in [mapped,materialized,mapped_table] {
            assert!(matches!(carrier,Type::SessionType(_)));assert!(q.format(&carrier).starts_with("def "));
            assert!(q.is_assignable(&carrier,&Type::RuntimeTable));assert!(!q.is_assignable(&carrier,&Type::RuntimeFunction));
            assert_eq!(q.call_signature_types(&carrier).len(),1);
            let call=q.call(&carrier,&[Type::Number]);assert!(call.errors.is_empty(),"{:?}",call.errors);assert_eq!(call.return_type,Type::Number);
            assert!(!q.call(&carrier,&[Type::String]).errors.is_empty());
            let read=q.property_type(&carrier,"value").unwrap();assert!(q.is_assignable(&Type::Nil,&read));assert!(q.is_assignable(&Type::Number,&read));assert!(!q.is_assignable(&Type::String,&read));
            assert!(q.table_write(&carrier,&value_key(),&Type::Nil).is_err());
            let class=q.named_type_reference(key(0),&[Type::Number]).unwrap();assert!(!q.is_assignable(&carrier,&class));
            let widened=q.widen(&carrier);let call=q.call(&widened,&[Type::Number]);assert!(call.errors.is_empty());assert_eq!(call.return_type,Type::Number);
        }
    });
}

#[test]
fn materialized_definition_relations_compare_overload_contracts_after_actual_fields() {
    with_program(|program,handles| {
        let mut p=definitions();p.base.declarations[0].generic=true;
        p.heads.push(DefinitionHead{owner:SignatureOwner::NamedType(key(0)),input:Type::NativeTypeParameter(parameter()),returned:Type::NativeTypeParameter(parameter())});
        let adapter=ProviderAdapter{provider:&p,handles};let mut checker=program.checker_with_native_provider(&adapter);checker.set_native_type_declaration_provider(&adapter);checker.enable_native_arrays();
        let mut carriers=Vec::new();
        for argument in [TypeId::NUMBER,TypeId::STRING] {
            let class=checker.native_type_reference(native_named::core_declaration(key(0)),&[argument]).unwrap();
            let actual=import_type(&mut checker,handles,&empty());let definition=checker.native_definition_value(bun_sema::types::NativeDefinitionId{snapshot:91001,index:0},class,actual).unwrap();
            assert!(checker.is_assignable(definition,class),"complete empty same-origin schema remains a valid instance");
            let table=checker.native_object_resolved(definition).unwrap();carriers.push((definition,table));
        }
        for source in [carriers[0].0,carriers[0].1] {for target in [carriers[1].0,carriers[1].1] {
            assert!(!checker.is_assignable(source,target),"same-origin empty fields must not erase different mapped call contracts");
            assert!(!checker.is_assignable(target,source));
        }}
    });
}

#[test]
fn runtime_table_relations_and_generic_inference_keep_definition_carrier() {
    Engine::new().with_native_provider(|| {
        let mut p=definitions();p.base.constraint=Some(Type::RuntimeTable);p.base.declarations[0].generic=true;
        p.heads.push(DefinitionHead{owner:SignatureOwner::NamedType(key(2)),input:Type::Number,returned:Type::String});p
    },|_,q| {
        let class=q.named_type_reference(key(2),&[]).unwrap();let definition=q.class_definition_value(owner(2),&class,&empty()).unwrap();
        assert!(q.is_assignable(&definition,&Type::RuntimeTable));assert!(!q.is_assignable(&definition,&Type::RuntimeFunction));
        assert!(q.is_assignable(&definition,&Type::Union(vec![Type::RuntimeTable,Type::Nil])));
        assert!(q.is_assignable(&class,&Type::Union(vec![Type::RuntimeTable,Type::Nil])),"the nullable shortcut also preserves ordinary named callable tables");
        assert!(!q.is_assignable(&definition,&Type::Union(vec![Type::RuntimeFunction,Type::Nil])));
        assert!(!q.is_assignable(&Type::Union(vec![definition.clone(),function(vec![],Type::Number)]),&Type::RuntimeTable));
        assert!(!q.is_assignable(&Type::Union(vec![definition.clone(),Type::Nil]),&Type::RuntimeTable));
        assert!(!q.is_assignable(&Type::RuntimeTable,&class),"the runtime category grants no class origin");
        let identity=q.pack_function(&PackFunctionType{params:vec![Parameter{name:"value".into(),ty:Type::NativeTypeParameter(parameter()),optional:false}],rest:None,type_parameters:vec![parameter()],returns:ValuePack::single(Type::NativeTypeParameter(parameter()))});
        let called=q.call(&identity,&[definition.clone()]);assert!(called.errors.is_empty(),"{:?}",called.errors);assert_eq!(called.return_type,definition,"T:table retains the argument's exact nominal partial carrier");
        assert!(q.table_write(&called.return_type,&value_key(),&Type::Nil).is_err());assert!(!q.is_assignable(&called.return_type,&class));
        assert!(!q.call(&identity,&[function(vec![],Type::Number)]).errors.is_empty());
        let complete=q.class_definition_value(owner(2),&class,&table(Type::Number)).unwrap();assert!(q.is_assignable(&complete,&class));
        let call=q.call(&complete,&[Type::Number]);assert!(call.errors.is_empty());assert_eq!(call.return_type,Type::String);
    });
}

struct DefinitionRuntimeGraph { context:u64, value:Type, kind:NativeRuntimeType }
impl DefinitionRuntimeGraph {
    fn graph(&self)->NativeGraphKey {NativeGraphKey{snapshot:91002,region:0,context:self.context}}
    fn at(&self,node:u32)->NativeFlowKey {NativeFlowKey{graph:self.graph(),node}}
    fn reference(&self)->NativeReferenceKey {NativeReferenceKey{graph:self.graph(),reference:0}}
}
impl NativeFlowProvider for DefinitionRuntimeGraph {
    fn contains_graph(&self,graph:NativeGraphKey)->bool {graph==self.graph()}
    fn contains_reference(&self,reference:NativeReferenceKey)->bool {reference==self.reference()}
    fn node(&self,at:NativeFlowKey)->Option<NativeFlowNode> {
        if at.graph!=self.graph(){return None}Some(match at.node {
            0=>NativeFlowNode::Start,
            1|2=>NativeFlowNode::Predicate{before:self.at(0),predicate:NativePredicate::RuntimeType{subject:self.reference(),value:NativeValueKey{snapshot:91002,index:0},input:self.at(0),kind:self.kind.clone(),equals:true},sense:at.node==1},
            _=>return None,
        })
    }
    fn is_shared(&self,_:NativeFlowKey)->bool {false}
    fn write_affects(&self,reference:NativeReferenceKey,target:NativeReferenceKey,_:NativeFlowKey)->NativeWriteRelation {if reference==target{NativeWriteRelation::Exact}else{NativeWriteRelation::Unaffected}}
    fn value_type(&self,_:NativeValueKey,_:NativeFlowKey,_:&mut dyn TypeQueries)->Type {self.value.clone()}
    fn call_effects(&self,_:NativeValueKey,_:NativeFlowKey,_:&mut dyn TypeQueries)->NativeCallEffects {NativeCallEffects::None}
    fn flow_error(&self,_:NativeFlowKey,error:NativeFlowError){panic!("unexpected definition runtime flow error: {error:?}")}
}

#[test]
fn runtime_predicates_preserve_callable_definition_identity_and_partial_write_contract() {
    Engine::new().with_native_provider(|| {
        let mut p=definitions();p.heads.push(DefinitionHead{owner:SignatureOwner::NamedType(key(2)),input:Type::Number,returned:Type::String});p
    },|_,q| {
        let class=q.named_type_reference(key(2),&[]).unwrap();let definition=q.class_definition_value(owner(2),&class,&empty()).unwrap();
        let value=Type::Union(vec![definition.clone(),Type::RuntimeFunction]);
        for (context,kind,table_branch) in [(1,NativeRuntimeType::Table,1),(2,NativeRuntimeType::Function,2)] {
            let graph=DefinitionRuntimeGraph{context,value:value.clone(),kind};
            let narrowed=q.flow_type(&graph,graph.at(table_branch),graph.reference(),&value,&value);assert!(!narrowed.incomplete);assert_eq!(narrowed.ty,definition);
            let other=q.flow_type(&graph,graph.at(3-table_branch),graph.reference(),&value,&value);assert_eq!(other.ty,Type::RuntimeFunction);
            assert!(q.table_write(&narrowed.ty,&value_key(),&Type::Nil).is_err());assert!(!q.is_assignable(&narrowed.ty,&class));
            let read=q.property_type(&narrowed.ty,"value").unwrap();assert!(q.is_assignable(&Type::Number,&read));assert!(q.is_assignable(&Type::Nil,&read));
            let called=q.call(&narrowed.ty,&[Type::Number]);assert!(called.errors.is_empty());assert_eq!(called.return_type,Type::String);
        }
    });
}
