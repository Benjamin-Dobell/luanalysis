#[path = "support/legacy.rs"]
mod legacy;
use luanalysis::{Diagnostic, DiagnosticCategory};
use luanalysis_fixtures::{Span, import};
use std::{collections::BTreeSet, fs};

fn observed(code: &'static str, start: usize, end: usize) -> Diagnostic {
    Diagnostic {
        code,
        message: "wording is not a test oracle".into(),
        span: luanalysis_syntax::Span { start, end },
    }
}

#[test]
fn matching_checks_semantic_codes_exact_ranges_multiplicity_and_positive_cases() {
    let fixture =
        import("<error descr=\"Type mismatch. Required: 'number' Found: 'string'\">'bad'</error>")
            .unwrap();
    assert_eq!(
        legacy::compare(&fixture, &[observed("TS2322", 0, 5)], false),
        (vec![], vec![])
    );
    for code in ["LUA2031", "LUA1001", "TS2304", "TS6133"] {
        assert_eq!(
            legacy::compare(&fixture, &[observed(code, 0, 5)], false),
            (vec![0], vec![0])
        );
    }
    assert_eq!(
        legacy::compare(&fixture, &[observed("TS2322", 0, 6)], false),
        (vec![0], vec![0])
    );
    let nested = import("<error><error>'bad'</error></error>").unwrap();
    assert_eq!(
        legacy::compare(&nested, &[observed("TS2322", 0, 5)], false),
        (vec![1], vec![])
    );
    let positive = import("local valid = 42").unwrap();
    assert_eq!(
        legacy::compare(&positive, &[observed("TS2322", 0, 5)], false),
        (vec![], vec![0])
    );
}

#[test]
fn undeclared_variable_warnings_translate_only_to_exact_name_errors() {
    let fixture =
        import("local value = <warning descr=\"Undeclared variable 'missing'.\">missing</warning>")
            .unwrap();
    let span = fixture.diagnostics[0].span;
    let error = observed("TS2304", span.start, span.end);
    assert_eq!(error.category(), DiagnosticCategory::Error);
    assert_eq!(legacy::compare(&fixture, &[error], false), (vec![], vec![]));
    assert_eq!(
        legacy::compare(&fixture, &[observed("TS2349", span.start, span.end)], false),
        (vec![0], vec![0])
    );
}

#[test]
fn psi_annotation_ranges_translate_from_the_ast_without_observed_diagnostic_input() {
    let marked = "---@<error descr=\"Alias 'A' circularly references itself.\">alias A A</error>\n";
    let fixture = import(marked).unwrap();
    let translated = legacy::translated_expectations(&fixture);
    assert_eq!(fixture.snippet(&fixture.diagnostics[0]), "alias A A");
    let start = fixture.source.find("A A").unwrap();
    assert_eq!(
        translated[0].span,
        Span {
            start,
            end: start + 1
        }
    );
    assert_eq!(
        translated[0].original_span,
        fixture.diagnostics[0].original_span
    );
    assert!(
        legacy::compare(&fixture, &[observed("TS2456", start, start + 1)], false)
            .0
            .is_empty()
    );
    // The declaration's other identical name must not be accepted as its name.
    assert_eq!(
        legacy::compare(&fixture, &[observed("TS2456", start + 2, start + 3)], false),
        (vec![0], vec![0])
    );
    let fixture = import("---@return string\nlocal function f<error descr=\"Return type 'string' specified but no return values found.\">() end</error>").unwrap();
    let translated = legacy::translated_expectations(&fixture);
    assert_eq!(
        &fixture.source[translated[0].span.start..translated[0].span.end],
        "@return string"
    );
}

#[test]
fn checked_weak_warnings_cannot_be_discarded_or_satisfied_by_an_arbitrary_error() {
    let fixture = import("<weak_warning descr=\"Insufficient assignees, values will be discarded.\">f()</weak_warning>").unwrap();
    assert_eq!(legacy::compare(&fixture, &[], true), (vec![0], vec![]));
    assert_eq!(
        legacy::compare(&fixture, &[observed("TS2554", 0, 3)], true),
        (vec![0], vec![0])
    );
    // The other old harnesses explicitly did not check weak warnings.
    assert_eq!(legacy::compare(&fixture, &[], false), (vec![], vec![]));
}

#[test]
fn completion_negative_quantifier_matches_the_original_assertion() {
    let case = legacy::cases()
        .into_iter()
        .find(|c| c.id == "TestModule/test_module_members_visibility")
        .unwrap();
    assert!(legacy::missing_labels(&case, &BTreeSet::new()).is_empty());
    let one = BTreeSet::from(["myFieldName".to_owned()]);
    assert!(legacy::missing_labels(&case, &one).is_empty());
    let both = BTreeSet::from(["myFieldName".to_owned(), "myFunction".to_owned()]);
    assert_eq!(legacy::missing_labels(&case, &both).len(), 1);
}

