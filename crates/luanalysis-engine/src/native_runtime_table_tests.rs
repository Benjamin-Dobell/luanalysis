use crate::*;
use std::cell::{Cell, RefCell};
fn declaration(index: u32) -> NativeTypeDeclarationKey {
    NativeTypeDeclarationKey {
        snapshot: 6400,
        index,
    }
}
fn member(index: u32) -> NativeTypeMemberKey {
    NativeTypeMemberKey {
        snapshot: 6400,
        index,
    }
}
fn expression(node: u32) -> NativeExpressionKey {
    NativeExpressionKey {
        snapshot: 6400,
        node,
    }
}
fn function(index: u32) -> NativeFunctionKey {
    NativeFunctionKey {
        snapshot: 6400,
        index,
    }
}
fn parameter() -> NativeTypeParameterKey {
    NativeTypeParameterKey {
        snapshot: 6400,
        index: 0,
    }
}
fn inherited_parameter() -> NativeTypeParameterKey {
    NativeTypeParameterKey { snapshot: 6400, index: 1 }
}
fn binding() -> NativeBindingKey {
    NativeBindingKey {
        snapshot: 6400,
        index: 0,
    }
}
fn graph() -> NativeGraphKey {
    NativeGraphKey {
        snapshot: 6401,
        region: 0,
        context: 0,
    }
}
fn at(node: u32) -> NativeFlowKey {
    NativeFlowKey {
        graph: graph(),
        node,
    }
}
fn reference(index: u32) -> NativeReferenceKey {
    NativeReferenceKey {
        graph: graph(),
        reference: index,
    }
}
fn value(index: u32) -> NativeValueKey {
    NativeValueKey {
        snapshot: 6401,
        index,
    }
}
fn key(name: &str) -> LuaKey {
    LuaKey::String(name.as_bytes().to_vec())
}
fn access(name: &str) -> TableKey {
    TableKey::Literal(key(name))
}

