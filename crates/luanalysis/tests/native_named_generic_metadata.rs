//! A test-only source ownership gate. Named semantic heads remain gated until
//! the native declaration provider is wired; these tests do not expand aliases.
use luanalysis::{Diagnostic, bind};
use luanalysis_engine::NativeFunctionKey;
use luanalysis_syntax::{ExprKind, StmtKind, TypeExprKind};
use luanalysis_engine::{FxHashMap as HashMap, FxHashSet as HashSet};

#[allow(dead_code)]
#[path = "../src/native_named.rs"]
mod native_named;
#[allow(dead_code)]
#[path = "../src/native_types.rs"]
mod native_types;
use native_named::*;
use native_types::*;

const SNAPSHOT: u64 = 505;

fn named(source: &str) -> NativeNamed {
    let file = luanalysis_syntax::parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let bound = bind::bind(&file);
    NativeNamed::new(SNAPSHOT, &file, &bound, &HashMap::default()).unwrap()
}

fn scopes(named: &NativeNamed) -> Vec<NamedGenericScopeRecipe> {
    named
        .declarations()
        .iter()
        .map(|source| source.generic_scope.clone())
        .collect()
}

#[test]
fn one_allocator_separates_same_index_function_and_named_generic_owners() {
    let named = named("---@alias Identity<T> T\n");
    let alias = named.candidates("Identity")[0];
    let function = NativeFunctionKey {
        snapshot: SNAPSHOT,
        index: alias.index,
    };
    let parameter = named.declaration(alias).unwrap().syntax.generics[0].clone();
    let types = NativeTypes::new_with_named(
        SNAPSHOT,
        vec![TypeScopeRecipe {
            owner: function,
            parent: None,
            parameters: vec![TypeParameterRecipe {
                name: parameter.name,
                constraint: None,
            }],
        }],
        scopes(&named),
    );
    let function_parameters = types.own_parameters(function);
    let named_parameters = types.own_parameters_in_context(NativeTypeContext::NamedType(alias));
    assert_eq!(function_parameters[0].index, 0);
    assert_eq!(named_parameters[0].index, 1);
    assert_ne!(function_parameters, named_parameters);
    assert_eq!(
        types.parameter(function_parameters[0]).owner,
        NativeTypeContext::Function(function)
    );
    assert_eq!(
        types.parameter(named_parameters[0]).owner,
        NativeTypeContext::NamedType(alias)
    );
    assert_eq!(
        types.resolve_name_in_context(Some(NativeTypeContext::Function(function)), "T"),
        TypeNameResolution::Parameter(function_parameters[0])
    );
    assert_eq!(
        types.resolve_name_in_context(Some(NativeTypeContext::NamedType(alias)), "T"),
        TypeNameResolution::Parameter(named_parameters[0])
    );
}

#[test]
fn named_sibling_constraints_keep_forward_reference_recipes_and_all_ids() {
    let named = named("---@alias Alias<A:B,B:number> A\n");
    let owner = NativeTypeContext::NamedType(named.candidates("Alias")[0]);
    let types = NativeTypes::new_with_named(SNAPSHOT, Vec::new(), scopes(&named));
    let parameters = types.own_parameters_in_context(owner);
    assert_eq!(parameters.len(), 2);
    assert_eq!(types.siblings(parameters[0]), parameters);
    assert_eq!(types.parameter(parameters[0]).order, 0);
    assert!(
        matches!(&types.parameter(parameters[0]).constraint.as_ref().unwrap().kind,TypeExprKind::Named(name) if name=="B")
    );
    assert_eq!(
        types.resolve_name_in_context(Some(owner), "B"),
        TypeNameResolution::Parameter(parameters[1])
    );
}

