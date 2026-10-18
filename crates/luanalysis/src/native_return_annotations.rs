//! Written original return-list contracts. Native workers own whole-pack
//! context, assignment, publication and physical-result cardinality.
use crate::{Checker, native_packs::AnnotationPack};
use luanalysis_engine::ValuePack;
use luanalysis_syntax::{Expr, Span, TypeAnnotation};

impl crate::native_functions::NativeRegistry {
    pub(super) fn validate_return_annotations(
        &self,
        queries: &mut dyn luanalysis_engine::TypeQueries,
    ) {
        // Written types are declarations even when their runtime return cannot
        // execute. Validate in the original declaration scope without checking
        // the unreachable expression or inventing a contextual invocation.
        for (owner, annotation) in self.packs().return_annotations() {
            let mut checker = self.checker(queries);
            checker.type_context = owner;
            for ty in &annotation.types {
                checker.annotation(ty);
            }
            self.merge_report(checker.report);
        }
    }
}

impl Checker<'_> {
    fn return_annotation(&self, statement: Span) -> Option<TypeAnnotation> {
        self.native?.packs().return_annotation(statement).cloned()
    }
    pub(super) fn return_contract_origin(&self, statement: Span) -> Option<Span> {
        self.native?
            .packs()
            .return_annotation(statement)
            .map(|annotation| annotation.span)
    }
    pub(super) fn return_expression_pack<E: std::borrow::Borrow<Expr>>(
        &mut self,
        values: &[E],
        statement: Span,
        expected: Option<&ValuePack>,
    ) -> ValuePack {
        let Some(annotation) = self.return_annotation(statement) else {
            return self.expression_list_pack(values, expected);
        };
        let declared = AnnotationPack::annotation(&annotation).lower(&mut |ty| self.annotation(ty));
        let declared = self.engine.declared_result_pack(&declared);
        let allowed = self.engine.pack_with_implicit_empty(&declared);
        let actual = self.expression_list_pack(values, Some(&allowed));
        if !self.engine.pack_assignable(&actual, &allowed) {
            self.error(
                "TS2322",
                values.first().map_or(statement, |value| value.borrow().span),
                "Lua result pack is not assignable to the written return-list type.",
            );
        }
        self.engine.return_pack_with_cardinality(&declared, &actual)
    }
}
