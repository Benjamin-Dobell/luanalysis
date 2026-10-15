use super::*;

fn self_key(index:u32)->NativeTypeParameterKey {param(1000+index)}
fn fluent(index:u32,receiver:bool)->Type {Type::Function(FunctionType{params:if receiver{vec![Parameter{name:"self".into(),ty:Type::NativeTypeParameter(self_key(index)),optional:false}]}else{vec![]},returns:Box::new(Type::NativeTypeParameter(self_key(index)))})}
struct SelfProvider {base:Provider}
impl NativeTypeProvider for SelfProvider {
    fn contains_type_declaration(&self,id:NativeTypeDeclarationKey)->bool {self.base.contains_type_declaration(id)}
    fn type_declaration_header(&self,id:NativeTypeDeclarationKey)->TypeDeclarationHeader {self.base.type_declaration_header(id)}
    fn type_declaration_self_parameter(&self,id:NativeTypeDeclarationKey)->Option<NativeTypeParameterKey> {(self.contains_type_declaration(id)&&self.base.heads[id.index as usize].kind==TypeDeclarationKind::Class).then(||self_key(id.index))}
    fn type_declaration_bases(&self,id:NativeTypeDeclarationKey,q:&mut dyn TypeQueries)->Vec<Type> {self.base.type_declaration_bases(id,q)}
    fn type_declaration_members(&self,id:NativeTypeDeclarationKey)->Vec<NativeTypeMemberKey> {self.base.type_declaration_members(id)}
    fn contains_type_member(&self,id:NativeTypeMemberKey)->bool {self.base.contains_type_member(id)}
    fn type_member_header(&self,id:NativeTypeMemberKey)->TypeMemberHeader {self.base.type_member_header(id)}
    fn type_member_type(&self,id:NativeTypeMemberKey,q:&mut dyn TypeQueries)->Type {self.base.type_member_type(id,q)}
    fn type_indexer_key(&self,id:NativeTypeMemberKey,q:&mut dyn TypeQueries)->Type {self.base.type_indexer_key(id,q)}
    fn contains_class_definition(&self,id:NativeClassDefinitionKey)->bool {self.base.contains_class_definition(id)}
    fn class_definition_header(&self,id:NativeClassDefinitionKey)->ClassDefinitionHeader {self.base.class_definition_header(id)}
    fn contains_type_parameter(&self,id:NativeTypeParameterKey)->bool {self.base.contains_type_parameter(id)||id.snapshot==92001&&id.index>=1000&&((id.index-1000)as usize)<self.base.heads.len()}
    fn type_parameter_name(&self,id:NativeTypeParameterKey)->String {if id.index>=1000{"self".into()}else{self.base.type_parameter_name(id)}}
    fn type_parameter_owner(&self,id:NativeTypeParameterKey)->Option<TypeDeclarationOwner> {if id.index>=1000{Some(TypeDeclarationOwner::NamedType(key(id.index-1000)))}else{self.base.type_parameter_owner(id)}}
    fn has_type_parameter_constraint(&self,id:NativeTypeParameterKey)->bool {id.index>=1000}
    fn type_parameter_constraint(&self,id:NativeTypeParameterKey,q:&mut dyn TypeQueries)->Option<Type> {if id.index>=1000{Some(q.named_declared_type(key(id.index-1000)).unwrap())}else{None}}
    fn type_declaration_error(&self,error:NamedTypeError){self.base.type_declaration_error(error)}
    fn contains_function(&self,_:NativeFunctionKey)->bool {false}
    fn function_parameters(&self,_:NativeFunctionKey,_:&mut dyn TypeQueries)->Vec<Parameter>{unreachable!()}
    fn function_declared_return(&self,_:NativeFunctionKey,_:&mut dyn TypeQueries)->Option<Type>{unreachable!()}
    fn function_inferred_return(&self,_:NativeFunctionKey,_:&mut dyn TypeQueries)->Type{unreachable!()}
    fn return_cycle(&self,_:NativeFunctionKey){unreachable!()}
    fn return_type_too_deep(&self,_:NativeFunctionKey){unreachable!()}
}
fn provider(heads:Vec<Head>)->SelfProvider {SelfProvider{base:Provider::new(heads)}}

