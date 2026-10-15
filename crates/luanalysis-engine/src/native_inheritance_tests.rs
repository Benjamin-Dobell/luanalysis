use crate::*;
use std::cell::{Cell,RefCell};

#[path = "native_self_tests.rs"]
mod native_self;

fn key(index:u32)->NativeTypeDeclarationKey {NativeTypeDeclarationKey{snapshot:92001,index}}
fn param(index:u32)->NativeTypeParameterKey {NativeTypeParameterKey{snapshot:92001,index}}
fn reference(index:u32,args:Vec<Type>)->Type {Type::NativeNamedReference{declaration:key(index),arguments:args}}
fn owner(index:u32)->NativeClassDefinitionKey {NativeClassDefinitionKey{snapshot:92001,index}}
fn signature(index:u32)->NativeSignatureKey {NativeSignatureKey{snapshot:92001,index}}
fn array(value:Type)->Type {Type::Table(TableType{indexers:vec![TableIndexer::new(Type::Number,value)],..Default::default()})}
struct Head {kind:TypeDeclarationKind,generic:bool,fields:Vec<Type>,bases:Vec<Type>,optional:bool,indexer:Option<Type>,inferred:bool}
impl Head {
    fn class(fields:Vec<Type>)->Self {Self{kind:TypeDeclarationKind::Class,generic:false,fields,bases:vec![],optional:false,indexer:None,inferred:false}}
}
struct Provider {heads:Vec<Head>,calls:bool,bases:Cell<usize>,reads:Cell<usize>,errors:RefCell<Vec<NamedTypeError>>}
impl Provider {fn new(heads:Vec<Head>)->Self {Self{heads,calls:false,bases:Cell::new(0),reads:Cell::new(0),errors:RefCell::new(vec![])}}}
impl NativeTypeProvider for Provider {
    fn contains_type_declaration(&self,id:NativeTypeDeclarationKey)->bool {id.snapshot==92001&&(id.index as usize)<self.heads.len()}
    fn type_declaration_header(&self,id:NativeTypeDeclarationKey)->TypeDeclarationHeader {let h=&self.heads[id.index as usize];TypeDeclarationHeader{kind:h.kind,name:format!("Head{}",id.index),object_form:Some(ObjectDeclarationForm::Record),parameters:if h.generic{vec![param(id.index)]}else{vec![]},outer_parameters:vec![]}}
    fn type_declaration_bases(&self,id:NativeTypeDeclarationKey,_:&mut dyn TypeQueries)->Vec<Type> {self.bases.set(self.bases.get()+1);self.heads[id.index as usize].bases.clone()}
    fn type_declaration_members(&self,id:NativeTypeDeclarationKey)->Vec<NativeTypeMemberKey> {(0..self.heads[id.index as usize].fields.len()).map(|n|NativeTypeMemberKey{snapshot:92001,index:id.index*100+n as u32}).collect()}
    fn contains_type_member(&self,id:NativeTypeMemberKey)->bool {id.snapshot==92001&&((id.index/100)as usize)<self.heads.len()&&((id.index%100)as usize)<self.heads[(id.index/100)as usize].fields.len()}
    fn type_member_header(&self,id:NativeTypeMemberKey)->TypeMemberHeader {TypeMemberHeader{owner:TypeMemberOwner::Declared(key(id.index/100)),key:if self.heads[(id.index/100)as usize].indexer.is_some(){TypeMemberKeyRecipe::Indexer{sparse:false}}else{TypeMemberKeyRecipe::Fixed(LuaKey::String(if id.index%100==0{b"value".to_vec()}else{b"other".to_vec()}))},visibility:TypeMemberVisibility::Public,optional:self.heads[(id.index/100)as usize].optional,readonly:false}}
    fn type_indexer_key(&self,id:NativeTypeMemberKey,_:&mut dyn TypeQueries)->Type {self.heads[(id.index/100)as usize].indexer.clone().unwrap()}
    fn type_member_origin(&self,id:NativeTypeMemberKey)->TableFieldOrigin {if self.heads[(id.index/100)as usize].inferred{TableFieldOrigin::Inferred}else{TableFieldOrigin::Declared}}
    fn type_member_type(&self,id:NativeTypeMemberKey,_:&mut dyn TypeQueries)->Type {self.reads.set(self.reads.get()+1);self.heads[(id.index/100)as usize].fields[(id.index%100)as usize].clone()}
    fn contains_type_parameter(&self,id:NativeTypeParameterKey)->bool {id.snapshot==92001&&(id.index as usize)<self.heads.len()}
    fn type_parameter_name(&self,id:NativeTypeParameterKey)->String {format!("T{}",id.index)}
    fn type_parameter_owner(&self,id:NativeTypeParameterKey)->Option<TypeDeclarationOwner> {Some(TypeDeclarationOwner::NamedType(key(id.index)))}
    fn contains_class_definition(&self,id:NativeClassDefinitionKey)->bool {id.snapshot==92001&&(id.index as usize)<self.heads.len()&&self.heads[id.index as usize].kind==TypeDeclarationKind::Class}
    fn class_definition_header(&self,id:NativeClassDefinitionKey)->ClassDefinitionHeader {ClassDefinitionHeader{declaration:key(id.index)}}
    fn type_declaration_call_signatures(&self,id:NativeTypeDeclarationKey)->Vec<NativeSignatureKey> {if self.calls&&id.index<2{vec![signature(id.index)]}else{vec![]}}
    fn contains_signature(&self,id:NativeSignatureKey)->bool {self.calls&&id.snapshot==92001&&id.index<2}
    fn signature_header(&self,id:NativeSignatureKey)->SignatureHeader {SignatureHeader{owner:SignatureOwner::NamedType(key(id.index)),site:SignatureSite{snapshot:92001,index:id.index},parent:None,has_direct_literal_parameter:false}}
    fn signature_outer_type_parameters(&self,id:NativeSignatureKey)->Vec<NativeTypeParameterKey> {if id.index==0{vec![param(0)]}else{vec![]}}
    fn signature_parameters(&self,id:NativeSignatureKey,_:&mut dyn TypeQueries)->Vec<Parameter> {vec![Parameter{name:"arg".into(),ty:if id.index==0{Type::NativeTypeParameter(param(0))}else{Type::Boolean},optional:false}]}
    fn signature_declared_return_pack(&self,id:NativeSignatureKey,_:&mut dyn TypeQueries)->ValuePack {ValuePack::single(if id.index==0{Type::NativeTypeParameter(param(0))}else{Type::Boolean})}
    fn type_declaration_error(&self,error:NamedTypeError){self.errors.borrow_mut().push(error)}
    fn contains_function(&self,_:NativeFunctionKey)->bool {false}
    fn function_parameters(&self,_:NativeFunctionKey,_:&mut dyn TypeQueries)->Vec<Parameter>{unreachable!()}
    fn function_declared_return(&self,_:NativeFunctionKey,_:&mut dyn TypeQueries)->Option<Type>{unreachable!()}
    fn function_inferred_return(&self,_:NativeFunctionKey,_:&mut dyn TypeQueries)->Type{unreachable!()}
    fn return_cycle(&self,_:NativeFunctionKey){unreachable!()}
    fn return_type_too_deep(&self,_:NativeFunctionKey){unreachable!()}
}