struct Member {
    name: &'static str,
    ty: Type,
    origin: TableFieldOrigin,
    indexer: Option<Type>,
}
struct Provider {
    members: Vec<Member>,
    seed: TableType,
    seed_calls: Cell<usize>,
    member_calls: Cell<usize>,
    receiver_calls: RefCell<Vec<u32>>,
    forbidden_receivers: Vec<u32>,
    foreign_receiver: bool,
    inherited_written: bool,
    generic: bool,
    faceted: bool,
    errors: RefCell<Vec<NamedTypeError>>,
}
impl Provider {
    fn new() -> Self {
        Self {
            members: vec![Member {
                name: "value",
                ty: Type::Number,
                origin: TableFieldOrigin::Inferred,
                indexer: None,
            }],
            seed: TableType::default(),
            seed_calls: Cell::new(0),
            member_calls: Cell::new(0),
            receiver_calls: RefCell::new(Vec::new()),
            forbidden_receivers: vec![],
            foreign_receiver: false,
            inherited_written: false,
            generic: false,
            faceted: false,
            errors: RefCell::new(vec![]),
        }
    }
    fn empty(queries: &mut dyn TypeQueries) -> Type {
        queries.table_type(&TableType::default()).unwrap()
    }
    fn allocation(&self, queries: &mut dyn TypeQueries) -> Type {
        let actual = Self::empty(queries);
        queries
            .runtime_table_value(declaration(0), &actual)
            .unwrap()
    }
    fn facet_value(&self, queries: &mut dyn TypeQueries) -> Type {
        if let Some(context) = queries.current_contextual_function() {
            return queries.contextual_parameter_type(&context, 0).unwrap();
        }
        // Canonical declaration frames have lexical ancestry but no runtime
        // binding capture. Only an actual captured value replaces the recipe.
        if let Some(value) = queries.current_captured_function()
            .and_then(|capture| queries.captured_binding(&capture, binding()))
        {
            return value;
        }
        if self.generic {
            Type::NativeTypeParameter(parameter())
        } else {
            Type::Number
        }
    }
}
impl NativeTypeProvider for Provider {
    fn contains_function(&self, original: NativeFunctionKey) -> bool {
        original.snapshot == 6400 && original.index < 4
    }
    fn function_parameters(
        &self,
        original: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        match original.index {
            1 => vec![Parameter {
                name: "value".into(),
                ty: Type::Unknown,
                optional: false,
            }],
            3 => vec![Parameter {
                name: "value".into(),
                ty: Type::NativeTypeParameter(parameter()),
                optional: false,
            }],
            _ => vec![],
        }
    }
    fn function_parameters_in_context(
        &self,
        original: NativeFunctionKey,
        context: &ContextualFunctionContext,
        queries: &mut dyn TypeQueries,
    ) -> Vec<Parameter> {
        if original.index == 1 {
            vec![Parameter {
                name: "value".into(),
                ty: queries.contextual_parameter_type(context, 0).unwrap(),
                optional: false,
            }]
        } else {
            self.function_parameters(original, queries)
        }
    }
    fn function_declared_return(
        &self,
        original: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        (original.index == 2).then_some(Type::Number)
    }
    fn function_inferred_return(
        &self,
        original: NativeFunctionKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        assert_ne!(
            original.index, 2,
            "declared recursive method head must not run its body"
        );
        self.allocation(queries)
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("unexpected return cycle")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("unexpected return depth")
    }
    fn contextual_parameter_indices(&self, original: NativeFunctionKey) -> Vec<usize> {
        if original.index == 1 { vec![0] } else { vec![] }
    }
    fn contains_expression(&self, original: NativeExpressionKey) -> bool {
        original.snapshot == 6400 && original.node < 4
    }
    fn contains_binding(&self, original: NativeBindingKey) -> bool {
        original == binding()
    }
    fn is_context_sensitive(&self, original: NativeExpressionKey) -> bool {
        original.node == 2
    }
    fn has_context_sensitive_parameters(&self, original: NativeExpressionKey) -> bool {
        original.node == 2
    }
    fn contains_type_parameter(&self, original: NativeTypeParameterKey) -> bool {
        self.generic && original == parameter() || self.inherited_written && original == inherited_parameter()
    }
    fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String {
        "T".into()
    }
    fn type_parameter_owner(&self, original: NativeTypeParameterKey) -> Option<TypeDeclarationOwner> {
        Some(if original == inherited_parameter() {
            TypeDeclarationOwner::NamedType(declaration(3))
        } else { TypeDeclarationOwner::Function(function(3)) })
    }
    fn function_type_parameters(
        &self,
        original: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Vec<NativeTypeParameterKey> {
        if self.generic && original.index == 3 {
            vec![parameter()]
        } else {
            vec![]
        }
    }
    fn contains_type_declaration(&self, original: NativeTypeDeclarationKey) -> bool {
        original.snapshot == 6400 && original.index < 4
    }
    fn type_declaration_header(&self, original: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        TypeDeclarationHeader {
            kind: match original.index {
                2 => TypeDeclarationKind::Class,
                3 => TypeDeclarationKind::Shape,
                _ => TypeDeclarationKind::RuntimeTable,
            },
            name: format!("original table {}", original.index),
            object_form: Some(ObjectDeclarationForm::Record),
            parameters: if original.index == 3 { vec![inherited_parameter()] } else { vec![] },
            outer_parameters: if self.generic {
                vec![parameter()]
            } else {
                vec![]
            },
        }
    }
    fn runtime_table_constructor(
        &self,
        original: NativeTypeDeclarationKey,
    ) -> Option<NativeExpressionKey> {
        (original.index < 2).then_some(expression(original.index))
    }
    fn runtime_table_seed(
        &self,
        _: NativeTypeDeclarationKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        self.seed_calls.set(self.seed_calls.get() + 1);
        let mut seed = self.seed.clone();
        if self.faceted {
            seed.fields
                .push(TableField::new(key("seed"), self.facet_value(queries)));
        }
        Some(queries.table_type(&seed).unwrap())
    }
    fn runtime_store_receiver(
        &self,
        store: NativeFlowKey,
        queries: &mut dyn TypeQueries,
    ) -> Option<Type> {
        assert!(!self.forbidden_receivers.contains(&store.node), "context lookup demanded an irrelevant receiver at {store:?}");
        self.receiver_calls.borrow_mut().push(store.node);
        if self.foreign_receiver {
            let actual = Self::empty(queries);
            return Some(queries.runtime_table_value(declaration(1), &actual).unwrap());
        }
        Some(self.allocation(queries))
    }
    fn contains_type_member(&self, original: NativeTypeMemberKey) -> bool {
        original.snapshot == 6400 && (original.index as usize) < self.members.len()
    }
    fn type_member_header(&self, original: NativeTypeMemberKey) -> TypeMemberHeader {
        TypeMemberHeader {
            owner: if self.inherited_written && original.index == 0 {
                TypeMemberOwner::Declared(declaration(3))
            } else { TypeMemberOwner::RuntimeStore(at(original.index + 1)) },
            key: if self.members[original.index as usize].indexer.is_some() {
                TypeMemberKeyRecipe::Indexer { sparse: true }
            } else {
                TypeMemberKeyRecipe::Fixed(key(self.members[original.index as usize].name))
            },
            visibility: TypeMemberVisibility::Public,
            optional: false,
            readonly: false,
        }
    }
    fn type_member_origin(&self, original: NativeTypeMemberKey) -> TableFieldOrigin {
        self.members[original.index as usize].origin
    }
    fn type_indexer_key(&self, original: NativeTypeMemberKey, _: &mut dyn TypeQueries) -> Type {
        self.members[original.index as usize]
            .indexer
            .clone()
            .expect("original computed key domain")
    }
    fn type_member_type(
        &self,
        original: NativeTypeMemberKey,
        queries: &mut dyn TypeQueries,
    ) -> Type {
        self.member_calls.set(self.member_calls.get() + 1);
        if self.faceted || self.generic {
            self.facet_value(queries)
        } else {
            self.members[original.index as usize].ty.clone()
        }
    }
    fn type_declaration_members(&self, _: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> {
        (0..self.members.len() as u32).map(member).collect()
    }
    fn type_declaration_bases(&self, original: NativeTypeDeclarationKey, _: &mut dyn TypeQueries) -> Vec<Type> {
        if self.inherited_written && original.index == 0 {
            vec![Type::NativeNamedReference { declaration: declaration(3), arguments: vec![Type::Number] }]
        } else { vec![] }
    }
    fn type_declaration_error(&self, error: NamedTypeError) {
        self.errors.borrow_mut().push(error);
    }
}

#[test]
fn written_context_filters_inferred_and_other_keys_before_receiver_discovery() {
    for foreign_receiver in [false, true] {
        Engine::new().with_native_provider(
            || {
                let mut provider = Provider::new();
                provider.members[0].origin = TableFieldOrigin::Declared;
                provider.members.push(Member {
                    name: "value", ty: Type::String, origin: TableFieldOrigin::Inferred, indexer: None,
                });
                provider.members.push(Member {
                    name: "other", ty: Type::Boolean, origin: TableFieldOrigin::Declared, indexer: None,
                });
                provider.forbidden_receivers = vec![2, 3];
                provider.foreign_receiver = foreign_receiver;
                provider
            },
            |provider, queries| {
                let table = provider.allocation(queries);
                let context = queries.table_value_context(&table, &access("value"));
                assert_eq!(context.map(|value| queries.inspect_type(&value)), (!foreign_receiver).then_some(Type::Number));
                assert_eq!(*provider.receiver_calls.borrow(), vec![1], "the matching written receiver still establishes actual ownership");
                assert_eq!(provider.member_calls.get(), usize::from(!foreign_receiver));
                assert_eq!(provider.seed_calls.get(), 0, "context does not materialize an evolving seed");
                assert!(queries.table_value_context(&table, &access("absent")).is_none());
                assert_eq!(*provider.receiver_calls.borrow(), vec![1]);
            },
        );
    }
}

#[test]
fn inherited_written_context_maps_the_original_generic_owner_without_inferred_receivers() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            provider.inherited_written = true;
            provider.members[0].origin = TableFieldOrigin::Declared;
            provider.members[0].ty = Type::NativeTypeParameter(inherited_parameter());
            provider.members.push(Member {
                name: "value", ty: Type::String, origin: TableFieldOrigin::Inferred, indexer: None,
            });
            provider.forbidden_receivers = vec![1, 2];
            provider
        },
        |provider, queries| {
            let table = provider.allocation(queries);
            let context = queries.table_value_context(&table, &access("value")).unwrap();
            assert_eq!(queries.inspect_type(&context), Type::Number);
            assert!(queries.is_assignable(&Type::Number, &context));
            assert!(!queries.is_assignable(&Type::String, &context));
            assert!(provider.receiver_calls.borrow().is_empty());
            assert_eq!(provider.member_calls.get(), 1);
            assert!(provider.errors.borrow().is_empty(), "{:#?}", provider.errors.borrow());
        },
    );
}