#[test]
fn documentation_parameter_candidates_preserve_ambiguity_and_own_shadowing() {
    let named =
        named("---@class One<T>\n---@class Two<T>\n---@alias Both<U> U\n---@alias Own<T> T\n");
    let types = NativeTypes::new_with_named(SNAPSHOT, Vec::new(), scopes(&named));
    let one = types
        .own_parameters_in_context(NativeTypeContext::NamedType(named.candidates("One")[0]))[0];
    let two = types
        .own_parameters_in_context(NativeTypeContext::NamedType(named.candidates("Two")[0]))[0];
    let both = NativeTypeContext::NamedType(named.candidates("Both")[0]);
    assert_eq!(
        types.resolve_name_in_context(Some(both), "T"),
        TypeNameResolution::Ambiguous(vec![one, two])
    );
    let own = NativeTypeContext::NamedType(named.candidates("Own")[0]);
    assert_eq!(
        types.resolve_name_in_context(Some(own), "T"),
        TypeNameResolution::Parameter(types.own_parameters_in_context(own)[0])
    );
}

#[test]
fn named_parameter_duplicates_keep_source_ranges_and_separate_siblings() {
    let source = "---@alias Alias<T:string,T:number> T\n";
    let named = named(source);
    let types = NativeTypes::new_with_named(SNAPSHOT, Vec::new(), scopes(&named));
    let owner = NativeTypeContext::NamedType(named.candidates("Alias")[0]);
    let parameters = types.own_parameters_in_context(owner);
    assert_eq!(parameters.len(), 2);
    assert_ne!(parameters[0], parameters[1]);
    assert_eq!(types.diagnostics().len(), 1);
    let diagnostic = &types.diagnostics()[0];
    assert_eq!(diagnostic.code, "TS2300");
    assert_eq!(&source[diagnostic.span.start..diagnostic.span.end], "T");
    assert_eq!(diagnostic.span.start, source.find(",T").unwrap() + 1);
}

#[test]
fn named_annotation_dependencies_capture_only_used_original_parent_parameters() {
    let source = "---@generic T,U\nlocal function outer()\n---@alias Inside<V> table<string,T[]>\nlocal value=1\nend";
    let file = luanalysis_syntax::parse(source);
    assert!(file.diagnostics.is_empty(), "{:#?}", file.diagnostics);
    let bound = bind::bind(&file);
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!("function")
    };
    let function_key = NativeFunctionKey {
        snapshot: SNAPSHOT,
        index: 0,
    };
    let functions = HashMap::from_iter([((function.span.start, function.span.end), function_key)]);
    let named = NativeNamed::new(SNAPSHOT, &file, &bound, &functions).unwrap();
    let types = NativeTypes::new_with_named(
        SNAPSHOT,
        vec![TypeScopeRecipe {
            owner: function_key,
            parent: None,
            parameters: function
                .annotations
                .generics
                .iter()
                .map(|parameter| TypeParameterRecipe {
                    name: parameter.name.clone(),
                    constraint: parameter.constraint.clone(),
                })
                .collect(),
        }],
        scopes(&named),
    );
    let inside = named.declaration(named.candidates("Inside")[0]).unwrap();
    let owner = NativeTypeContext::NamedType(inside.key);
    let function_parameters = types.own_parameters(function_key);
    // The alias names `T` of the enclosing function; `U` is visible but unused.
    assert_eq!(types.resolve_name_in_context(Some(owner), "T"), TypeNameResolution::Parameter(function_parameters[0]));
    let used = HashSet::from_iter([function_parameters[0]]);
    assert_eq!(
        types.outer_parameters_in_context(owner, &used),
        vec![function_parameters[0]]
    );
    assert!(
        matches!(&file.statements[0].kind,StmtKind::LocalFunction{function,..} if matches!(&function.body[0].kind,StmtKind::Local{values,..} if matches!(values[0].kind,ExprKind::Number(_))))
    );
}

#[test]
fn unused_visible_documentation_parameters_do_not_create_outer_mapper_inputs() {
    let named = named("---@class Owner<T,U>\n---@alias Child<V> V\n");
    let types = NativeTypes::new_with_named(SNAPSHOT, Vec::new(), scopes(&named));
    let owner = NativeTypeContext::NamedType(named.candidates("Child")[0]);
    assert!(
        types
            .outer_parameters_in_context(owner, &HashSet::default())
            .is_empty()
    );
    let doc_owner = NativeTypeContext::NamedType(named.candidates("Owner")[0]);
    let used = HashSet::from_iter([types.own_parameters_in_context(doc_owner)[1]]);
    assert_eq!(
        types.outer_parameters_in_context(owner, &used),
        used.into_iter().collect::<Vec<_>>()
    );
}
