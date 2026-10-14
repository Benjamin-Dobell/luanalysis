#[path = "../src/native_packs.rs"]
mod native_packs;

use luanalysis::{bind, flow};
use luanalysis_engine::{NativeExpressionKey, Type, ValuePack};
use luanalysis_syntax::{StmtKind, TypeExprKind, TypeLiteral, parse};
use native_packs::{
    Adjustment, AnnotationPack, InitializerContext, ListContext, NativePacks, only_empty_results,
};
use std::collections::HashMap;

#[test]
fn original_lists_expand_only_raw_final_calls_and_varargs() {
    let source =
        "local function f(...) return f(), (f()), ... end\nlocal a,b=f(),f()\nlocal c,d=(f())\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let bound = bind::bind(&file);
    let graph = flow::build(&file, &bound);
    let spans: HashMap<_, _> = graph
        .expressions
        .iter()
        .map(|expression| {
            (
                (
                    expression.expression.span.start,
                    expression.expression.span.end,
                ),
                expression.id.index,
            )
        })
        .collect();
    let snapshot = 901;
    let recipes = NativePacks::new(snapshot, &file, &graph.functions, &|expression| {
        NativeExpressionKey {
            snapshot,
            node: spans[&(expression.span.start, expression.span.end)],
        }
    });
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!()
    };
    let returned = recipes.list_at(function.body[0].span).unwrap();
    assert_eq!(returned.context, ListContext::Return);
    assert_eq!(
        returned
            .expressions
            .iter()
            .map(|expression| expression.adjustment)
            .collect::<Vec<_>>(),
        [Adjustment::Scalar, Adjustment::Scalar, Adjustment::Expand]
    );
    assert_eq!(
        returned.owner.unwrap().index,
        graph.functions[&(function.span.start, function.span.end)]
    );
    let assignment = recipes.list_at(file.statements[1].span).unwrap();
    assert_eq!(
        assignment
            .expressions
            .iter()
            .map(|expression| expression.adjustment)
            .collect::<Vec<_>>(),
        [Adjustment::Scalar, Adjustment::Expand]
    );
    assert_eq!(recipes.get(assignment.key), assignment);
    assert_eq!(
        recipes
            .list_at(file.statements[2].span)
            .unwrap()
            .expressions[0]
            .adjustment,
        Adjustment::Scalar
    );
}

#[test]
fn a_named_last_table_field_prevents_expansion_of_the_previous_sequence() {
    let source = "local function pair() return 1,2 end\nlocal first={pair(), marker=3}\nlocal second={marker=3, pair()}\nlocal third={(pair())}\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let bound = bind::bind(&file);
    let graph = flow::build(&file, &bound);
    let spans: HashMap<_, _> = graph
        .expressions
        .iter()
        .map(|expression| {
            (
                (
                    expression.expression.span.start,
                    expression.expression.span.end,
                ),
                expression.id.index,
            )
        })
        .collect();
    let snapshot = 902;
    let recipes = NativePacks::new(snapshot, &file, &graph.functions, &|expression| {
        NativeExpressionKey {
            snapshot,
            node: spans[&(expression.span.start, expression.span.end)],
        }
    });
    for (statement, (expected, spelling)) in file.statements[1..].iter().zip([
        (Adjustment::Scalar, "pair()"),
        (Adjustment::Expand, "pair()"),
        (Adjustment::Scalar, "(pair())"),
    ]) {
        let StmtKind::Local { values, .. } = &statement.kind else {
            panic!()
        };
        let sequence = recipes.table_sequence(values[0].span).unwrap();
        assert_eq!(sequence.elements.len(), 1);
        assert_eq!(sequence.elements[0].index, 1);
        assert_eq!(sequence.elements[0].expression.adjustment, expected);
        assert_eq!(
            &source[sequence.elements[0].expression.span.start
                ..sequence.elements[0].expression.span.end],
            spelling
        );
    }
}