struct Flow {
    nodes: Vec<NativeFlowNode>,
    values: Vec<Type>,
    weak: Option<NativeWriteRelation>,
}
impl Flow {
    fn write(before: u32, index: u32, target: u32) -> NativeFlowNode {
        NativeFlowNode::Write {
            before: at(before),
            target: reference(target),
            value: value(index),
            input: at(before),
        }
    }
}
impl NativeFlowProvider for Flow {
    fn contains_graph(&self, original: NativeGraphKey) -> bool {
        original == graph()
    }
    fn contains_reference(&self, original: NativeReferenceKey) -> bool {
        original.graph == graph() && original.reference < 2
    }
    fn node(&self, original: NativeFlowKey) -> Option<NativeFlowNode> {
        (original.graph == graph())
            .then(|| self.nodes.get(original.node as usize).cloned())
            .flatten()
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool {
        false
    }
    fn write_affects(
        &self,
        query: NativeReferenceKey,
        target: NativeReferenceKey,
        _: NativeFlowKey,
    ) -> NativeWriteRelation {
        if let Some(weak) = self.weak {
            return weak;
        }
        if query == target {
            NativeWriteRelation::Exact
        } else if query.reference == 0 && target.reference == 1 {
            NativeWriteRelation::Descendant
        } else {
            NativeWriteRelation::Unaffected
        }
    }
    fn write_member_key(
        &self,
        _: NativeReferenceKey,
        target: NativeReferenceKey,
        _: NativeFlowKey,
    ) -> Option<LuaKey> {
        (target.reference == 1).then(|| key("value"))
    }
    fn reference_member_key(
        &self,
        receiver: NativeReferenceKey,
        member: NativeReferenceKey,
    ) -> Option<LuaKey> {
        (receiver.reference == 0 && member.reference == 1).then(|| key("value"))
    }
    fn value_type(
        &self,
        original: NativeValueKey,
        _: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> Type {
        self.values[original.index as usize].clone()
    }
    fn call_effects(
        &self,
        _: NativeValueKey,
        _: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> NativeCallEffects {
        NativeCallEffects::None
    }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) {
        panic!("invalid original graph: {error:?}")
    }
}

#[test]
fn runtime_table_creation_and_future_key_reads_do_not_demand_schema_rhs_values() {
    Engine::new().with_native_provider(Provider::new, |provider, q| {
        let table = provider.allocation(q);
        assert_eq!(provider.seed_calls.get(), 0);
        assert_eq!(q.runtime_table_declaration(&table), Some(declaration(0)));
        assert_eq!(q.table_lookup(&table, &access("value")), Ok(Type::Nil));
        assert_eq!(provider.member_calls.get(), 0);
        assert_eq!(provider.seed_calls.get(), 0);
        assert!(q.table_lookup(&table, &access("unknown")).is_err());
        let schema = q.runtime_table_schema(&table).unwrap();
        assert_eq!(q.runtime_table_declaration(&schema), Some(declaration(0)));
        assert_eq!(q.table_lookup(&schema, &access("value")), Ok(Type::Number));
        assert_eq!(provider.seed_calls.get(), 1);
        assert_eq!(provider.member_calls.get(), 1);
    });
}

#[test]
fn runtime_table_flow_keeps_presence_deletion_exact_replacement_and_branch_alternatives() {
    Engine::new().with_native_provider(Provider::new, |provider, q| {
        let table = provider.allocation(q);
        let schema = q.runtime_table_schema(&table).unwrap();
        let flow = Flow {
            nodes: vec![
                NativeFlowNode::Start,
                Flow::write(0, 0, 1),
                Flow::write(1, 1, 1),
                Flow::write(0, 2, 0),
                NativeFlowNode::Merge {
                    edges: vec![at(0), at(1)],
                },
                NativeFlowNode::Predicate {
                    before: at(4),
                    predicate: NativePredicate::Equal {
                        subject: reference(1),
                        other: value(1),
                        input: at(4),
                        equals: false,
                    },
                    sense: true,
                },
            ],
            values: vec![Type::NumberLiteral(1.0), Type::Nil, table.clone()],
            weak: None,
        };
        for declared in [&schema, &table] {
            let stored = q.flow_type(&flow, at(1), reference(0), declared, &table);
            assert!(!stored.incomplete);
            assert_eq!(
                q.table_lookup(&stored.ty, &access("value")),
                Ok(Type::NumberLiteral(1.0))
            );
            let deleted = q.flow_type(&flow, at(2), reference(0), declared, &table);
            assert_eq!(q.table_lookup(&deleted.ty, &access("value")), Ok(Type::Nil));
            let replaced = q.flow_type(&flow, at(3), reference(0), declared, &table);
            assert_eq!(
                q.runtime_table_declaration(&replaced.ty),
                Some(declaration(0))
            );
            assert_eq!(
                q.table_lookup(&replaced.ty, &access("value")),
                Ok(Type::Nil)
            );
            let joined = q.flow_type(&flow, at(4), reference(0), declared, &table);
            let value = q.table_lookup(&joined.ty, &access("value")).unwrap();
            assert!(q.is_assignable(&Type::NumberLiteral(1.0), &value));
            assert!(
                q.is_assignable(&Type::Nil, &value),
                "join must retain absent branch: {}",
                q.format(&value)
            );
            let narrowed = q.flow_type(&flow, at(5), reference(0), declared, &table);
            assert_eq!(
                q.table_lookup(&narrowed.ty, &access("value")),
                Ok(Type::NumberLiteral(1.0))
            );
        }
    });
}

#[test]
fn original_runtime_schemas_remain_structural_and_actual_views_do_not_gain_future_members() {
    Engine::new().with_native_provider(Provider::new, |provider, q| {
        let actual = q
            .table_type(&TableType {
                fields: vec![TableField::new(key("value"), Type::Number)],
                ..Default::default()
            })
            .unwrap();
        let first = q.runtime_table_value(declaration(0), &actual).unwrap();
        let second = q.runtime_table_value(declaration(1), &actual).unwrap();
        assert!(q.is_assignable(&first, &second));
        assert!(q.is_assignable(&second, &first));
        let shape = Type::Shape(vec![Property {
            name: "value".into(),
            ty: Type::Number,
            optional: false,
            readonly: false,
        }]);
        assert!(q.is_assignable(&first, &shape));
        let empty = provider.allocation(q);
        assert!(!q.is_assignable(&empty, &shape));
        assert!(q.is_assignable(&first, &Type::RuntimeTable));
        assert!(!q.is_assignable(&first, &Type::RuntimeFunction));
        let class = q.named_declared_type(declaration(2)).unwrap();
        assert!(q.runtime_table_schema(&class).is_none());
        assert!(q.runtime_table_value(declaration(2), &actual).is_err());
    });
}

#[test]
fn empty_structural_targets_accept_extras_without_erasing_known_contracts() {
    Engine::new().with_native_provider(Provider::new, |provider, queries| {
        let empty = provider.allocation(queries);
        let fresh = queries.table_type(&TableType {
            fields: vec![
                TableField::new(key("new"), Type::StringLiteral(b"word".to_vec())),
                TableField::new(key("extra"), Type::BooleanLiteral(true)),
            ],
            fresh: true,
            ..Default::default()
        }).unwrap();
        let replacement = queries.runtime_table_value(declaration(1), &fresh).unwrap();
        assert!(queries.is_assignable(&replacement, &empty), "an inferred empty allocation permits fresh structural extras");
        assert!(queries.is_assignable(&fresh, &empty));
        assert_eq!(queries.runtime_table_declaration(&replacement), Some(declaration(1)));
        let written_empty = Provider::empty(queries);
        assert!(queries.is_assignable(&replacement, &written_empty), "written empty shapes follow the same TS empty-object policy");
        assert!(queries.is_assignable(&fresh, &written_empty));
        let known = queries.table_type(&TableType {
            fields: vec![TableField::new(key("new"), Type::String)],
            ..Default::default()
        }).unwrap();
        assert!(!queries.is_assignable(&replacement, &known), "empty-target comparison must not erase original constructor freshness");
        assert!(!queries.is_assignable(&fresh, &known), "known-field targets retain excess-property checking");
        let nonempty = queries.table_type(&TableType {
            fields: vec![TableField::new(key("old"), Type::Number)],
            ..Default::default()
        }).unwrap();
        let nonempty = queries.runtime_table_value(declaration(0), &nonempty).unwrap();
        assert!(!queries.is_assignable(&replacement, &nonempty));
        let nominal = queries.named_declared_type(declaration(2)).unwrap();
        assert!(!queries.is_assignable(&replacement, &nominal));
        assert!(!queries.is_assignable(&replacement, &Type::RuntimeFunction));
        assert_eq!(provider.seed_calls.get(), 0, "actual empty target recognition does not demand prospective schema");
        assert_eq!(provider.member_calls.get(), 0);
    });
}

#[test]
fn original_seed_singletons_validate_later_stores_before_inference_widening() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            provider.seed.fields.push(TableField::declared(
                key("value"),
                Type::NumberLiteral(1.0),
            ));
            provider.seed.fresh = true;
            provider.members[0].ty = Type::NumberLiteral(1.0);
            provider
        },
        |provider, queries| {
            let actual = queries.table_type(&provider.seed).unwrap();
            let table = queries.runtime_table_value(declaration(0), &actual).unwrap();
            let schema = queries.runtime_table_schema(&table).unwrap();
            let value = queries.table_lookup(&schema, &access("value")).unwrap();
            assert_eq!(queries.inspect_type(&value), Type::NumberLiteral(1.0));
            assert!(queries.table_write(&table, &access("value"), &Type::NumberLiteral(1.0)).is_ok());
            assert!(queries.table_write(&table, &access("value"), &Type::NumberLiteral(2.0)).is_err());
            assert!(provider.errors.borrow().is_empty());
        },
    );
}

