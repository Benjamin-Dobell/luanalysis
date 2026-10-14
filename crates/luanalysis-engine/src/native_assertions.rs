use super::native_packs::import_pack;
use super::{SessionQueries, Type, ValuePack, import_type};

impl SessionQueries<'_, '_, '_> {
    pub(super) fn assertion_overlap_native(&mut self, source: &Type, target: &Type) -> bool {
        let source = import_type(self.checker, self.handles, source);
        let target = import_type(self.checker, self.handles, target);
        self.checker.native_assertion_overlap(source, target)
    }
    pub(super) fn exclude_type_native(&mut self, source: &Type, excluded: &Type) -> Type {
        let source = import_type(self.checker, self.handles, source);
        let excluded = import_type(self.checker, self.handles, excluded);
        let ty = self.checker.native_exclude_type(source, excluded);
        self.handles.retain(ty)
    }
    pub(super) fn exclude_pack_native(
        &mut self,
        source: &ValuePack,
        fixed: &[Type],
        tail: Option<&Type>,
    ) -> ValuePack {
        let source = import_pack(self.checker, self.handles, source);
        let fixed: Vec<_> = fixed
            .iter()
            .map(|ty| import_type(self.checker, self.handles, ty))
            .collect();
        let tail = tail.map(|ty| import_type(self.checker, self.handles, ty));
        let result = self.checker.native_exclude_pack(source, &fixed, tail);
        self.handles.retain_pack(self.checker, result)
    }
}
