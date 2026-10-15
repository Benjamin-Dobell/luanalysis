use super::*;
use luanalysis_engine::NativeAliasDispatch;

struct NoTypeQueries;

impl FlowValueQueries for NoTypeQueries {
    fn environment_access_path(
        &self,
        _: &Expr,
        _: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Vec<flow::Key>> {
        panic!("dispatch metadata must not resolve environment values")
    }
    fn original_type_operation(
        &self,
        _: &ValueRecipe,
        _: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> bool {
        panic!("dispatch metadata must not resolve intrinsic types")
    }
    fn environment_value(
        &self,
        _: Option<BindingId>,
        _: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> Type {
        panic!("dispatch metadata must not resolve environment values")
    }
    fn report_iterator_error(
        &self,
        _: NativeFlowKey,
        _: NativeIteratorStage,
        _: &Type,
        _: luanalysis_engine::CallError,
        _: &mut dyn TypeQueries,
    ) {
        panic!("dispatch metadata must not check iterators")
    }
    fn flow_value_type(
        &self,
        _: &NativeFlowSources,
        _: &ValueRecipe,
        _: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> Type {
        panic!("dispatch metadata must not infer values")
    }
    fn flow_call_effects(
        &self,
        _: &NativeFlowSources,
        _: &ValueRecipe,
        _: NativeFlowKey,
        _: &mut dyn TypeQueries,
    ) -> NativeCallEffects {
        panic!("dispatch metadata must not infer call effects")
    }
    fn report_flow_error(&self, _: Span, _: NativeFlowError) {
        panic!("invalid original dispatch fixture")
    }
}

fn fixture(expression: &str) -> (NativeFlowSources, flow::EvaluationId) {
    let source = format!("local left,right,holder={{}},{{}},{{}}\nlocal result={expression}\n");
    let file = luanalysis_syntax::parse(&source);
    assert!(
        file.diagnostics.is_empty(),
        "{expression}: {:?}",
        file.diagnostics
    );
    let bound = crate::bind::bind(&file);
    let sources = NativeFlowSources::new(&file, &bound, 34_445, &HashMap::new());
    let evaluation = sources
        .program
        .evaluations
        .iter()
        .find(|evaluation| {
            matches!(
                sources
                    .program
                    .expression(evaluation.expression)
                    .expression
                    .kind,
                ExprKind::Binary { .. }
            )
        })
        .expect("the fixture contains one original binary expression")
        .id;
    (sources, evaluation)
}

fn dispatch(expression: &str) -> NativeAliasDispatch {
    let (sources, evaluation) = fixture(expression);
    let values = NoTypeQueries;
    let adapter = FlowAdapter {
        sources: &sources,
        values: &values,
    };
    let evaluation = sources.program.evaluation(evaluation);
    adapter.alias_node_dispatch(sources.flow_key(evaluation.completed, 0))
}

#[test]
fn primitive_literal_equality_is_nondispatching_from_original_syntax_only() {
    for literal in [
        "nil",
        "true",
        "false",
        "'number'",
        "42",
        "-42",
        "((nil))",
        "(('number'))",
        "((-42))",
    ] {
        for operator in ["==", "~="] {
            for expression in [
                format!("left {operator} {literal}"),
                format!("{literal} {operator} right"),
            ] {
                let result = dispatch(&expression);
                assert!(
                    matches!(result, NativeAliasDispatch::NoDispatch),
                    "{expression}: {result:?}"
                );
            }
        }
    }
}

#[test]
fn table_unknown_and_asserted_variable_comparisons_keep_dispatch_unknown() {
    for expression in [
        "left == right",
        "left ~= right",
        "{} == {}",
        "{} ~= left",
        "left == {}",
        "((left)) == ((right))",
        "(--[[---@type string]] left) == right",
        "left ~= (--[[---@type string]] right)",
        "left < 'number'",
        "'number' < right",
        "left < 42",
    ] {
        let result = dispatch(expression);
        assert!(
            matches!(result, NativeAliasDispatch::Unknown),
            "{expression}: {result:?}"
        );
    }
}

#[test]
fn nondispatching_equality_preserves_independent_operand_dispatch_nodes() {
    for expression in ["holder.value == 'number'", "probe() ~= 'number'"] {
        let (sources, binary) = fixture(expression);
        let values = NoTypeQueries;
        let adapter = FlowAdapter {
            sources: &sources,
            values: &values,
        };
        let binary = sources.program.evaluation(binary);
        assert!(matches!(
            adapter.alias_node_dispatch(sources.flow_key(binary.completed, 0)),
            NativeAliasDispatch::NoDispatch
        ));
        let operand = sources.program.evaluation(binary.operands[0]);
        if expression.starts_with("holder") {
            assert!(
                matches!(
                    adapter.alias_node_dispatch(sources.flow_key(operand.completed, 0)),
                    NativeAliasDispatch::ReferenceRead(_)
                ),
                "the field operand can still invoke __index"
            );
        } else {
            assert!(
                matches!(
                    sources.program.node(operand.completed).kind,
                    flow::NodeKind::Call { .. }
                ),
                "the operand's call still has its own original effect node"
            );
        }
    }
}