#[test]
fn original_seed_fields_anchor_contracts_despite_incompatible_future_stores() {
    for origin in [TableFieldOrigin::Inferred, TableFieldOrigin::Declared] {
        Engine::new().with_native_provider(
            || {
                let mut provider = Provider::new();
                provider.seed.fields.push(TableField {
                    origin,
                    ..TableField::new(key("value"), Type::NumberLiteral(1.0))
                });
                provider.seed.fresh = true;
                provider.members[0].ty = Type::StringLiteral(b"wrong".to_vec());
                provider
            },
            |provider, queries| {
                let actual = queries.table_type(&provider.seed).unwrap();
                let table = queries.runtime_table_value(declaration(0), &actual).unwrap();
                let schema = queries.runtime_table_schema(&table).unwrap();
                let value = queries.table_lookup(&schema, &access("value")).unwrap();
                assert_eq!(queries.inspect_type(&value), if origin == TableFieldOrigin::Declared { Type::NumberLiteral(1.0) } else { Type::Number });
                assert!(queries.table_write(&table, &access("value"), &Type::StringLiteral(b"wrong".to_vec())).is_err(), "the original store checker rejects the RHS at its own source location");
                assert!(queries.table_write(&table, &access("value"), &Type::NumberLiteral(1.0)).is_ok(), "an invalid future store cannot destroy the prefix contract");
                assert_eq!(provider.member_calls.get(), 0, "an anchored contract does not depend on a future RHS");
            },
        );
    }
}

