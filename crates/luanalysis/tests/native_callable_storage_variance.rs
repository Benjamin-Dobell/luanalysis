//! Nominal callable contracts can expose heap storage despite having no fields.
use luanalysis::check;

fn relation(contract: &str, source: &str, target: &str, accepted: bool) {
    let source = format!("---@class Callable<T>\n---@overload {contract}\n\n---@param value Callable<{source}>\n---@return Callable<{target}>\nlocal function convert(value) return value end\n");
    let report = check(&source);
    if accepted {
        assert!(report.diagnostics.is_empty(), "{source}\n{:#?}", report.diagnostics);
    } else {
        assert_eq!(report.diagnostics.len(), 1, "{source}\n{:#?}", report.diagnostics);
        let error = &report.diagnostics[0];
        assert_eq!(error.code, "TS2322", "{source}\n{:#?}", report.diagnostics);
        assert_eq!(&source[error.span.start..error.span.end], "value");
    }
}

#[test]
fn callable_class_results_preserve_scalar_covariance() {
    relation("fun():T", "1", "number", true);
    relation("fun():T", "number", "1", false);
}

#[test]
fn callable_class_inputs_preserve_scalar_contravariance() {
    relation("fun(value:T):void", "number", "1", true);
    relation("fun(value:T):void", "1", "number", false);
}

#[test]
fn callable_class_results_cannot_widen_exposed_mutable_arrays_or_records() {
    for contract in ["fun():T[]", "fun():{value:T}"] {
        relation(contract, "1", "number", false);
        relation(contract, "number", "1", false);
    }
}

#[test]
fn callable_class_inputs_cannot_change_the_mutable_domain_of_caller_storage() {
    for contract in ["fun(value:T[]):void", "fun(value:{value:T}):void"] {
        relation(contract, "number", "1", false);
        relation(contract, "1", "number", false);
    }
}

#[test]
fn multiple_scalar_results_are_values_not_mutable_pack_storage() {
    relation("fun():number,T", "1", "number", true);
    relation("fun():number,T", "number", "1", false);
}

#[test]
fn scalar_varargs_preserve_contravariance_without_exposing_the_rest_carrier() {
    relation("fun(...:T):void", "number", "1", true);
    relation("fun(...:T):void", "1", "number", false);
}

#[test]
fn mutable_values_inside_varargs_or_later_results_still_require_invariance() {
    for contract in ["fun(...:T[]):void", "fun(...:{value:T}):void", "fun():number,T[]"] {
        relation(contract, "1", "number", false);
        relation(contract, "number", "1", false);
    }
}