#[test]
fn manifest_accounts_for_every_method_resource_group_and_original_option() {
    let cases = legacy::cases();
    assert_eq!(cases.len(), 99);
    let ids: BTreeSet<_> = cases.iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), cases.len());
    let mut inventory_ids = BTreeSet::new();
    let mut methods = 0;
    let mut ide_only = 0;
    let mut unsupported = 0;
    for line in legacy::INVENTORY.lines().filter(|l| !l.starts_with('#')) {
        methods += 1;
        let row: Vec<_> = line.split('\t').collect();
        match row[1] {
            "ide-only" => ide_only += 1,
            "unsupported" => unsupported += 1,
            _ => assert!(!row[2].is_empty()),
        }
        for id in row[2].split('|').filter(|id| !id.is_empty()) {
            assert!(ids.contains(id));
            inventory_ids.insert(id);
        }
    }
    assert_eq!((methods, ide_only, unsupported), (103, 4, 2));
    assert_eq!(ids, inventory_ids);
    for case in &cases {
        assert!(!case.origin.is_empty());
        assert!(!case.notes.contains('\n'));
        for input in &case.inputs {
            assert!(legacy::root().join(input).is_file(), "{input}");
        }
    }
    let case = |id| cases.iter().find(|c| c.id == id).unwrap();
    assert_eq!(
        case("TypeSafetyTest/testRequire").inputs,
        [
            "resources/inspections/requireB.lua",
            "resources/inspections/requireC.lua",
            "resources/inspections/requireA.lua"
        ]
    );
    assert_eq!(
        case("TypeSafetyTest/testClassFromAnotherFile").inputs.len(),
        2
    );
    assert_eq!(case("TypeSafetyTest/testGlobalUsage").inputs.len(), 2);
    assert_eq!(case("TypeSafetyTest/testModules").version, "5.1");
    assert!(case("TypeSafetyTest/testFunctionMultipleReturns").weak);
    assert!(!case("TypeSafetyTest/testNonStrictNil").strict_nil);
    assert!(!case("TypeSafetyTest/testUnknown").strict_nil);
    assert!(case("TypeSafetyTest/testUnknown").unknown);
    // Untrimmed Kotlin triple strings retain their initial newline/indentation.
    let raw = fs::read_to_string(
        legacy::root().join(case("ReturnTypeTest/test_matching_return_type").inputs[0]),
    )
    .unwrap();
    assert!(raw.starts_with("\n        ---@return string"));
    let ledger = include_str!("legacy/SHA256SUMS.tsv");
    let files: Vec<_> = ledger.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(files.len(), 97);
    let mut lua = 0;
    for row in files {
        let original = row
            .split('\t')
            .next()
            .unwrap()
            .strip_prefix("src/test/")
            .unwrap();
        let source = fs::read_to_string(legacy::root().join(original)).unwrap();
        if original.ends_with(".lua") {
            import(&source).unwrap_or_else(|e| panic!("{original}: {e}"));
            lua += 1;
        }
    }
    assert_eq!(lua, 70);
}

fn run(id: &str) {
    let case = legacy::cases()
        .into_iter()
        .find(|case| case.id == id)
        .unwrap();
    let directory = legacy::Temp::new();
    let result = legacy::run(&case, &directory.0);
    assert!(result.passed, "{}: {}", case.id, result.detail);
}