#[test]
fn stored_runtime_carriers_clear_freshness_without_losing_allocation_identity() {
    Engine::new().with_native_provider(Provider::new, |_, queries| {
        let actual = queries.table_type(&TableType {
            fields: vec![
                TableField::new(key("value"), Type::NumberLiteral(1.0)),
                TableField::new(key("extra"), Type::BooleanLiteral(true)),
                TableField::declared(key("written"), Type::NumberLiteral(1.0)),
            ],
            fresh: true,
            ..Default::default()
        }).unwrap();
        let table = queries.runtime_table_value(declaration(0), &actual).unwrap();
        let target = queries.table_type(&TableType {
            fields: vec![TableField::new(key("value"), Type::Number)],
            ..Default::default()
        }).unwrap();
        assert!(!queries.is_assignable(&table, &target), "a direct constructor retains excess-property checking");
        let stored = queries.widen(&table);
        assert!(queries.is_assignable(&stored, &target), "a stored table permits structural extra properties");
        assert_eq!(queries.runtime_table_declaration(&stored), Some(declaration(0)));
        assert_eq!(queries.table_lookup(&stored, &access("value")), Ok(Type::Number));
        let written = queries.table_lookup(&stored, &access("written")).unwrap();
        assert_eq!(queries.inspect_type(&written), Type::NumberLiteral(1.0));
        assert!(!queries.is_assignable(&Type::Number, &written));
    });
}

