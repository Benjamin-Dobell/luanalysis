use crate::*;

fn graph(region: u32) -> NativeGraphKey { NativeGraphKey { snapshot: 8701, region, context: 0 } }
fn at(region: u32, node: u32) -> NativeFlowKey { NativeFlowKey { graph: graph(region), node } }
fn reference(region: u32, index: u32) -> NativeReferenceKey { NativeReferenceKey { graph: graph(region), reference: index } }
fn value(index: u32) -> NativeValueKey { NativeValueKey { snapshot: 8701, index } }
fn function() -> NativeFunctionKey { NativeFunctionKey { snapshot: 8702, index: 0 } }
fn declaration() -> NativeTypeDeclarationKey { NativeTypeDeclarationKey { snapshot: 8702, index: 0 } }
fn expression() -> NativeExpressionKey { NativeExpressionKey { snapshot: 8702, node: 0 } }
fn key(name: &str) -> LuaKey { LuaKey::String(name.as_bytes().to_vec()) }

struct Calls;
impl Calls {
    fn initial(&self, queries: &mut dyn TypeQueries) -> Type {
        let table = queries.table_type(&TableType {
            fields: vec![TableField::new(key("current"), Type::Number)],
            ..Default::default()
        }).unwrap();
        queries.runtime_table_value(declaration(), &table).unwrap()
    }
}
impl NativeTypeProvider for Calls {
    fn project_flow_provider(&self) -> Option<&dyn NativeFlowProvider> { Some(self) }
    fn contains_function(&self, key: NativeFunctionKey) -> bool { key == function() }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> { Vec::new() }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> { Some(Type::Nil) }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type { Type::Nil }
    fn return_cycle(&self, _: NativeFunctionKey) { panic!("nonrecursive fixture") }
    fn return_type_too_deep(&self, _: NativeFunctionKey) { panic!("finite fixture") }
    fn contains_expression(&self, key: NativeExpressionKey) -> bool { key == expression() }
    fn contains_type_declaration(&self, key: NativeTypeDeclarationKey) -> bool { key == declaration() }
    fn type_declaration_header(&self, _: NativeTypeDeclarationKey) -> TypeDeclarationHeader {
        TypeDeclarationHeader { kind: TypeDeclarationKind::RuntimeTable, name: String::new(), object_form: Some(ObjectDeclarationForm::Record), parameters: Vec::new(), outer_parameters: Vec::new() }
    }
    fn runtime_table_constructor(&self, _: NativeTypeDeclarationKey) -> Option<NativeExpressionKey> { Some(expression()) }
    fn type_declaration_members(&self, _: NativeTypeDeclarationKey) -> Vec<NativeTypeMemberKey> { Vec::new() }
    fn function_flow_execution(&self, original: NativeFunctionKey, creation: NativeFlowKey, queries: &mut dyn TypeQueries) -> Option<NativeAliasExecution> {
        assert_eq!(original, function());
        assert_eq!(creation, at(0, 2));
        let captured = queries.current_captured_function().expect("selected original callee frame");
        let selected = queries.captured_function_value(&captured);
        assert_eq!(queries.native_function_key(&selected), Some(function()));
        Some(NativeAliasExecution { entry: at(1, 0), terminals: vec![at(1, 1)] })
    }
}
impl NativeFlowProvider for Calls {
    fn contains_graph(&self, key: NativeGraphKey) -> bool { key == graph(0) || key == graph(1) }
    fn contains_reference(&self, key: NativeReferenceKey) -> bool { self.contains_graph(key.graph) && key.reference < 4 }
    fn node(&self, point: NativeFlowKey) -> Option<NativeFlowNode> {
        Some(match (point.graph.region, point.node) {
            (_, 0) => NativeFlowNode::Start,
            (0, 1) => NativeFlowNode::Write { before: at(0, 0), target: reference(0, 0), value: value(0), input: at(0, 0) },
            (0, 2) => NativeFlowNode::Write { before: at(0, 1), target: reference(0, 3), value: value(8), input: at(0, 1) },
            (0, 3) => NativeFlowNode::Write { before: at(0, 2), target: reference(0, 1), value: value(1), input: at(0, 2) },
            (0, 4) => NativeFlowNode::Call { before: at(0, 3), value: value(3), input: at(0, 3) },
            (0, 5) => NativeFlowNode::Write { before: at(0, 4), target: reference(0, 2), value: value(6), input: at(0, 4) },
            (0, 6) => NativeFlowNode::Call { before: at(0, 5), value: value(7), input: at(0, 5) },
            (1, 1) => NativeFlowNode::Write { before: at(1, 0), target: reference(1, 1), value: value(2), input: at(1, 0) },
            _ => return None,
        })
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool { false }
    fn reference_is_cell(&self, key: NativeReferenceKey) -> bool { key.reference == 0 || key.reference == 3 || key == reference(0, 1) }
    fn reference_declaration(&self, key: NativeReferenceKey) -> Option<NativeVariableDeclaration> {
        self.reference_is_cell(key).then(|| NativeVariableDeclaration { owner: graph(0), reference: reference(0, key.reference), writes: vec![] })
    }
    fn reference_in_graph(&self, original: NativeReferenceKey, target: NativeGraphKey) -> Option<NativeReferenceKey> {
        (self.contains_graph(target) && (original.reference == 0 || original.reference == 3))
            .then_some(NativeReferenceKey { graph: target, ..original })
    }
    fn reference_member_path(&self, parent: NativeReferenceKey, child: NativeReferenceKey) -> Option<Vec<LuaKey>> {
        (parent.graph == child.graph && parent.reference == 0 && child.reference == 2).then(|| vec![key("current")])
    }
    fn write_affects(&self, left: NativeReferenceKey, right: NativeReferenceKey, _: NativeFlowKey) -> NativeWriteRelation {
        if left == right { NativeWriteRelation::Exact }
        else if left.graph == right.graph && left.reference == 0 && (right.reference == 2 || right == reference(1, 1)) { NativeWriteRelation::Descendant }
        else { NativeWriteRelation::Unaffected }
    }
    fn write_member_key(&self, _: NativeReferenceKey, target: NativeReferenceKey, _: NativeFlowKey) -> Option<LuaKey> {
        if target == reference(0, 2) { Some(key("current")) }
        else if target == reference(1, 1) { Some(key("saved")) }
        else { None }
    }
    fn alias_node_dispatch(&self, _: NativeFlowKey) -> NativeAliasDispatch { NativeAliasDispatch::NoDispatch }
    fn alias_call_source(&self, original: NativeValueKey, input: NativeFlowKey) -> Option<NativeAliasCall> {
        matches!(original.index, 3 | 7).then(|| NativeAliasCall {
            callee: NativeAliasReturnValue { value: value(4), input }, arguments: Vec::new(), execution_input: input,
        })
    }
    fn value_alias_source(&self, original: NativeValueKey, input: NativeFlowKey) -> NativeAliasValue {
        match original.index {
            0 => NativeAliasValue::Allocation,
            1 => NativeAliasValue::Function { function: function(), creation: at(0, 2) },
            4 => NativeAliasValue::Reference { reference: reference(0, 1), input },
            _ => NativeAliasValue::NotTable,
        }
    }
    fn value_type(&self, original: NativeValueKey, _: NativeFlowKey, queries: &mut dyn TypeQueries) -> Type {
        match original.index {
            0 => self.initial(queries),
            1 | 4 => Type::NativeFunction(function()),
            2 => {
                assert!(queries.is_native_call_execution(graph(1)));
                let external = queries.flow_type(self, at(1, 0), reference(1, 3), &Type::Boolean, &Type::Boolean);
                assert_eq!(external.ty, Type::Boolean);
                assert!(queries.is_native_call_execution(graph(1)), "suspended caller reads restore the active original entry");
                let bound = Type::Union(vec![Type::Number, Type::String]);
                queries.flow_type(self, at(1, 0), reference(1, 2), &bound, &bound).ty
            },
            6 => Type::String,
            8 => {
                assert!(!queries.is_native_call_execution(graph(1)), "caller values cannot observe callee entry state");
                Type::Boolean
            },
            _ => panic!("only reached original RHS values are demanded"),
        }
    }
    fn call_effects(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects { NativeCallEffects::None }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) { panic!("{error:?}") }
}

#[test]
fn repeated_original_installer_calls_use_each_incoming_view_and_restore_parent_queries() {
    Engine::new().with_native_provider(|| Calls, |provider, queries| {
        let initial = provider.initial(queries);
        for (point, expected) in [(at(0, 4), Type::Number), (at(0, 6), Type::String), (at(0, 4), Type::Number)] {
            let result = queries.flow_type(provider, point, reference(0, 0), &initial, &initial);
            assert!(!result.incomplete);
            assert_eq!(queries.table_lookup(&result.ty, &TableKey::Literal(key("saved"))), Ok(expected));
            assert!(!queries.is_native_call_execution(graph(1)));
            assert!(queries.current_captured_function().is_none());
        }
    });
}