macro_rules! regression {
    ($name:ident, $id:literal) => {
        #[test]
        fn $name() {
            run($id);
        }
    };
}
regression!(
    parsingtest_test_parse_chinese_non_java_characters,
    "ParsingTest/test_parse_chinese_non_java_characters"
);
regression!(testalias_test_alias, "TestAlias/test_alias");
regression!(
    testcompletion_testlocalcompletion,
    "TestCompletion/testLocalCompletion"
);
regression!(
    testcompletion_testglobalcompletion0,
    "TestCompletion/testGlobalCompletion0"
);
regression!(
    testcompletion_testparamcompletion,
    "TestCompletion/testParamCompletion"
);
regression!(
    testcompletion_testannotation0,
    "TestCompletion/testAnnotation0"
);
regression!(
    testcompletion_testannotation1,
    "TestCompletion/testAnnotation1"
);
regression!(
    testcompletion_testannotationarray,
    "TestCompletion/testAnnotationArray"
);
regression!(
    testcompletion_testannotationfun,
    "TestCompletion/testAnnotationFun"
);
regression!(testcompletion_testanonymous, "TestCompletion/testAnonymous");
regression!(
    testcompletion_test_doc_table_1,
    "TestCompletion/test_doc_table_1"
);
regression!(
    testcompletion_testfieldcompletion,
    "TestCompletion/testFieldCompletion"
);
regression!(
    testmodule_test_module_field_completion,
    "TestModule/test_module_field_completion"
);
regression!(
    testmultipleresults_test_multiple_results_1,
    "TestMultipleResults/test_multiple_results_1"
);
regression!(
    testprimitiveliteral_test_string_literals,
    "TestPrimitiveLiteral/test_string_literals"
);
regression!(
    testprimitiveliteral_test_number_literals,
    "TestPrimitiveLiteral/test_number_literals"
);
regression!(
    testprimitiveliteral_test_boolean_literals,
    "TestPrimitiveLiteral/test_boolean_literals"
);
regression!(testself_test_self, "TestSelf/test_self");
regression!(generictest_test_generic_1, "GenericTest/test_generic_1");
regression!(generictest_test_generic_2, "GenericTest/test_generic_2");
regression!(generictest_test_generic_3, "GenericTest/test_generic_3");
regression!(
    generictest_test_custom_iterator,
    "GenericTest/test_custom_iterator"
);
regression!(
    languageleveltest_testallowlua53features,
    "LanguageLevelTest/testAllowLua53Features"
);
regression!(
    languageleveltest_testallowlua54features,
    "LanguageLevelTest/testAllowLua54Features"
);
regression!(
    returntypetest_test_return_type_mismatch,
    "ReturnTypeTest/test_return_type_mismatch"
);
regression!(
    returntypetest_test_matching_return_type,
    "ReturnTypeTest/test_matching_return_type"
);
regression!(
    returntypetest_test_implicit_return_void,
    "ReturnTypeTest/test_implicit_return_void"
);
regression!(
    returntypetest_test_explicit_return_void,
    "ReturnTypeTest/test_explicit_return_void"
);
regression!(
    typesafetytest_testfunctionclosures,
    "TypeSafetyTest/testFunctionClosures"
);
regression!(
    typesafetytest_testfunctioncovariance,
    "TypeSafetyTest/testFunctionCovariance"
);
regression!(
    typesafetytest_testfunctiongenericsscope,
    "TypeSafetyTest/testFunctionGenericsScope"
);
regression!(
    typesafetytest_testgenericclasscovariance,
    "TypeSafetyTest/testGenericClassCovariance"
);
regression!(
    typesafetytest_testlambdaparams,
    "TypeSafetyTest/testLambdaParams"
);
regression!(
    typesafetytest_testlocaldefassignment,
    "TypeSafetyTest/testLocalDefAssignment"
);
regression!(typesafetytest_testrequire, "TypeSafetyTest/testRequire");
regression!(typesafetytest_testsnippet, "TypeSafetyTest/testSnippet");
regression!(typesafetytest_teststrictnil, "TypeSafetyTest/testStrictNil");
regression!(
    typesafetytest_teststringliterals,
    "TypeSafetyTest/testStringLiterals"
);
regression!(
    typesafetytest_testtrailingtype,
    "TypeSafetyTest/testTrailingType"
);
regression!(issues_test81, "Issues/test81");
regression!(issues_test82, "Issues/test82");
regression!(issues_test84, "Issues/test84");
regression!(issues_test85, "Issues/test85");
regression!(issues_test87, "Issues/test87");
regression!(issues_test148, "Issues/test148");
regression!(varargtest_test_vararg_1, "VarargTest/test_vararg_1");
regression!(varargtest_test_vararg_2, "VarargTest/test_vararg_2");
regression!(illegal_alias, "IllegalAliasTest/testIllegalAlias");
regression!(non_return, "ReturnTypeTest/test_non_return");

regression!(
    generic_class_fields,
    "TypeSafetyTest/testGenericClassFields"
);

#[test]
fn member_and_index_range_translations_keep_the_exact_key_identity() {
    let fixture = import(
        "<error descr=\"No such member 'missing' found on type 'Object'\">object.missing</error>",
    )
    .unwrap();
    let expected = legacy::translated_expectations(&fixture);
    assert_eq!(
        &fixture.source[expected[0].span.start..expected[0].span.end],
        "missing"
    );
    let fixture = import(
        "<error descr=\"No such indexer '[number]' found on type 'Object'\">object[key]</error>",
    )
    .unwrap();
    let expected = legacy::translated_expectations(&fixture);
    assert_eq!(
        &fixture.source[expected[0].span.start..expected[0].span.end],
        "key"
    );
    assert!(
        legacy::compare(&fixture, &[observed("TS2538", 7, 10)], false)
            .0
            .is_empty()
    );
    assert_eq!(
        legacy::compare(&fixture, &[observed("TS2538", 0, 6)], false),
        (vec![0], vec![0])
    );
}

#[test]
fn an_outline_with_the_right_flat_labels_cannot_satisfy_the_old_hierarchy() {
    use luanalysis::language_service::{DocumentSymbol, SymbolKind};
    let span = luanalysis_syntax::Span { start: 0, end: 6 };
    let method = DocumentSymbol {
        name: "m:method".into(),
        detail: None,
        kind: SymbolKind::Method,
        range: span,
        selection: span,
        children: vec![],
    };
    let mut class = DocumentSymbol {
        name: "A".into(),
        detail: None,
        kind: SymbolKind::Class,
        range: span,
        selection: span,
        children: vec![],
    };
    assert!(!legacy::symbol_hierarchy_matches(
        &[class.clone(), method.clone()],
        "method",
        "A",
        "method"
    ));
    class.children.push(method);
    assert!(legacy::symbol_hierarchy_matches(
        &[class],
        "method",
        "A",
        "method"
    ));
}