#[test]
fn repeated_inferred_stores_union_and_widen_but_written_contracts_remain_strict() {
    Engine::new().with_native_provider(
        || {
            let mut p = Provider::new();
            p.members.push(Member {
                name: "value",
                ty: Type::StringLiteral(b"text".to_vec()),
                origin: TableFieldOrigin::Inferred,
                indexer: None,
            });
            p
        },
        |p, q| {
            let table = p.allocation(q);
            assert!(
                q.table_value_context(&table, &access("value")).is_none(),
                "inferred RHS cannot contextualize itself"
            );
            let schema = q.runtime_table_schema(&table).unwrap();
            let value = q.table_lookup(&schema, &access("value")).unwrap();
            assert!(
                q.is_assignable(&Type::Number, &value) && q.is_assignable(&Type::String, &value)
            );
            assert!(
                q.table_write(&table, &access("value"), &Type::Boolean)
                    .is_err()
            );
        },
    );
    Engine::new().with_native_provider(
        || {
            let mut p = Provider::new();
            p.members[0].origin = TableFieldOrigin::Declared;
            p
        },
        |p, q| {
            let table = p.allocation(q);
            let context = q.table_value_context(&table, &access("value")).unwrap();
            assert_eq!(q.inspect_type(&context), Type::Number);
            assert!(
                q.table_write(&table, &access("value"), &Type::Number)
                    .is_ok()
            );
            assert!(
                q.table_write(&table, &access("value"), &Type::String)
                    .is_err()
            );
        },
    );
}

