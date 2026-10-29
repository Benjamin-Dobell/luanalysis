use luanalysis::{CheckReport, check};

const FORWARD: &str = r#"
---@return string,number
local function produce() return 'ok',42 end
---@generic T
---@param producer PRODUCER
---@return boolean,T
local function tag(producer) return true,producer() end
---@type boolean,string,number
local flag,text,count=tag(produce)
"#;

fn accepted(source: &str) -> CheckReport {
    let report = check(source);
    assert!(
        report.diagnostics.is_empty(),
        "{source}\n{:#?}",
        report.diagnostics
    );
    report
}

fn rejected(source: &str, code: &str) {
    let report = check(source);
    assert_eq!(
        report.diagnostics.len(),
        1,
        "{source}\n{:#?}",
        report.diagnostics
    );
    assert_eq!(
        report.diagnostics[0].code, code,
        "{source}\n{:#?}",
        report.diagnostics
    );
}

#[test]
fn direct_and_aliased_producers_preserve_every_result_and_reject_wrong_later_slots() {
    for (aliases, annotation) in [
        ("", "fun():T"),
        ("---@alias Producer<R> fun():R\n", "Producer<T>"),
        (
            "---@alias Producer<R> fun():R\n---@alias Forward<S> Producer<S>\n---@alias Renamed<U> Forward<U>\n",
            "Renamed<T>",
        ),
        (
            "---@alias Identity<V> V\n---@alias Producer<R> fun():R\n",
            "Producer<Identity<T>>",
        ),
    ] {
        let source = format!("{aliases}{}", FORWARD.replace("PRODUCER", annotation));
        accepted(&source);
        for wrong in ["boolean,string,string", "boolean,number,number"] {
            rejected(&source.replace("boolean,string,number", wrong), "TS2322");
        }
    }
}

#[test]
fn alias_formal_positions_preserve_the_distinction_between_producers_and_scalar_transformers() {
    for annotation in ["fun(value:T):T", "Transformer<T,T>"] {
        let source = format!(
            r#"
---@alias Transformer<Output,Input> fun(value:Input):Output
---@generic T
---@param callback {annotation}
---@param value T
---@return T
local function apply(callback,value) return callback(value) end
---@type number
local result=apply(function(value) return value end,42)
"#
        );
        accepted(&source);
        rejected(
            &source.replace("---@type number", "---@type string"),
            "TS2322",
        );
        rejected(
            &source.replace("return callback(value)", "return 'wrong'"),
            "TS2322",
        );
    }
}

#[test]
fn an_alias_does_not_make_a_returned_getters_captured_scalar_into_an_input_pack() {
    let source = r#"
---@alias Getter<Result> fun():Result
---@generic T
---@param value T
---@return Getter<T>
local function make(value)
    return function() return value end
end
local getter=make(42)
---@type number
local result=getter()
"#;
    accepted(source);
    rejected(
        &source.replace("---@type number", "---@type string"),
        "TS2322",
    );
}

#[test]
fn alias_producers_keep_constraints_on_the_first_result_without_losing_later_results() {
    let source = r#"
---@alias Producer<Result> fun():Result
---@generic T:number
---@param producer Producer<T>
---@return T
local function forward(producer) return producer() end
---@return number,string
local function produce() return 42,'ok' end
---@type number,string
local first,second=forward(produce)
"#;
    accepted(source);
    accepted(&source.replace("Producer<Result>", "Producer<Result:number>"));
    rejected(
        &source.replace("---@type number,string", "---@type number,number"),
        "TS2322",
    );
    let wrong = source.replace(
        "---@return number,string\nlocal function produce() return 42,'ok' end",
        "---@return string,number\nlocal function produce() return 'bad',42 end",
    );
    let report = check(&wrong);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2345" | "TS2769")),
        "{wrong}\n{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_alias_own_shadowing_generic_does_not_capture_the_outer_parameter_with_the_same_name() {
    let source = r#"
---@alias Identity<T> fun<T>(value:T):T
---@generic T
---@param value T
---@param identity Identity<T>
---@return T
local function keep(value,identity)
    local other=identity('other')
    return value
end
---@type number
local result=keep(42,function(value) return value end)
"#;
    accepted(source);
    rejected(
        &source.replace("---@type number", "---@type string"),
        "TS2322",
    );
}