#[test]
fn mapped_ancestors_preserve_lazy_member_owner_and_nested_arguments() {
    Engine::new().with_native_provider(|| {
        let mut base=Head::class(vec![Type::NativeTypeParameter(param(0)),Type::String]);base.generic=true;
        let mut derived=Head::class(vec![]);derived.generic=true;derived.bases=vec![reference(0,vec![array(Type::NativeTypeParameter(param(1)))])];
        Provider::new(vec![base,derived,Head::class(vec![])])
    },|p,q| {
        let derived=q.named_type_reference(key(1),&[Type::Number]).unwrap();let value=q.property_type(&derived,"value").unwrap();
        assert_eq!(p.reads.get(),1,"an inherited literal read does not resolve sibling member recipes");
        assert!(q.is_assignable(&value,&array(Type::Number)));assert!(!q.is_assignable(&value,&array(Type::String)));
        let base=q.named_type_reference(key(0),&[array(Type::Number)]).unwrap();assert!(q.is_assignable(&derived,&base));
        let wrong=q.named_type_reference(key(0),&[array(Type::String)]).unwrap();assert!(!q.is_assignable(&derived,&wrong));
        let unrelated=q.named_type_reference(key(2),&[]).unwrap();assert!(!q.is_assignable(&derived,&unrelated));
        let count=p.bases.get();assert_eq!(q.property_type(&derived,"other"),Some(Type::String));assert_eq!(p.bases.get(),count,"base recipes are cached in original declaration space");
    });
}

