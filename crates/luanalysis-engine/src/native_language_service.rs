//! Queries for the language server and inspections that checking never asks.
use super::{SessionQueries, Type, export_type, import_type};

impl SessionQueries<'_, '_, '_> {
    pub(super) fn type_members_native(&mut self, ty: &Type) -> Vec<(String, Type)> {
        let ty = import_type(self.checker, self.handles, ty);
        self.checker.native_completion_members(ty).into_iter().map(|(name, value)| {
            let name = String::from_utf8_lossy(self.checker.atoms().bytes(name)).into_owned();
            (name, export_type(self.checker, self.handles, value))
        }).collect()
    }
}

impl SessionQueries<'_, '_, '_> {
    pub(super) fn types_comparable_native(&mut self, left: &Type, right: &Type) -> bool {
        let left = import_type(self.checker, self.handles, left);
        let right = import_type(self.checker, self.handles, right);
        self.checker.are_comparable(left, right)
    }
}