#[test]
fn cyclic_alias_metadata_converges_and_retains_an_independent_producer_branch() {
    let source = r#"
---@alias Recursive<T> {next:Recursive<T>|nil,produce:fun():T}
---@generic R
---@param source Recursive<R>
---@return R
local function forward(source) return source.produce() end
---@return string
local function produce() return 'ok' end
---@type Recursive<string>
local source={produce=produce}
---@type string
local result=forward(source)
"#;
    accepted(source);
    rejected(
        &source.replace(
            "---@type string\nlocal result",
            "---@type number\nlocal result",
        ),
        "TS2322",
    );
    // A concrete scalar alias argument does not authorize extra producer
    // results; recursive source metadata must retain that distinction.
    let wrong = source.replace(
        "---@return string\nlocal function produce() return 'ok' end",
        "---@return string,number\nlocal function produce() return 'ok',42 end",
    );
    let report = check(&wrong);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "TS2322" | "TS2345")),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "LUA2019" | "LUA2031")),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn aliases_with_the_same_name_are_reported_instead_of_choosing_a_producer_optimistically() {
    let source = format!(
        "---@alias Producer<T> fun():T\n---@alias Producer<T> fun(value:T):T\n{}",
        FORWARD.replace("PRODUCER", "Producer<T>")
    );
    let report = check(&source);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.code == "TS2300"),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|error| error.code == "LUA2019"),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn project_aliases_cross_source_snapshots_and_preserve_later_result_errors() {
    use luanalysis::project_check::{ProjectCheckReport, check_project};
    use luanalysis_project::{
        Project, ProjectConfig, ProjectInput, ProjectLoadPlan, Resolver, SourceRole,
    };
    use std::{fs, path::PathBuf};

    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "luanalysis result pack aliases {}",
        std::process::id()
    )));
    fs::create_dir_all(&fixture.0).unwrap();
    let aliases = fixture.0.join("aliases.def.lua");
    let entry = fixture.0.join("entry.lua");
    fs::write(
        &aliases,
        "---@alias Producer<R> fun():R\n---@alias Forward<U> Producer<U>\n",
    )
    .unwrap();
    let run = |source: &str| -> ProjectCheckReport {
        fs::write(&entry, source).unwrap();
        let resolver = Resolver::new(ProjectConfig::new([fixture.0.clone()])).unwrap();
        let mut plan = ProjectLoadPlan::new([entry.clone()]);
        plan.libraries
            .push(ProjectInput::new(aliases.clone(), SourceRole::Declaration));
        check_project(Project::load_plan(&resolver, plan, |_| vec![]))
    };
    let source = FORWARD.replace("PRODUCER", "Forward<T>");
    let report = run(&source);
    for file in &report.files {
        assert!(
            file.report.diagnostics.is_empty(),
            "{}: {:#?}",
            report.source(file.file).unwrap().path.display(),
            file.report.diagnostics
        );
    }
    let report = run(&source.replace("boolean,string,number", "boolean,string,string"));
    let errors: Vec<_> = report
        .files
        .iter()
        .flat_map(|file| &file.report.diagnostics)
        .collect();
    assert_eq!(errors.len(), 1, "{report:#?}");
    assert_eq!(errors[0].code, "TS2322");
}

#[test]
fn erased_alias_arguments_do_not_turn_a_scalar_identity_into_a_pack_contract() {
    for annotation in ["Ignore<fun():T>", "Forward<fun():T>"] {
        let source = format!(
            r#"
---@alias Ignore<Unused> number
---@alias Forward<Value> Ignore<Value>
---@generic T
---@param value T
---@param ignored {annotation}
---@return T
local function identity(value,ignored) return value end
---@type number
local result=identity(42,1)
"#
        );
        accepted(&source);
        rejected(
            &source.replace("---@type number", "---@type string"),
            "TS2322",
        );
    }
    // Conversely, an actually used transparent formal retains its callable.
    let source = format!(
        "---@alias Identity<Value> Value\n{}",
        FORWARD.replace("PRODUCER", "Identity<fun():T>")
    );
    accepted(&source);
    rejected(
        &source.replace("boolean,string,number", "boolean,string,string"),
        "TS2322",
    );
}

