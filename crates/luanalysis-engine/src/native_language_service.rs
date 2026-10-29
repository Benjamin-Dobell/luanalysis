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
    pub(super) fn named_object_member_origins_native(&mut self, ty: &Type, name: &str) -> Vec<crate::NativeTypeMemberKey> {
        let ty = import_type(self.checker, self.handles, ty);
        self.checker.native_object_member_origins(ty, name.as_bytes()).into_iter()
            .map(|member| crate::NativeTypeMemberKey { snapshot: member.snapshot, index: member.index })
            .collect()
    }

    pub(super) fn member_visibility_native(&mut self, ty: &Type, name: &str) -> Vec<(crate::TypeMemberVisibility, crate::NativeTypeDeclarationKey, crate::NativeTypeMemberKey)> {
        use bun_sema::check::native_named::NativeTypeMemberVisibility;
        let ty = import_type(self.checker, self.handles, ty);
        self.checker.native_member_visibility(ty, name.as_bytes()).into_iter().map(|(visibility, owner, member)| (
            match visibility {
                NativeTypeMemberVisibility::Public => crate::TypeMemberVisibility::Public,
                NativeTypeMemberVisibility::Protected => crate::TypeMemberVisibility::Protected,
                NativeTypeMemberVisibility::Private => crate::TypeMemberVisibility::Private,
                NativeTypeMemberVisibility::Package => crate::TypeMemberVisibility::Package,
            },
            crate::NativeTypeDeclarationKey { snapshot: owner.snapshot, index: owner.index },
            crate::NativeTypeMemberKey { snapshot: member.snapshot, index: member.index },
        )).collect()
    }

    pub(super) fn types_comparable_native(&mut self, left: &Type, right: &Type) -> bool {
        let left = import_type(self.checker, self.handles, left);
        let right = import_type(self.checker, self.handles, right);
        self.checker.are_comparable(left, right)
    }
}
