//! A result can be usable in this inference context without being cacheable.
use super::Checker;

#[derive(Clone, Copy)]
pub(super) struct NativeQueryContext {
    serial: u64,
    inherited_taint: bool,
}

impl Checker<'_, '_> {
    pub(super) fn native_query_context(&self) -> NativeQueryContext {
        let frame = self.frames.last().expect("entered native query");
        NativeQueryContext {
            serial: frame.serial,
            inherited_taint: frame.tainted && !frame.circular && !frame.incomplete_flow,
        }
    }

    /// Consume only this finished query's sidecar. The caller gets no Stored
    /// permission, and every enclosing frame and inference scope stays intact.
    pub(super) fn native_complete_without_cache(&mut self, context: NativeQueryContext, memoizable: bool) -> bool {
        if !context.inherited_taint || !memoizable
            || self.lowest_taint_since(context.serial) != usize::MAX
            || !self.left_frame.as_ref().is_some_and(|frame| {
                frame.serial == context.serial && !frame.circular && !frame.incomplete_flow
            })
        { return false }
        self.left_frame.take();
        true
    }
}
