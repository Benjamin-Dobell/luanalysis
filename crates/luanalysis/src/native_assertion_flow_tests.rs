use super::*;

fn fixture(call: &str) -> (NativeFlowSources, flow::EvaluationId) {
    let prefix = "local subject=nil\nlocal other=true\nlocal holder={};\n";
    let source = format!("{prefix}{call}");
    let file = luanalysis_syntax::parse(&source);
    assert!(file.diagnostics.is_empty(), "{:?}", file.diagnostics);
    let bound = crate::bind::bind(&file);
    let sources = NativeFlowSources::new(&file, &bound, 34_444, &HashMap::default());
    let call = sources
        .program
        .evaluations
        .iter()
        .find(|evaluation| {
            let expression = &sources.program.expression(evaluation.expression).expression;
            expression.span.start == prefix.len()
                && matches!(
                    expression.kind,
                    ExprKind::Call { .. } | ExprKind::MethodCall { .. }
                )
        })
        .unwrap()
        .id;
    (sources, call)
}

#[test]
fn assertion_recipes_keep_original_argument_inputs_and_decline_unknown_calls() {
    for call in [
        "probe(subject)",
        "(probe)((subject),'message')",
        "probe(subject~=nil)",
        "probe(subject and other)",
    ] {
        let (sources, call) = fixture(call);
        let call = sources.program.evaluation(call);
        let input = sources.flow_key(call.input, 0);
        let NativeCallEffects::Assert {
            condition,
            input: argument_input,
        } = sources.assertion_call_effect(call, input)
        else {
            panic!("{call:?}")
        };
        let first = sources.program.evaluation(call.operands[1]);
        assert_eq!(condition, sources.evaluation_value(first.id));
        assert_eq!(argument_input, sources.flow_key(first.input, 0));
        assert!(
            sources
                .assertion_predicate(condition, argument_input)
                .is_some()
        );
    }
    for call in [
        "probe(unknown())",
        "probe(subject,unknown())",
        "probe(subject~=unknown())",
    ] {
        let (sources, call) = fixture(call);
        let call = sources.program.evaluation(call);
        assert_eq!(
            sources.assertion_call_effect(call, sources.flow_key(call.input, 0)),
            NativeCallEffects::None
        );
    }
    let (sources, call) = fixture("probe(false,unknown())");
    let call = sources.program.evaluation(call);
    assert!(matches!(
        sources.assertion_call_effect(call, sources.flow_key(call.input, 0)),
        NativeCallEffects::Assert { .. }
    ));
    let (sources, call) = fixture("probe()");
    let call = sources.program.evaluation(call);
    assert_eq!(
        sources.assertion_call_effect(call, sources.flow_key(call.input, 0)),
        NativeCallEffects::Never
    );
}

#[test]
fn colon_assertion_uses_the_original_receiver_as_its_physical_first_argument() {
    let (sources, call) = fixture("holder:probe(subject)");
    let call = sources.program.evaluation(call);
    let NativeCallEffects::Assert { condition, input } =
        sources.assertion_call_effect(call, sources.flow_key(call.input, 0))
    else {
        panic!("expected receiver predicate")
    };
    let receiver = sources.program.evaluation(call.operands[0]);
    assert_eq!(condition, sources.evaluation_value(receiver.id));
    let NativePredicate::Truthy {
        subject: Some(reference),
        ..
    } = sources.assertion_predicate(condition, input).unwrap()
    else {
        panic!("receiver must keep original reference")
    };
    let original = sources.source_reference(reference).unwrap();
    assert_eq!(original, &sources.program.references[&receiver.expression]);
}

#[test]
fn a_later_unary_call_cannot_preserve_a_guard_assertions_earlier_subject() {
    let (sources, call) = fixture("probe(isGuard(subject),unknown(1))");
    let call = sources.program.evaluation(call);
    assert_eq!(
        sources.assertion_call_effect_with_guard(call, sources.flow_key(call.input, 0), true),
        NativeCallEffects::None,
    );
}