#[test]
fn runtime_schema_generic_mapper_and_original_recursive_function_head_are_preserved() {
    Engine::new().with_native_provider(
        || {
            let mut p = Provider::new();
            p.generic = true;
            p
        },
        |_, q| {
            let result = q.call(&Type::NativeFunction(function(3)), &[Type::Number]);
            assert!(result.errors.is_empty(), "{result:?}");
            let schema = q.runtime_table_schema(&result.return_type).unwrap();
            assert_eq!(q.table_lookup(&schema, &access("value")), Ok(Type::Number));
        },
    );
    Engine::new().with_native_provider(
        || {
            let mut p = Provider::new();
            p.members[0].ty = Type::NativeFunction(function(2));
            p
        },
        |p, q| {
            let table = p.allocation(q);
            let schema = q.runtime_table_schema(&table).unwrap();
            let method = q.table_lookup(&schema, &access("value")).unwrap();
            let result = q.call(&method, &[]);
            assert!(result.errors.is_empty(), "{result:?}");
            assert_eq!(result.return_type, Type::Number);
        },
    );
}

#[test]
fn contextual_and_captured_allocation_facets_keep_independent_seed_and_member_queries() {
    Engine::new().with_native_provider(
        || {
            let mut p = Provider::new();
            p.faceted = true;
            p
        },
        |_, q| {
            let mut created = vec![];
            for ty in [Type::Number, Type::String] {
                let expected = Type::Function(FunctionType {
                    params: vec![Parameter {
                        name: "value".into(),
                        ty: ty.clone(),
                        optional: false,
                    }],
                    returns: Box::new(Type::Unknown),
                });
                let contextual = q.contextual_function(
                    function(1),
                    expression(2),
                    &expected,
                    NativeCheckMode::empty(),
                );
                let result = q.call(&contextual, &[ty.clone()]);
                assert!(result.errors.is_empty(), "{result:?}");
                created.push((result.return_type, ty));
            }
            for (table, expected) in created {
                let schema = q.runtime_table_schema(&table).unwrap();
                assert_eq!(
                    q.table_lookup(&schema, &access("value")),
                    Ok(expected.clone())
                );
                assert_eq!(q.table_lookup(&schema, &access("seed")), Ok(expected));
            }
            for ty in [Type::Number, Type::String] {
                let closure = q.captured_function(function(0), &[(binding(), ty.clone())]);
                let result = q.call(&closure, &[]);
                assert!(result.errors.is_empty(), "{result:?}");
                let schema = q.runtime_table_schema(&result.return_type).unwrap();
                assert_eq!(q.table_lookup(&schema, &access("value")), Ok(ty.clone()));
                assert_eq!(q.table_lookup(&schema, &access("seed")), Ok(ty));
            }
            assert!(q.current_contextual_function().is_none());
            assert!(q.current_captured_function().is_none());
        },
    );
}