#[test]
fn aliased_tuple_members_and_homogeneous_input_results_keep_their_scalar_contracts() {
    let tuple = r#"
---@alias Pair<Value> fun():number,Value
---@generic T
---@param producer Pair<T>
---@return number,T
local function forward(producer) return producer() end
---@return number,string
local function produce() return 42,'ok' end
---@type number,string
local first,second=forward(produce)
"#;
    accepted(tuple);
    rejected(
        &tuple.replace("---@type number,string", "---@type number,number"),
        "TS2322",
    );
    let repeated = r#"
---@alias Repeated<Value> fun():Value...
---@generic T
---@param producer Repeated<T>
---@return T...
local function forward(producer) return producer() end
---@return string...
local function produce() return 'ok','again' end
---@type string|nil,string|nil
local first,second=forward(produce)
"#;
    accepted(repeated);
    rejected(
        &repeated.replace(
            "---@type string|nil,string|nil",
            "---@type string|nil,number|nil",
        ),
        "TS2322",
    );
}

#[test]
fn a_mutually_recursive_alias_without_a_callable_does_not_invent_a_producer() {
    let source = r#"
---@alias Left<T> Right<T>
---@alias Right<U> Left<U>
---@generic T
---@param ignored Left<T>
---@param value T
---@return T
local function keep(ignored,value) return value end
"#;
    let report = check(source);
    assert!(!report.diagnostics.is_empty(), "{report:#?}");
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|error| matches!(error.code, "LUA2019" | "TS2322")),
        "{report:#?}"
    );
}

#[test]
fn an_alias_captured_lexical_parameter_keeps_its_original_outer_pack_identity() {
    let source = r#"
---@generic Outer
---@param producer fun():Outer
---@return Outer
local function run(producer)
    ---@alias Captured<Input> fun(value:Input):Outer
    ---@param callback Captured<number>
    ---@return Outer
    local function forward(callback) return callback(42) end
    return forward(function(value) return producer() end)
end
---@return string,number
local function produce() return 'ok',42 end
---@type string,number
local text,count=run(produce)
"#;
    accepted(source);
    rejected(
        &source.replace("---@type string,number", "---@type string,string"),
        "TS2322",
    );
}

#[test]
fn scalar_any_callback_aliases_preserve_zero_or_one_results_without_becoming_whole_any_packs() {
    let callbacks = [
        "---@return void\nlocal function produce() end\n",
        "---@return number\nlocal function produce() return 42 end\n",
        "---@return number,boolean\nlocal function produce() return 42,false end\n",
    ];
    for annotation in ["fun():any", "Producer<any>"] {
        for (count, callback) in callbacks.iter().enumerate() {
            let source = format!(
                "---@alias Producer<Value> fun():Value\n---@param callback {annotation}\nlocal function consume(callback) return callback() end\n{callback}consume(produce)\n"
            );
            if count <= 1 {
                accepted(&source);
            } else {
                rejected(&source, "TS2345");
            }
        }
    }
    for annotation in ["fun():any...", "WholeAny"] {
        for callback in callbacks {
            accepted(&format!(
                "---@alias WholeAny fun():any...\n---@param callback {annotation}\nlocal function consume(callback) return callback() end\n{callback}consume(produce)\n"
            ));
        }
    }
}

#[test]
fn mapped_void_and_nil_alias_results_preserve_the_direct_callback_cardinality_policy() {
    for (direct, aliased, callbacks) in [
        (
            "fun():void",
            "Producer<void>",
            vec![
                "---@return void\nlocal function produce() end\n",
                "---@return number\nlocal function produce() return 42 end\n",
                "---@return number,boolean\nlocal function produce() return 42,false end\n",
            ],
        ),
        (
            "fun():nil",
            "Producer<nil>",
            vec![
                "---@return void\nlocal function produce() end\n",
                "---@return nil\nlocal function produce() return nil end\n",
            ],
        ),
    ] {
        for annotation in [direct, aliased] {
            for callback in &callbacks {
                accepted(&format!(
                    "---@alias Producer<Value> fun():Value\n---@param callback {annotation}\nlocal function consume(callback) return callback() end\n{callback}consume(produce)\n"
                ));
            }
        }
    }
    for annotation in ["fun():nil", "Producer<nil>"] {
        rejected(
            &format!(
                "---@alias Producer<Value> fun():Value\n---@param callback {annotation}\nlocal function consume(callback) return callback() end\n---@return nil,nil\nlocal function produce() return nil,nil end\nconsume(produce)\n"
            ),
            "TS2345",
        );
    }
}

