//! A reusable native argument is checked once per mode and function frame,
//! until the host discards the results recorded since a mark (as it does when
//! it discards the report of a body check that did not complete).
use super::*;
use std::cell::Cell;

struct Arguments {
    reusable: bool,
    checks: Cell<usize>,
}

fn function() -> NativeFunctionKey {
    NativeFunctionKey { snapshot: 6500, index: 0 }
}

fn argument() -> NativeExpressionKey {
    NativeExpressionKey { snapshot: 6500, node: 0 }
}

impl NativeTypeProvider for Arguments {
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        key == function()
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        vec![Parameter { name: "value".into(), ty: Type::Number, optional: false }]
    }
    fn function_declared_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Option<Type> {
        Some(Type::Nil)
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        panic!("the function's return is written")
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("the function's return is not recursive")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("the function's return is finite")
    }
    fn contains_expression(&self, key: NativeExpressionKey) -> bool {
        key == argument()
    }
    fn check_expression(&self, _: NativeExpressionKey, _: Option<&Type>, _: NativeCheckMode, _: &mut dyn TypeQueries) -> Type {
        self.checks.set(self.checks.get() + 1);
        Type::Number
    }
    fn check_reusable_expression(
        &self,
        key: NativeExpressionKey,
        expected: Option<&Type>,
        mode: NativeCheckMode,
        queries: &mut dyn TypeQueries,
    ) -> (Type, bool) {
        (self.check_expression(key, expected, mode, queries), self.reusable)
    }
}

fn call(queries: &mut dyn TypeQueries) {
    let result = queries.call_native_arguments_scalar(
        &Type::NativeFunction(function()), None,
        &[PackArgument::Expression(argument())], &[], None,
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
}

fn checks(reusable: bool) -> (usize, usize, usize) {
    Engine::new().with_native_provider(
        move || Arguments { reusable, checks: Cell::new(0) },
        |provider, queries| {
            let mark = queries.argument_results_mark();
            call(queries);
            let first = provider.checks.get();
            call(queries);
            let second = provider.checks.get();
            queries.discard_argument_results_since(mark);
            call(queries);
            (first, second, provider.checks.get())
        },
    )
}

#[test]
fn a_reusable_argument_is_checked_again_only_after_its_results_are_discarded() {
    let (first, second, discarded) = checks(true);
    assert!(first > 0, "the argument must actually be checked");
    assert_eq!(second, first, "a repeated call reuses the argument's result");
    assert_eq!(discarded, first * 2, "a discarded result is checked again");
}

#[test]
fn an_argument_that_is_not_reusable_is_checked_by_every_call() {
    let (first, second, discarded) = checks(false);
    assert!(first > 0, "the argument must actually be checked");
    assert_eq!(second, first * 2);
    assert_eq!(discarded, first * 3);
}