#[test]
fn semantic_self_is_not_a_written_generic_and_body_self_stays_polymorphic() {
    Engine::new().with_native_provider(||provider(vec![Head::class(vec![fluent(0,true)]),Head::class(vec![])]),|_,q| {
        let class=q.named_type_reference(key(0),&[]).unwrap();let body=Type::NativeTypeParameter(self_key(0));
        assert!(q.is_assignable(&body,&class));assert!(!q.is_assignable(&class,&body),"the base class cannot be returned as an arbitrary subtype self");
        let method=q.property_type(&body,"value").unwrap();let signature=q.function_signature(&method).unwrap();assert_eq!(signature.params.len(),1);assert_eq!(signature.params[0].ty,body);assert_eq!(*signature.returns,body);
        let called=q.call(&method,&[body.clone()]);assert!(called.errors.is_empty(),"{:?}",called.errors);assert_eq!(called.return_type,body);
        assert_eq!(q.named_receiver_type(&body,&body).unwrap(),body);
        assert!(matches!(q.named_type_reference(key(0),&[Type::Number]),Err(NamedTypeError::TypeArgumentArity{maximum:0,..})));
        let unrelated=q.named_type_reference(key(1),&[]).unwrap();assert!(matches!(q.named_receiver_type(&class,&unrelated),Err(NamedTypeError::InvalidReceiver{..})));
    });
}

#[test]
fn inherited_self_uses_actual_derived_receiver_and_existing_generic_mapper() {
    Engine::new().with_native_provider(|| {
        let mut base=Head::class(vec![fluent(0,true),Type::NativeTypeParameter(param(0))]);base.generic=true;
        let mut child=Head::class(vec![]);child.generic=true;child.bases=vec![reference(0,vec![Type::NativeTypeParameter(param(1))])];provider(vec![base,child])
    },|_,q| {
        let derived=q.named_type_reference(key(1),&[Type::Number]).unwrap();let base=q.named_type_reference(key(0),&[Type::Number]).unwrap();
        let method=q.property_type(&derived,"value").unwrap();let called=q.call(&method,&[derived.clone()]);assert!(called.errors.is_empty(),"{:?}",called.errors);assert_eq!(called.return_type,derived);
        assert_eq!(q.property_type(&called.return_type,"other"),Some(Type::Number));
        assert!(q.is_assignable(&derived,&base),"runtime receiver parameter0 must not break ordinary nominal ancestry");
    });
}

#[test]
fn dot_self_return_adds_no_runtime_receiver_parameter() {
    Engine::new().with_native_provider(||provider(vec![Head::class(vec![fluent(0,false)])]),|_,q| {
        let class=q.named_type_reference(key(0),&[]).unwrap();let function=q.property_type(&class,"value").unwrap();assert!(q.function_signature(&function).unwrap().params.is_empty());
        let call=q.call(&function,&[]);assert!(call.errors.is_empty());assert_eq!(call.return_type,class);
    });
}

#[test]
fn partial_definition_is_not_promoted_and_complete_definition_self_keeps_actual_role() {
    Engine::new().with_native_provider(||provider(vec![Head::class(vec![fluent(0,true),Type::Number])]),|_,q| {
        let class=q.named_type_reference(key(0),&[]).unwrap();let partial=q.class_definition_value(owner(0),&class,&Type::Table(TableType::default())).unwrap();
        assert!(matches!(q.named_receiver_type(&partial,&partial),Err(NamedTypeError::InvalidReceiver{..})));assert!(!q.is_assignable(&partial,&class));
        let read=q.property_type(&partial,"other").unwrap();assert!(q.is_assignable(&Type::Number,&read));assert!(q.is_assignable(&Type::Nil,&read));
        let actual=Type::Table(TableType{fields:vec![TableField::new(LuaKey::String(b"value".to_vec()),fluent(0,true)),TableField::new(LuaKey::String(b"other".to_vec()),Type::Number),TableField::new(LuaKey::String(b"extra".to_vec()),Type::String)],..Default::default()});
        let complete=q.class_definition_value(owner(0),&class,&actual).unwrap();assert!(q.is_assignable(&complete,&class));let method=q.property_type(&complete,"value").unwrap();
        let called=q.call(&method,&[complete.clone()]);assert!(called.errors.is_empty(),"{:?}",called.errors);assert_eq!(called.return_type,complete);assert!(q.format(&called.return_type).starts_with("def "));assert_eq!(q.property_type(&called.return_type,"extra"),Some(Type::String));
    });
}

