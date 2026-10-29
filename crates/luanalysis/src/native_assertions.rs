//! Original inline assertion metadata; exclusions remain native operations.
use super::native_packs::AnnotationPack;
use super::{Checker, EvaluatedExpression};
use luanalysis_engine::{Type, ValuePack};
use luanalysis_syntax::{Expr, ExpressionAssertion, ExpressionAssertionKind};

impl Checker<'_> {
    fn assertions(&self, expression: &Expr) -> Vec<ExpressionAssertion> {
        if self
            .active_assertions
            .contains(&(expression.span.start, expression.span.end))
        {
            return Vec::new();
        }
        self.native.map_or_else(Vec::new, |native| {
            let mut assertions = native.expression_assertions(expression.span);
            if let Some(annotation) = native.assignment_annotation(expression.span) {
                assertions.push(ExpressionAssertion {
                    kind: ExpressionAssertionKind::Type,
                    annotation: annotation.clone(),
                    expression: expression.span,
                    span: annotation.span,
                });
            }
            assertions
        })
    }
    fn assertion_pack(&mut self, assertion: &ExpressionAssertion) -> ValuePack {
        AnnotationPack::annotation(&assertion.annotation).lower(&mut |ty| self.annotation(ty))
    }
    fn asserted_report(&mut self, expression: &Expr, ty: &Type) {
        self.report
            .types
            .retain(|info| info.span != expression.span);
        self.record_expression_type(expression.span, ty);
    }
    pub(super) fn asserted_expression_value(
        &mut self,
        expression: &Expr,
        capture_origin: bool,
    ) -> Option<EvaluatedExpression> {
        let assertions = self.assertions(expression);
        if assertions.is_empty() {
            return None;
        }
        self.active_assertions
            .insert((expression.span.start, expression.span.end));
        let cast = assertions
            .iter()
            .find(|assertion| assertion.kind == ExpressionAssertionKind::Type);
        let target_pack = cast.map(|assertion| self.assertion_pack(assertion));
        let target = target_pack
            .as_ref()
            .map(|pack| self.engine.pack_slot(pack, 0));
        let context = target.as_ref();
        let mut value = self.expression_value_at_current_flow(expression, context, capture_origin);
        // Keep this evaluation's physical origin even when its static view changes.
        if let Some(target) = target {
            // An assignment's `---@type` is checked, unlike an inline cast.
            if self.native.is_some_and(|native| native.assignment_annotation(expression.span).is_some())
                && self.native.is_none_or(|native| native.expression_assertions(expression.span).is_empty())
            {
                self.compatible(&value.ty, &target, expression.span);
            }
            // V1 @type is an authoritative static view, including unrelated
            // conversions. The operand above still checks calls and bodies.
            value.ty = target;
        }
        // Without a replacing cast, `---@not T` and `---@cast x -T` remove `T`; either way
        // EmmyLua Analyzer's `---@cast x +T` adds it.
        for assertion in assertions.iter().filter(|assertion| assertion.kind == ExpressionAssertionKind::Add || cast.is_none()) {
            let pack = self.assertion_pack(assertion);
            let operand = self.engine.pack_slot(&pack, 0);
            value.ty = match assertion.kind {
                ExpressionAssertionKind::Add => self.engine.union_reduced(&[value.ty.clone(), operand]),
                _ => self.engine.exclude_type(&value.ty, &operand),
            };
        }
        self.active_assertions
            .remove(&(expression.span.start, expression.span.end));
        self.asserted_report(expression, &value.ty);
        Some(value)
    }
    pub(super) fn asserted_expression_pack(&mut self, expression: &Expr) -> Option<ValuePack> {
        let assertions = self.assertions(expression);
        if assertions.is_empty() {
            return None;
        }
        self.active_assertions
            .insert((expression.span.start, expression.span.end));
        let mut value = self.expression_pack(expression, None);
        if let Some(assertion) = assertions
            .iter()
            .find(|assertion| assertion.kind == ExpressionAssertionKind::Type)
        {
            let target = self.assertion_pack(assertion);
            value = target;
        } else {
            for assertion in assertions.iter().filter(|assertion| assertion.kind == ExpressionAssertionKind::Not) {
                let excluded = self.assertion_pack(assertion);
                let rows = self
                    .engine
                    .inspect_pack(&excluded)
                    .expect("original annotation lowers to concrete exclusion recipes");
                let count = rows.iter().map(|row| row.fixed.len()).max().unwrap_or(0);
                let fixed: Vec<_> = (0..count)
                    .map(|index| self.engine.pack_slot(&excluded, index))
                    .collect();
                let tails: Vec<_> = rows
                    .iter()
                    .filter_map(|row| row.tail.as_deref().cloned())
                    .collect();
                let tail = (!tails.is_empty()).then(|| self.engine.union_reduced(&tails));
                value = self.engine.exclude_pack(&value, &fixed, tail.as_ref());
            }
        }
        // EmmyLua Analyzer's `+T` adds to the first value, the one a cast types.
        for assertion in assertions.iter().filter(|assertion| assertion.kind == ExpressionAssertionKind::Add) {
            let pack = self.assertion_pack(assertion);
            let operand = self.engine.pack_slot(&pack, 0);
            let first = self.engine.pack_slot(&value, 0);
            let widened = self.engine.union_reduced(&[first, operand]);
            value = ValuePack::Alternatives(vec![luanalysis_engine::PackAlternative { fixed: vec![luanalysis_engine::PackSlot::required(widened)], tail: None }]);
        }
        self.active_assertions
            .remove(&(expression.span.start, expression.span.end));
        let scalar = self.engine.pack_slot(&value, 0);
        self.asserted_report(expression, &scalar);
        Some(value)
    }
}