#[test]
fn inherited_member_context_preserves_owner_mapper_and_caches_absence() {
    Engine::new().with_native_provider(|| {
        let mut base = Head::class(vec![Type::NativeTypeParameter(param(0)), Type::String]);
        base.generic = true;
        let mut derived = Head::class(vec![Type::Unknown]);
        derived.generic = true;
        derived.bases = vec![reference(0, vec![array(Type::NativeTypeParameter(param(1)))])];
        Provider::new(vec![base, derived])
    }, |provider, queries| {
        let member = NativeTypeMemberKey { snapshot: 92001, index: 100 };
        let expected = array(Type::NativeTypeParameter(param(1)));
        let context = queries.named_member_context(member).unwrap().unwrap();
        assert!(queries.is_assignable(&context, &expected));
        assert!(queries.is_assignable(&expected, &context));
        assert_eq!(provider.reads.get(), 1, "only the matching inherited member is demanded");
        let reads = provider.reads.get();
        assert_eq!(queries.named_member_context(member).unwrap(), Some(context));
        assert_eq!(provider.reads.get(), reads, "completed context is cached");
        let base_member = NativeTypeMemberKey { snapshot: 92001, index: 0 };
        assert_eq!(queries.named_member_context(base_member).unwrap(), None);
        assert_eq!(queries.named_member_context(base_member).unwrap(), None);
        assert_eq!(provider.reads.get(), reads, "absence does not demand the member's own initializer");
        let foreign = NativeTypeMemberKey { snapshot: 92002, index: 100 };
        assert!(matches!(queries.named_member_context(foreign), Err(NamedTypeError::InvalidMember(key)) if key == foreign));
        assert_eq!(provider.reads.get(), reads);
    });
}

#[test]
fn definition_context_uses_inherited_contract_before_an_invalid_inferred_override() {
    Engine::new().with_native_provider(|| {
        let base = Head::class(vec![Type::Number]);
        let mut derived = Head::class(vec![Type::String]);
        derived.inferred = true;
        derived.bases = vec![reference(0, vec![])];
        Provider::new(vec![base, derived])
    }, |provider, queries| {
        let derived = queries.named_declared_type(key(1)).unwrap();
        let context = queries.class_definition_context(owner(1), &derived).unwrap();
        assert_eq!(provider.reads.get(), 1, "the initializer context does not demand its own inferred member");
        let value = queries.property_type(&context, "value").unwrap();
        assert!(queries.is_assignable(&Type::Number, &value));
        assert!(queries.is_assignable(&Type::Nil, &value));
        assert!(!queries.is_assignable(&Type::String, &value));
        assert!(matches!(queries.validate_named_declaration(key(1)), Err(NamedTypeError::IncompatibleOverride { .. })));
        let reads = provider.reads.get();
        assert_eq!(queries.class_definition_context(owner(1), &derived).unwrap(), context);
        assert_eq!(provider.reads.get(), reads, "a later full-schema error does not invalidate the completed initializer context");
    });
}

#[test]
fn shape_edges_do_not_confer_class_ancestry() {
    Engine::new().with_native_provider(|| {
        let base=Head::class(vec![Type::Number]);let mut shape=Head::class(vec![]);shape.kind=TypeDeclarationKind::Shape;shape.bases=vec![reference(0,vec![])];
        let mut derived=Head::class(vec![]);derived.bases=vec![reference(1,vec![])];Provider::new(vec![base,shape,derived])
    },|_,q| {
        let derived=q.named_type_reference(key(2),&[]).unwrap();let base=q.named_type_reference(key(0),&[]).unwrap();let shape=q.named_type_reference(key(1),&[]).unwrap();
        assert_eq!(q.property_type(&derived,"value"),Some(Type::Number));assert!(q.is_assignable(&derived,&shape));assert!(!q.is_assignable(&derived,&base));
    });
}

#[test]
fn valid_middle_override_remains_effective_in_grandchild_and_invalid_own_override_reports_origin() {
    Engine::new().with_native_provider(|| {
        let base=Head::class(vec![Type::Union(vec![Type::Number,Type::String])]);let mut middle=Head::class(vec![Type::String]);middle.bases=vec![reference(0,vec![])];let mut leaf=Head::class(vec![]);leaf.bases=vec![reference(1,vec![])];let mut bad=Head::class(vec![Type::Boolean]);bad.bases=vec![reference(0,vec![])];Provider::new(vec![base,middle,leaf,bad])
    },|_,q| {
        assert!(q.validate_named_declaration(key(2)).is_ok());let leaf=q.named_type_reference(key(2),&[]).unwrap();assert_eq!(q.property_type(&leaf,"value"),Some(Type::String));
        assert!(matches!(q.validate_named_declaration(key(3)),Err(NamedTypeError::IncompatibleOverride{member:NativeTypeMemberKey{index:300,..},..})));
    });
}