#[test]
fn direct_and_aliased_returned_getters_keep_the_same_physical_result_requirements() {
    for (body, accepts) in [
        ("", false),
        ("return value", true),
        ("return value,value", false),
    ] {
        let mut outcomes = Vec::new();
        for annotation in ["fun():T", "Getter<T>"] {
            let source = format!(
                r#"
---@alias Getter<Value> fun():Value
---@generic T
---@param value T
---@return {annotation}
local function make(value) return function() {body} end end
local getter=make(42)
"#
            );
            let report = check(&source);
            assert_eq!(
                report.diagnostics.is_empty(),
                accepts,
                "{source}\n{:#?}",
                report.diagnostics
            );
            outcomes.push(
                report
                    .diagnostics
                    .iter()
                    .map(|error| error.code)
                    .collect::<Vec<_>>(),
            );
        }
        assert_eq!(outcomes[0], outcomes[1], "body: {body}");
    }
}

#[test]
fn scalar_any_aliases_cannot_supply_two_required_arguments_by_final_call_expansion() {
    for annotation in ["fun():any", "Producer<any>"] {
        let source = format!(
            r#"
---@alias Producer<Value> fun():Value
---@param first number
---@param second number
local function needs_two(first,second) end
---@param producer {annotation}
local function forward(producer) needs_two(producer()) end
"#
        );
        let report = check(&source);
        assert_eq!(
            report.diagnostics.len(),
            1,
            "{source}\n{:#?}",
            report.diagnostics
        );
        assert_eq!(report.diagnostics[0].code, "TS2554");
        assert_eq!(
            &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
            "needs_two(producer())"
        );
        // A nonfinal call is scalarized to one argument, even if it returns
        // nothing; the separately written final value supplies argument two.
        accepted(&source.replace("needs_two(producer())", "needs_two(producer(),42)"));
    }
}

#[test]
fn returned_scalar_generic_getters_keep_physical_presence_when_instantiated_with_any_or_nil() {
    for value in ["any", "nil"] {
        for annotation in ["fun():T", "Getter<T>"] {
            let source = format!(
                r#"
---@alias Getter<Value> fun():Value
---@generic T
---@param value T
---@return {annotation}
local function make(value) return function() return value end end
---@param value {value}
local function run(value)
    local getter=make(value)
    local function zero() end
    zero(getter())
end
"#
            );
            let report = check(&source);
            assert_eq!(
                report.diagnostics.len(),
                1,
                "{source}\n{:#?}",
                report.diagnostics
            );
            assert_eq!(report.diagnostics[0].code, "TS2554");
            assert_eq!(
                &source[report.diagnostics[0].span.start..report.diagnostics[0].span.end],
                "zero(getter())"
            );
            accepted(&source.replace("zero(getter())", "local result=getter()\n    zero()"));
        }
    }
}

#[test]
fn correlated_owner_rows_take_the_whole_producer_sequence_for_direct_and_aliased_producers() {
    for annotation in ["fun():T", "Producer<T>"] {
        let source = format!(
            r#"
---@alias Producer<Result> fun():Result
---@generic T
---@param producer {annotation}
---@param choose boolean
---@return (true,T)|(false,number)
local function tagged(producer,choose)
    if choose then return true,producer() end
    return false,0
end
---@return string
local function produce() return 'ok' end
---@type boolean,string|number
local flag,value=tagged(produce,true)
"#
        );
        accepted(&source);
        // As v1's `TyMultipleResults.substitute` splices, `T` in each row of a union is the
        // producer's whole result list: `(true, string, number) | (false, number)`.
        let pair = source.replace(
            "---@return string\nlocal function produce() return 'ok' end",
            "---@return string,number\nlocal function produce() return 'ok',42 end",
        );
        accepted(&pair);
        let pair = pair.replace(
            "---@type boolean,string|number\nlocal flag,value=",
            "---@type boolean,string|number,number|nil\nlocal flag,value,count=",
        );
        accepted(&pair);
        rejected(&pair.replace("number|nil", "number"), "TS2322");
        let missing = source.replace("return true,producer()", "return true");
        rejected(&missing, "TS2322");
        let corrected = missing.replace(
            "---@return (true,T)|(false,number)",
            "---@return true|(true,T)|(false,number)",
        );
        // The explicit one-result success row leaves the second destination
        // absent, so callers must include nil in that value's written domain.
        rejected(&corrected, "TS2322");
        accepted(&corrected.replace(
            "---@type boolean,string|number",
            "---@type boolean,string|number|nil",
        ));
    }
}
