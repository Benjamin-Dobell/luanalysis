//! Lua generic-for hidden state, carried as correlated native result-pack rows.
//! Traversal and loop publication remain in the shared flow walker.

use super::Checker;
use super::native_flow::{NativeFlowKey, NativeFlowProvider, NativeReferenceKey, NativeIteratorProjection, NativeFlowResult, NativeIteratorStage};
use super::native_packs::{NativePackAlternative, NativePackSlot};
use crate::types::{NativeResultPack, TypeId};

impl<'p, 's> Checker<'p, 's> {
    pub(super) fn native_iterator_header(&mut self, pack: NativeResultPack) -> NativeResultPack {
        // Lua adjusts the frozen header list to iterator, invariant state and
        // initial hidden control. The optional fourth closing value does not
        // participate in the iterator call.
        match self.native_adjust_pack(pack, 3) {
            Ok(frame) => frame,
            Err(_) => NativeResultPack { ty: TypeId::ERROR },
        }
    }

    pub(super) fn native_iterator_step(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        stage: NativeIteratorStage,
        frame: NativeResultPack,
    ) -> NativeResultPack {
        let Some(rows) = self.native_pack_alternatives(frame) else {
            provider.flow_error(at, super::native_flow::NativeFlowError::Unsupported("unresolved generic-for frame"));
            return NativeResultPack { ty: TypeId::ERROR };
        };
        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            let row = self.native_result_pack(&[row]);
            let iterator = self.native_pack_slot(row, 0);
            let state = self.native_pack_slot(row, 1);
            let control = self.native_pack_slot(row, 2);
            let called = self.call_resolved_result_pack(iterator, &[state, control], &[], None);
            for error in called.errors {
                provider.iterator_error(at, stage, iterator, error, self);
            }
            let returned = self.native_concat_pack(&[iterator, state], called.pack);
            results.push(returned.ty);
        }
        NativeResultPack { ty: self.union(&results) }
    }

    pub(super) fn native_iterator_filter(&mut self, frame: NativeResultPack, enters: bool) -> NativeResultPack {
        let Some(rows) = self.native_pack_alternatives(frame) else { return frame };
        let mut filtered = Vec::with_capacity(rows.len());
        for mut row in rows {
            let pack = self.native_result_pack(&[row.clone()]);
            let control = self.native_pack_slot(pack, 2);
            let control = self.native_runtime_type(control, &super::native_flow::NativeRuntimeType::Nil, !enters);
            if control.is_never() { continue; }
            if let Some(slot) = row.fixed.get_mut(2) {
                slot.ty = control;
                if enters { slot.optional = false; }
            } else if enters {
                while row.fixed.len() < 2 {
                    row.fixed.push(NativePackSlot { ty: TypeId::UNDEFINED, optional: false });
                }
                row.fixed.push(NativePackSlot { ty: control, optional: false });
            } else {
                // Rest-only returns can produce zero values. A physically
                // present nil first result is another row only when the rest
                // element itself admits nil; retain each complete row.
                let absent = NativePackAlternative { fixed: row.fixed.clone(), tail: None };
                if let Some(tail) = row.tail {
                    let present = self.native_runtime_type(tail, &super::native_flow::NativeRuntimeType::Nil, true);
                    if !present.is_never() {
                        row.fixed.push(NativePackSlot { ty: present, optional: false });
                        filtered.push(row);
                    }
                }
                filtered.push(absent);
                continue;
            }
            filtered.push(row);
        }
        self.native_result_pack(&filtered)
    }

    pub fn native_flow_iterator_pack(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        frame: NativeReferenceKey,
    ) -> NativeResultPack {
        // UNKNOWN is the broad declared carrier, distinct from the entry pack.
        // NEVER is the initial value before IteratorStart. This keeps the
        // shared walker's ordinary entry-first back-edge policy active.
        let result = self.native_flow_iterator_frame(provider, at, frame);
        NativeResultPack { ty: result.ty }
    }

    pub fn native_iterator_result(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        frame: NativeReferenceKey,
        index: usize,
    ) -> TypeId {
        self.native_iterator_result_with_status(provider, at, frame, index).ty
    }

    pub fn native_iterator_result_with_status(
        &mut self,
        provider: &dyn NativeFlowProvider,
        at: NativeFlowKey,
        frame: NativeReferenceKey,
        index: usize,
    ) -> NativeFlowResult {
        let frame = self.native_flow_iterator_frame(provider, at, frame);
        let ty = self.native_pack_slot(NativeResultPack { ty: frame.ty }, index.saturating_add(2));
        NativeFlowResult { ty, incomplete: frame.incomplete, unresolved_auto: false }
    }

    pub fn native_flow_iterator_projection(&mut self, provider: &dyn NativeFlowProvider, at: NativeFlowKey, projection: NativeIteratorProjection, ordinary: TypeId) -> NativeFlowResult {
        let frame = self.native_flow_iterator_frame(provider, at, projection.frame);
        let projected = self.native_pack_slot(NativeResultPack { ty: frame.ty }, projection.index);
        let ty = self.map_type(ordinary, |checker, part| {
            if checker.are_comparable(part, projected) { part } else { TypeId::NEVER }
        });
        NativeFlowResult { ty, incomplete: frame.incomplete, unresolved_auto: false }
    }
}