#[test]
fn productive_recursive_members_survive_while_growing_base_cycles_are_bounded() {
    Engine::new().with_native_provider(|| {
        let mut node=Head::class(vec![Type::Union(vec![reference(0,vec![]),Type::Nil])]);node.bases=vec![reference(1,vec![])];let base=Head::class(vec![]);
        let mut cyclic=Head::class(vec![]);cyclic.generic=true;cyclic.bases=vec![reference(2,vec![array(Type::NativeTypeParameter(param(2)))])];Provider::new(vec![node,base,cyclic])
    },|_,q| {
        assert!(q.validate_named_declaration(key(0)).is_ok());assert!(matches!(q.validate_named_declaration(key(2)),Err(NamedTypeError::CircularBase(_))));
    });
}

#[test]
fn inherited_overloads_and_definition_partial_views_keep_mappers_and_origins() {
    Engine::new().with_native_provider(|| {
        let mut base=Head::class(vec![Type::NativeTypeParameter(param(0))]);base.generic=true;let mut child=Head::class(vec![]);child.generic=true;child.bases=vec![reference(0,vec![Type::NativeTypeParameter(param(1))])];let mut p=Provider::new(vec![base,child]);p.calls=true;p
    },|_,q| {
        let child=q.named_type_reference(key(1),&[Type::Number]).unwrap();let base=q.named_type_reference(key(0),&[Type::Number]).unwrap();
        let definition=q.class_definition_value(owner(1),&child,&Type::Table(TableType::default())).unwrap();assert_eq!(q.call_signature_types(&definition).len(),2);
        for value in [Type::Number,Type::Boolean] {let call=q.call(&definition,&[value.clone()]);assert!(call.errors.is_empty(),"{:?}",call.errors);assert_eq!(call.return_type,value);}
        let read=q.property_type(&definition,"value").unwrap();assert!(q.is_assignable(&Type::Nil,&read));assert!(q.is_assignable(&Type::Number,&read));
        let access=TableKey::Literal(LuaKey::String(b"value".to_vec()));assert!(q.table_write(&definition,&access,&Type::Nil).is_err());assert!(q.table_write(&definition,&access,&Type::String).is_err());assert!(!q.is_assignable(&definition,&base));
        let actual=Type::Table(TableType{fields:vec![TableField::new(LuaKey::String(b"value".to_vec()),Type::Number)],..Default::default()});let complete=q.class_definition_value(owner(1),&child,&actual).unwrap();assert!(q.is_assignable(&complete,&base));
    });
}

#[test]
fn materialized_class_and_definition_tables_keep_inherited_call_heads_and_member_relations() {
    with_program(|program,handles| {
        let mut base=Head::class(vec![Type::NativeTypeParameter(param(0))]);base.generic=true;let mut child=Head::class(vec![]);child.bases=vec![reference(0,vec![Type::Number])];let mut p=Provider::new(vec![base,child]);p.calls=true;
        let adapter=ProviderAdapter{provider:&p,handles};let mut checker=program.checker_with_native_provider(&adapter);checker.set_native_type_declaration_provider(&adapter);checker.enable_native_arrays();
        let class=checker.native_type_reference(native_named::core_declaration(key(1)),&[]).unwrap();let base=checker.native_type_reference(native_named::core_declaration(key(0)),&[TypeId::NUMBER]).unwrap();let table=checker.native_object_resolved(class).unwrap();assert!(checker.is_assignable(table,base));
        let actual=import_type(&mut checker,handles,&Type::Table(TableType::default()));let definition=checker.native_definition_value(bun_sema::types::NativeDefinitionId{snapshot:92001,index:1},class,actual).unwrap();let partial=checker.native_object_resolved(definition).unwrap();assert!(!checker.is_assignable(partial,base));
        let class=export_type(&mut checker,handles,class);let table=export_type(&mut checker,handles,table);let partial=export_type(&mut checker,handles,partial);let mut q=SessionQueries{checker:&mut checker,handles};
        for carrier in [class,table,partial.clone()] {assert_eq!(q.call_signature_types(&carrier).len(),2);for value in [Type::Number,Type::Boolean] {let call=q.call(&carrier,&[value.clone()]);assert!(call.errors.is_empty(),"{:?}",call.errors);assert_eq!(call.return_type,value);}}
        let read=q.property_type(&partial,"value").unwrap();assert!(q.is_assignable(&Type::Nil,&read));assert!(q.table_write(&partial,&TableKey::Literal(LuaKey::String(b"value".to_vec())),&Type::Nil).is_err());
    });
}