#[test]
fn correlated_annotation_recipes_keep_canonical_whole_rows_and_tail_indices() {
    let source =
        "---@return false | (true, number, string...)\nlocal function result() return false end\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!()
    };
    let annotation = function.annotations.returns.as_ref().unwrap();
    let recipe = AnnotationPack::annotation(annotation);
    assert_eq!(recipe.types.as_ptr(), annotation.types.as_ptr());
    assert_eq!(recipe.span, annotation.span);
    assert_eq!(recipe.alternatives.len(), 2);
    assert_eq!(recipe.alternatives[0].fixed, 0..1);
    assert_eq!(recipe.alternatives[0].variadic, None);
    assert_eq!(recipe.alternatives[1].fixed, 1..3);
    assert_eq!(recipe.alternatives[1].variadic, Some(3));
    let lowered = recipe.lower(&mut |ty| match &ty.kind {
        TypeExprKind::Literal(TypeLiteral::Boolean(value)) => Type::BooleanLiteral(*value),
        TypeExprKind::Named(name) if name == "number" => Type::Number,
        TypeExprKind::Named(name) if name == "string" => Type::String,
        other => panic!("unexpected original type recipe {other:?}"),
    });
    let ValuePack::Alternatives(rows) = lowered else {
        panic!()
    };
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].fixed[0].ty, Type::BooleanLiteral(false));
    assert_eq!(rows[1].fixed[0].ty, Type::BooleanLiteral(true));
    assert_eq!(rows[1].fixed[1].ty, Type::Number);
    assert_eq!(rows[1].tail.as_deref(), Some(&Type::String));
    assert_eq!(
        &source[recipe.types[3].span.start..recipe.types[3].span.end],
        "string"
    );
}

#[test]
fn empty_function_cardinality_excludes_nested_returns_and_preserves_explicit_nil() {
    let source = "---@return nil\nlocal function empty() local function inner() return 1 end return end\nlocal function one() return nil end\nlocal function forward() return empty() end\n";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let results: Vec<_> = file
        .statements
        .iter()
        .map(|statement| {
            let StmtKind::LocalFunction { function, .. } = &statement.kind else {
                panic!()
            };
            only_empty_results(function)
        })
        .collect();
    assert_eq!(results, [true, false, false]);
}

#[test]
fn initializer_contexts_retain_original_annotation_rows_and_assignment_addresses() {
    let source = "local function owner()\n---@type false|true, string|number\nlocal success,value=pair()\nsuccess,value=pair()\nreturn value\nend";
    let file = parse(source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let bound = bind::bind(&file);
    let graph = flow::build(&file, &bound);
    let spans: HashMap<_, _> = graph
        .expressions
        .iter()
        .map(|expression| {
            (
                (
                    expression.expression.span.start,
                    expression.expression.span.end,
                ),
                expression.id.index,
            )
        })
        .collect();
    let snapshot = 903;
    let recipes = NativePacks::new(snapshot, &file, &graph.functions, &|expression| {
        NativeExpressionKey {
            snapshot,
            node: spans[&(expression.span.start, expression.span.end)],
        }
    });
    let StmtKind::LocalFunction { function, .. } = &file.statements[0].kind else {
        panic!()
    };
    let StmtKind::Local {
        names,
        annotation: Some(annotation),
        ..
    } = &function.body[0].kind
    else {
        panic!()
    };
    let local = recipes.list_at(function.body[0].span).unwrap();
    let (same, InitializerContext::Local(original), index) = recipes
        .initializer_for_expression(local.expressions[0].expression)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(same, local);
    assert_eq!(index, 0);
    assert_eq!(original, annotation);
    assert_eq!(original.types.len(), 2);
    assert!(original.packs.is_none());
    assert_eq!(
        local.owner.unwrap().index,
        graph.functions[&(function.span.start, function.span.end)]
    );
    let assignment = recipes.list_at(function.body[1].span).unwrap();
    let (_, InitializerContext::Assignment(targets), _) = recipes
        .initializer_for_expression(assignment.expressions[0].expression)
        .unwrap()
    else {
        panic!()
    };
    for (target, declaration) in targets.iter().zip(names) {
        assert_eq!(
            bound.binding_at(target.span.start),
            bound.binding_at(declaration.span.start)
        );
        assert_eq!(
            &source[target.span.start..target.span.end],
            declaration.text
        );
    }
}
