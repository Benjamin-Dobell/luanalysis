//! Original inline assertion metadata; all exclusion and relations are native.
use super::native_packs::AnnotationPack;
use super::{Checker, EvaluatedExpression, describe};
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
            native.expression_assertions(expression.span)
        })
    }
    fn assertion_pack(&mut self, assertion: &ExpressionAssertion) -> ValuePack {
        AnnotationPack::annotation(&assertion.annotation).lower(&mut |ty| self.annotation(ty))
    }
    fn assertion_overlap(&mut self, source: &Type, target: &Type, assertion: &ExpressionAssertion) {
        if !self.engine.assertion_overlap(source, target) {
            self.error("TS2352", assertion.span, format!("Conversion of type '{}' to type '{}' may be a mistake because neither type sufficiently overlaps with the other.", describe(source), describe(target)));
        }
    }
    fn asserted_report(&mut self, expression: &Expr, ty: &Type) {
        self.report
            .types
            .retain(|info| info.span != expression.span);
        self.record_expression_type(expression.span, ty);
    }
    pub(super) fn asserted_expression_value(&mut self, expression: &Expr, capture_origin: bool) -> Option<EvaluatedExpression> {
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
        if let (Some(assertion), Some(target)) = (cast, target) {
            self.assertion_overlap(&value.ty, &target, assertion);
            value.ty = target;
        } else {
            for assertion in &assertions {
                let pack = self.assertion_pack(assertion);
                let excluded = self.engine.pack_slot(&pack, 0);
                value.ty = self.engine.exclude_type(&value.ty, &excluded);
            }
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
            let source_first = self.engine.pack_slot(&value, 0);
            let target_first = self.engine.pack_slot(&target, 0);
            self.assertion_overlap(&source_first, &target_first, assertion);
            value = target;
        } else {
            for assertion in &assertions {
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
        self.active_assertions
            .remove(&(expression.span.start, expression.span.end));
        let scalar = self.engine.pack_slot(&value, 0);
        self.asserted_report(expression, &scalar);
        Some(value)
    }
}
