use crate::*;
use std::cell::Cell;

fn graph() -> NativeGraphKey { NativeGraphKey { snapshot: 7732, region: 0, context: 0 } }
fn at() -> NativeFlowKey { NativeFlowKey { graph: graph(), node: 0 } }
fn reference(index: u32) -> NativeReferenceKey { NativeReferenceKey { graph: graph(), reference: index } }
fn value() -> NativeValueKey { NativeValueKey { snapshot: 7732, index: 0 } }
fn key(name: &str) -> LuaKey { LuaKey::String(name.as_bytes().to_vec()) }

struct Written {
    bounds: [Option<Type>; 2],
    walks: Cell<usize>,
    forbid_walk: bool,
}
impl Written {
    fn compare(&self, queries: &mut dyn TypeQueries) -> NativeAliasRelation {
        queries.flow_alias_relation(self, reference(0), at(), value(), at())
    }
}
impl NativeFlowProvider for Written {
    fn reference_written_contract(&self, reference: NativeReferenceKey, _: &mut dyn TypeQueries) -> Option<Type> {
        self.bounds[reference.reference as usize].clone()
    }
    fn contains_graph(&self, original: NativeGraphKey) -> bool { original == graph() }
    fn contains_reference(&self, original: NativeReferenceKey) -> bool { original.graph == graph() && original.reference < 2 }
    fn node(&self, _: NativeFlowKey) -> Option<NativeFlowNode> {
        assert!(!self.forbid_walk, "written non-table/disjoint bounds precede any heap provenance walk");
        self.walks.set(self.walks.get() + 1);
        Some(NativeFlowNode::Start)
    }
    fn is_shared(&self, _: NativeFlowKey) -> bool { false }
    fn value_alias_source(&self, _: NativeValueKey, _: NativeFlowKey) -> NativeAliasValue {
        NativeAliasValue::Reference { reference: reference(1), input: at() }
    }
    fn write_affects(&self, _: NativeReferenceKey, _: NativeReferenceKey, _: NativeFlowKey) -> NativeWriteRelation { NativeWriteRelation::Unaffected }
    fn value_type(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> Type { panic!("actual flow types cannot prove written-bound disjointness") }
    fn call_effects(&self, _: NativeValueKey, _: NativeFlowKey, _: &mut dyn TypeQueries) -> NativeCallEffects { NativeCallEffects::None }
    fn flow_error(&self, _: NativeFlowKey, error: NativeFlowError) { panic!("{error:?}") }
}

fn record(queries: &mut dyn TypeQueries, fields: Vec<(&str, Type)>) -> Type {
    queries.table_type(&TableType {
        fields: fields.into_iter().map(|(name, ty)| TableField::declared(key(name), ty)).collect(),
        ..Default::default()
    }).unwrap()
}

#[test]
fn invariant_scalar_bounds_prune_aliases_before_heap_provenance() {
    Engine::new().with_session(|queries| {
        for ty in [Type::Number, Type::String, Type::Boolean, Type::RuntimeFunction, Type::Union(vec![Type::Number, Type::String])] {
            for bounds in [[Some(ty.clone()), None], [None, Some(ty)]] {
                let source = Written { bounds, walks: Cell::new(0), forbid_walk: true };
                assert_eq!(source.compare(queries), NativeAliasRelation::Different);
                assert_eq!(source.walks.get(), 0);
            }
        }
    });
}

#[test]
fn conflicting_required_field_and_written_indexer_bounds_disprove_aliasing() {
    Engine::new().with_session(|queries| {
        let dictionary = queries.table_type(&TableType {
            indexers: vec![TableIndexer::declared(Type::String, Type::Union(vec![Type::Number, Type::String]))],
            ..Default::default()
        }).unwrap();
        let parent = record(queries, vec![("values", dictionary.clone()), ("stable", Type::Union(vec![Type::Number, Type::String]))]);
        let source = Written { bounds: [Some(parent), Some(dictionary)], walks: Cell::new(0), forbid_walk: true };
        assert_eq!(source.compare(queries), NativeAliasRelation::Different);
        assert_eq!(source.walks.get(), 0);
    });
}

#[test]
fn unrelated_required_properties_do_not_disprove_structural_aliases() {
    Engine::new().with_session(|queries| {
        let left = record(queries, vec![("a", Type::Number)]);
        let right = record(queries, vec![("b", Type::String)]);
        let nested_left = record(queries, vec![("shared", left.clone())]);
        let nested_right = record(queries, vec![("shared", right.clone())]);
        for bounds in [[Some(left), Some(right)], [Some(nested_left), Some(nested_right)], [Some(Type::Unknown), Some(Type::RuntimeTable)]] {
            let source = Written { bounds, walks: Cell::new(0), forbid_walk: false };
            assert_eq!(source.compare(queries), NativeAliasRelation::Possible);
            assert!(source.walks.get() > 0);
        }
    });
}

#[test]
fn structural_call_signatures_do_not_imply_a_physical_function_identity() {
    Engine::new().with_session(|queries| {
        let callable = Type::Function(FunctionType { params: vec![], returns: Box::new(Type::Number) });
        let callable_child = record(queries, vec![("child", callable.clone()), ("stable", Type::Union(vec![Type::Number, Type::String]))]);
        let table_child = record(queries, vec![("child", Type::RuntimeTable), ("stable", Type::Union(vec![Type::Number, Type::String]))]);
        for bounds in [[Some(callable), Some(Type::RuntimeTable)], [Some(callable_child), Some(table_child)]] {
            let source = Written { bounds, walks: Cell::new(0), forbid_walk: false };
            assert_eq!(source.compare(queries), NativeAliasRelation::Possible, "a callable table can satisfy both written contracts");
        }
    });
}

#[test]
fn original_union_discriminants_remain_comparable_across_frozen_epochs() {
    Engine::new().with_session(|queries| {
        let kind = Type::Union(vec![Type::StringLiteral(b"old".to_vec()), Type::StringLiteral(b"new".to_vec())]);
        let left = record(queries, vec![("kind", kind.clone()), ("stable", Type::Union(vec![Type::Number, Type::String]))]);
        let right = record(queries, vec![("kind", kind), ("stable", Type::Union(vec![Type::Number, Type::String]))]);
        let source = Written { bounds: [Some(left), Some(right)], walks: Cell::new(0), forbid_walk: false };
        assert_eq!(source.compare(queries), NativeAliasRelation::Possible);
    });
}