#[test]
fn explicit_self_binding_survives_subsequent_mapper_and_does_not_reset_to_class() {
    with_program(|program,handles| {
        let mut derived=Head::class(vec![]);derived.bases=vec![reference(0,vec![])];let p=provider(vec![Head::class(vec![fluent(0,true)]),derived]);
        let adapter=ProviderAdapter{provider:&p,handles};let mut checker=program.checker_with_native_provider(&adapter);checker.set_native_type_declaration_provider(&adapter);checker.enable_native_arrays();
        let class=checker.native_declared_head(native_named::core_declaration(key(0))).unwrap();let derived=checker.native_declared_head(native_named::core_declaration(key(1))).unwrap();let formal=import_type(&mut checker,handles,&Type::NativeTypeParameter(self_key(0)));
        let bound=checker.native_receiver_type(class,formal).unwrap();assert_eq!(checker.native_receiver_type(bound,class).unwrap(),bound,"an explicit self binding wins over a later default lookup receiver");
        let mapper=checker.mapper_from(&[formal],&[derived]);let mapped=checker.instantiate(bound,mapper);assert_ne!(mapped,bound,"receiver type-variable flags require instantiation even with no written class generics");
        let mapped=export_type(&mut checker,handles,mapped);let derived=export_type(&mut checker,handles,derived);let mut q=SessionQueries{checker:&mut checker,handles};let method=q.property_type(&mapped,"value").unwrap();let called=q.call(&method,&[derived.clone()]);assert!(called.errors.is_empty(),"{:?}",called.errors);assert_eq!(called.return_type,derived);
    });
}

#[test]
fn materialized_definition_self_returns_exact_partial_view_carrier_after_complete_bound() {
    with_program(|program,handles| {
        let p=provider(vec![Head::class(vec![fluent(0,true),Type::Number])]);let adapter=ProviderAdapter{provider:&p,handles};let mut checker=program.checker_with_native_provider(&adapter);checker.set_native_type_declaration_provider(&adapter);checker.enable_native_arrays();
        let class=checker.native_declared_head(native_named::core_declaration(key(0))).unwrap();let actual=Type::Table(TableType{fields:vec![TableField::new(LuaKey::String(b"value".to_vec()),fluent(0,true)),TableField::new(LuaKey::String(b"other".to_vec()),Type::Number),TableField::new(LuaKey::String(b"extra".to_vec()),Type::Boolean)],..Default::default()});let actual=import_type(&mut checker,handles,&actual);
        let definition=checker.native_definition_value(bun_sema::types::NativeDefinitionId{snapshot:92001,index:0},class,actual).unwrap();let table=checker.native_object_resolved(definition).unwrap();let table=export_type(&mut checker,handles,table);let mut q=SessionQueries{checker:&mut checker,handles};
        let method=q.property_type(&table,"value").unwrap();let called=q.call(&method,&[table.clone()]);assert!(called.errors.is_empty(),"{:?}",called.errors);assert_eq!(called.return_type,table);assert!(q.format(&called.return_type).starts_with("def "));assert_eq!(q.property_type(&called.return_type,"extra"),Some(Type::Boolean));
    });
}

#[test]
fn structural_inheritance_cannot_bind_class_self_without_proving_nominal_constraint() {
    Engine::new().with_native_provider(|| {
        let base=Head::class(vec![fluent(0,true)]);let mut shape=Head::class(vec![]);shape.kind=TypeDeclarationKind::Shape;shape.bases=vec![reference(0,vec![])];provider(vec![base,shape])
    },|_,q| {
        assert!(matches!(q.validate_named_declaration(key(1)),Err(NamedTypeError::InvalidReceiver{..})),"a structural shape does not acquire the class's nominal self bound");
    });
}