#[test]
fn inherited_index_domains_detect_cross_base_fields_and_partial_union_overlap() {
    Engine::new().with_native_provider(|| {
        let field=Head::class(vec![Type::Number]);let mut strings=Head::class(vec![Type::String]);strings.indexer=Some(Type::String);
        let mut conflict=Head::class(vec![]);conflict.bases=vec![reference(0,vec![]),reference(1,vec![])];
        let mut a=Head::class(vec![Type::Number]);a.indexer=Some(Type::Union(vec![Type::StringLiteral(b"a".to_vec()),Type::StringLiteral(b"b".to_vec())]));
        let mut b=Head::class(vec![Type::String]);b.indexer=Some(Type::Union(vec![Type::StringLiteral(b"b".to_vec()),Type::StringLiteral(b"c".to_vec())]));
        let mut overlap=Head::class(vec![]);overlap.bases=vec![reference(3,vec![]),reference(4,vec![])];
        let mut broad=Head::class(vec![Type::Union(vec![Type::Number,Type::String])]);broad.indexer=Some(Type::String);broad.bases=vec![reference(0,vec![])];
        Provider::new(vec![field,strings,conflict,a,b,overlap,broad])
    },|_,q| {
        assert!(matches!(q.validate_named_declaration(key(2)),Err(NamedTypeError::IncompatibleBase{..})));
        assert!(matches!(q.validate_named_declaration(key(5)),Err(NamedTypeError::IncompatibleBase{..})));
        assert!(q.validate_named_declaration(key(6)).is_ok(),"a broad indexer permits the inherited narrower fixed field");
    });
}

#[test]
fn grandchild_keeps_valid_indexer_override_layers_and_nonoverlapping_domain_tails() {
    Engine::new().with_native_provider(|| {
        let values=Type::Union(vec![Type::Number,Type::String]);
        let mut base=Head::class(vec![values.clone()]);base.indexer=Some(Type::String);
        let mut middle=Head::class(vec![Type::String]);middle.indexer=Some(Type::String);middle.bases=vec![reference(0,vec![])];
        let mut leaf=Head::class(vec![]);leaf.bases=vec![reference(1,vec![])];
        let domain=|a:&[u8],b:&[u8]|Type::Union(vec![Type::StringLiteral(a.to_vec()),Type::StringLiteral(b.to_vec())]);
        let mut partial_base=Head::class(vec![values]);partial_base.indexer=Some(domain(b"b",b"c"));
        let mut partial_middle=Head::class(vec![Type::String]);partial_middle.indexer=Some(domain(b"a",b"b"));partial_middle.bases=vec![reference(3,vec![])];
        let mut partial_leaf=Head::class(vec![]);partial_leaf.bases=vec![reference(4,vec![])];
        let mut invalid=Head::class(vec![Type::Boolean]);invalid.indexer=Some(domain(b"a",b"b"));invalid.bases=vec![reference(3,vec![])];
        let mut invalid_leaf=Head::class(vec![]);invalid_leaf.bases=vec![reference(6,vec![])];
        Provider::new(vec![base,middle,leaf,partial_base,partial_middle,partial_leaf,invalid,invalid_leaf])
    },|_,q| {
        let lookup=|q:&mut dyn TypeQueries,value:&Type,key:&[u8]|q.table_lookup(value,&TableKey::Literal(LuaKey::String(key.to_vec()))).unwrap();
        assert!(q.validate_named_declaration(key(2)).is_ok());let leaf=q.named_type_reference(key(2),&[]).unwrap();assert_eq!(lookup(q,&leaf,b"any"),Type::String);
        assert!(q.validate_named_declaration(key(5)).is_ok());let leaf=q.named_type_reference(key(5),&[]).unwrap();
        for name in [b"a",b"b"] {assert_eq!(lookup(q,&leaf,name),Type::String);}
        let tail=lookup(q,&leaf,b"c");assert!(q.is_assignable(&Type::Number,&tail));assert!(q.is_assignable(&Type::String,&tail));assert!(!q.is_assignable(&Type::Boolean,&tail));
        assert!(matches!(q.validate_named_declaration(key(7)),Err(NamedTypeError::IncompatibleOverride{member:NativeTypeMemberKey{index:600,..},..})),"an invalid parent cannot be accepted as an already validated base");
    });
}