#[test]
fn future_computed_key_domains_preserve_nil_without_demanding_future_values() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            provider.members[0].indexer = Some(Type::String);
            provider.members[0].origin = TableFieldOrigin::Declared;
            provider
        },
        |provider, queries| {
            let table = provider.allocation(queries);
            assert_eq!(
                queries.table_lookup(&table, &access("future")),
                Ok(Type::Nil)
            );
            assert_eq!(
                queries.table_lookup(&table, &TableKey::Type(Type::String)),
                Ok(Type::Nil)
            );
            assert_eq!(provider.member_calls.get(), 0);
            assert_eq!(provider.seed_calls.get(), 0);
            assert!(
                queries
                    .table_lookup(&table, &TableKey::Literal(LuaKey::Integer(1)))
                    .is_err()
            );
            let schema = queries.runtime_table_schema(&table).unwrap();
            let member = queries.table_lookup(&schema, &access("future")).unwrap();
            assert!(queries.is_assignable(&Type::Number, &member));
            assert!(queries.is_assignable(&Type::Nil, &member));
        },
    );
}

#[test]
fn computed_stores_do_not_invent_an_unwritten_runtime_index_signature() {
    Engine::new().with_native_provider(
        || {
            let mut provider = Provider::new();
            provider.members[0].indexer = Some(Type::String);
            provider
        },
        |provider, queries| {
            let table = provider.allocation(queries);
            assert!(queries.table_lookup(&table, &access("unknown")).is_err());
            assert!(queries.table_write(&table, &TableKey::Type(Type::String), &Type::Number).is_err());
            assert_eq!(provider.member_calls.get(), 0);
        },
    );
}

#[test]
fn possible_alias_writes_preserve_both_actual_presence_continuations() {
    Engine::new().with_native_provider(Provider::new, |provider, queries| {
        for relation in [
            NativeWriteRelation::PossibleExact,
            NativeWriteRelation::PossibleAncestor,
            NativeWriteRelation::PossibleDescendant,
        ] {
            let empty = provider.allocation(queries);
            let actual = queries
                .table_type(&TableType {
                    fields: vec![TableField::new(key("value"), Type::Number)],
                    ..Default::default()
                })
                .unwrap();
            let present = queries
                .runtime_table_value(declaration(0), &actual)
                .unwrap();
            let (initial, assigned, target) = if relation == NativeWriteRelation::PossibleDescendant
            {
                (empty.clone(), Type::Number, 1)
            } else {
                (present, empty, 0)
            };
            let flow = Flow {
                nodes: vec![NativeFlowNode::Start, Flow::write(0, 0, target)],
                values: vec![assigned],
                weak: Some(relation),
            };
            let schema = queries.runtime_table_schema(&initial).unwrap();
            let after = queries.flow_type(&flow, at(1), reference(0), &schema, &initial);
            assert!(!after.incomplete);
            let value = queries.table_lookup(&after.ty, &access("value")).unwrap();
            assert!(
                queries.is_assignable(&Type::Number, &value),
                "{relation:?}: {}",
                queries.format(&value)
            );
            assert!(
                queries.is_assignable(&Type::Nil, &value),
                "{relation:?}: {}",
                queries.format(&value)
            );
        }
    });
}

#[test]
fn exact_replacement_keeps_the_new_constructor_schema_and_old_alias_view_separate() {
    Engine::new().with_native_provider(Provider::new, |_, queries| {
        let original_view = queries
            .table_type(&TableType {
                fields: vec![TableField::new(key("value"), Type::Number)],
                ..Default::default()
            })
            .unwrap();
        let original = queries
            .runtime_table_value(declaration(0), &original_view)
            .unwrap();
        let replacement_view = queries
            .table_type(&TableType {
                fields: vec![TableField::new(key("other"), Type::String)],
                ..Default::default()
            })
            .unwrap();
        let replacement = queries
            .runtime_table_value(declaration(1), &replacement_view)
            .unwrap();
        let flow = Flow {
            nodes: vec![NativeFlowNode::Start, Flow::write(0, 0, 0)],
            values: vec![replacement],
            weak: None,
        };
        let result = queries.flow_type(&flow, at(1), reference(0), &original, &original);
        assert!(!result.incomplete);
        assert_eq!(
            queries.runtime_table_declaration(&result.ty),
            Some(declaration(1))
        );
        assert_eq!(
            queries.table_lookup(&result.ty, &access("other")),
            Ok(Type::String)
        );
        assert!(queries.table_lookup(&result.ty, &access("value")).is_err());
        assert_eq!(
            queries.runtime_table_declaration(&original),
            Some(declaration(0))
        );
        assert_eq!(
            queries.table_lookup(&original, &access("value")),
            Ok(Type::Number)
        );
    });
}