#[test]
fn fixed_method_store_updates_actual_definition_presence_without_promoting_missing_members() {
    Engine::new().with_native_provider(||provider(vec![Head::class(vec![fluent(0,true),Type::Number])]),|_,q| {
        let class=q.named_type_reference(key(0),&[]).unwrap();
        let actual=Type::Table(TableType{fields:vec![TableField::new(LuaKey::String(b"other".to_vec()),Type::Number)],..Default::default()});
        let partial=q.class_definition_value(owner(0),&class,&actual).unwrap();
        assert!(matches!(q.property_type_checked(&partial,"value"),Err(TableError::ValueType{..})),"a receiver failure must survive typed lookup");
        let complete=q.definition_field_write(&partial,&LuaKey::String(b"value".to_vec()),&fluent(0,true)).unwrap();
        assert!(q.is_assignable(&complete,&class));
        assert!(q.is_assignable(&complete,&partial),"the earlier partial constraint retains its view while binding self to this exact receiver");
        let method=q.property_type_checked(&complete,"value").unwrap().unwrap();
        let called=q.call(&method,&[complete.clone()]);assert!(called.errors.is_empty(),"{:?}",called.errors);assert_eq!(called.return_type,complete);
        assert!(matches!(q.definition_field_write(&complete,&LuaKey::String(b"other".to_vec()),&Type::String),Err(TableError::ValueType{..})));
        assert!(matches!(q.definition_field_write(&complete,&LuaKey::String(b"other".to_vec()),&Type::Nil),Err(TableError::ValueType{..})));
        assert_eq!(q.property_type_checked(&complete,"missing").unwrap(),None);
        assert!(matches!(q.definition_field_write(&actual,&LuaKey::String(b"other".to_vec()),&Type::Number),Err(TableError::NotTable{..})),"a public table cannot acquire definition origin");
        let empty=q.class_definition_value(owner(0),&class,&Type::Table(TableType::default())).unwrap();
        let still_partial=q.definition_field_write(&empty,&LuaKey::String(b"value".to_vec()),&fluent(0,true)).unwrap();
        assert!(!q.is_assignable(&still_partial,&class));
        assert!(matches!(q.property_type_checked(&still_partial,"value"),Err(TableError::ValueType{..})),"the actual write cannot initialize another missing member");
        let number=q.property_type(&still_partial,"other").unwrap();assert!(q.is_assignable(&Type::Nil,&number));
    });
}

#[test]
fn fixed_definition_data_stores_preserve_written_literal_and_broad_schema_semantics() {
    for (declared, copied, permits_two) in [
        (Type::NumberLiteral(1.0), "1", false),
        (Type::Number, "number", true),
        (Type::Union(vec![Type::NumberLiteral(1.0), Type::NumberLiteral(2.0)]), "1", true),
    ] {
        Engine::new().with_native_provider(
            || provider(vec![Head::class(vec![declared])]),
            |_, q| {
                let class = q.named_type_reference(key(0), &[]).unwrap();
                let partial = q.class_definition_value(owner(0), &class, &Type::Table(TableType::default())).unwrap();
                let member = LuaKey::String(b"value".to_vec());
                let first = q.definition_field_write(&partial, &member, &Type::NumberLiteral(1.0)).unwrap();
                let read = q.property_type(&first, "value").unwrap();
                let copy = q.widen(&read);
                assert_eq!(q.format(&copy), copied);
                let second = q.definition_field_write(&first, &member, &Type::NumberLiteral(2.0));
                assert_eq!(second.is_ok(), permits_two, "a previous flow value must not replace the written mutable schema");
                assert!(!q.is_assignable(&Type::Table(TableType::default()), &class));
            },
        );
    }
}

struct DefinitionWrites { method:Type, demands:Cell<usize> }
fn write_graph()->NativeGraphKey {NativeGraphKey{snapshot:92002,region:0,context:0}}
fn write_node(node:u32)->NativeFlowKey {NativeFlowKey{graph:write_graph(),node}}
fn write_ref(reference:u32)->NativeReferenceKey {NativeReferenceKey{graph:write_graph(),reference}}
impl NativeFlowProvider for DefinitionWrites {
    fn contains_graph(&self,g:NativeGraphKey)->bool {g==write_graph()}
    fn contains_reference(&self,r:NativeReferenceKey)->bool {r.graph==write_graph()&&r.reference<2}
    fn node(&self,n:NativeFlowKey)->Option<NativeFlowNode> {
        if n.graph!=write_graph(){return None}
        Some(match n.node {
            0=>NativeFlowNode::Start,
            1=>NativeFlowNode::Write{before:write_node(0),target:write_ref(1),value:NativeValueKey{snapshot:92002,index:0},input:write_node(0)},
            2=>NativeFlowNode::Merge{edges:vec![write_node(0),write_node(1)]},
            _=>return None,
        })
    }
    fn is_shared(&self,_:NativeFlowKey)->bool {false}
    fn write_affects(&self,r:NativeReferenceKey,t:NativeReferenceKey,_:NativeFlowKey)->NativeWriteRelation {if r==write_ref(0)&&t==write_ref(1){NativeWriteRelation::Descendant}else{NativeWriteRelation::Unaffected}}
    fn write_member_key(&self,r:NativeReferenceKey,t:NativeReferenceKey,_:NativeFlowKey)->Option<LuaKey> {(r==write_ref(0)&&t==write_ref(1)).then(||LuaKey::String(b"value".to_vec()))}
    fn value_type(&self,v:NativeValueKey,input:NativeFlowKey,_:&mut dyn TypeQueries)->Type {self.demands.set(self.demands.get()+1);assert_eq!(v,NativeValueKey{snapshot:92002,index:0});assert_eq!(input,write_node(0));self.method.clone()}
    fn call_effects(&self,_:NativeValueKey,_:NativeFlowKey,_:&mut dyn TypeQueries)->NativeCallEffects {NativeCallEffects::None}
    fn flow_error(&self,at:NativeFlowKey,error:NativeFlowError){panic!("unexpected flow error {at:?}: {error:?}")}
}

#[test]
fn shared_flow_fixed_method_store_preserves_straightline_presence_but_not_conditional_presence() {
    Engine::new().with_native_provider(||provider(vec![Head::class(vec![fluent(0,true),Type::Number])]),|_,q| {
        let class=q.named_type_reference(key(0),&[]).unwrap();
        let actual=Type::Table(TableType{fields:vec![TableField::new(LuaKey::String(b"other".to_vec()),Type::Number)],..Default::default()});
        let partial=q.class_definition_value(owner(0),&class,&actual).unwrap();
        let flow=DefinitionWrites{method:fluent(0,true),demands:Cell::new(0)};
        let ordinary=q.flow_type(&flow,write_node(1),write_ref(0),&actual,&actual);assert_eq!(ordinary.ty,actual,"ordinary table policies are unchanged");
        assert_eq!(flow.demands.get(),0,"an irrelevant descendant store must not demand a recursive RHS merely to discard it");
        let straight=q.flow_type(&flow,write_node(1),write_ref(0),&partial,&partial);
        assert_eq!(flow.demands.get(),1,"a real definition store uses the original frozen RHS");
        assert!(!straight.incomplete);assert!(q.is_assignable(&straight.ty,&class),"descendant transfer must retain its new actual view instead of assignment_reduced_type restoring the old partial declaration");
        let method=q.property_type_checked(&straight.ty,"value").unwrap().unwrap();let call=q.call(&method,&[straight.ty.clone()]);assert!(call.errors.is_empty(),"{:?}",call.errors);assert_eq!(call.return_type,straight.ty);
        let conditional=q.flow_type(&flow,write_node(2),write_ref(0),&partial,&partial);
        assert!(!conditional.incomplete);assert!(!q.is_assignable(&conditional.ty,&class),"a conditional member store cannot prove whole-value completeness");
        assert!(matches!(q.property_type_checked(&conditional.ty,"value"),Err(TableError::ValueType{..})));
    });
}
