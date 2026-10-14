//! Type-only original Lua assertion operations. Source owns annotation spans;
//! exclusion, tuple structure and mapper identity remain native.
use super::Checker;
use super::native_packs::{NativePackSlot, NativePackAlternative};
use crate::types::{NativeResultPack, TypeId};

impl<'p, 's> Checker<'p, 's> {
    pub fn native_assertion_overlap(&mut self, source: TypeId, target: TypeId) -> bool {
        let source = self.widened(source);
        self.are_comparable(source, target)
    }
    pub fn native_exclude_type(&mut self, source: TypeId, excluded: TypeId) -> TypeId {
        if self.is_any(source) && !self.is_any(excluded) && excluded != TypeId::UNKNOWN { return source; }
        if excluded.is_undefined() { return self.non_nullable(source); }
        self.filter(source, |checker, part| !checker.is_assignable(part, excluded))
    }
    pub fn native_exclude_pack(&mut self, source: NativeResultPack, fixed: &[TypeId], tail: Option<TypeId>) -> NativeResultPack {
        let Some(rows) = self.native_pack_alternatives(source) else { return source; };
        let mut result = Vec::new();
        for mut row in rows {
            if row.tail.is_none() && fixed.iter().enumerate().skip(row.fixed.len()).any(|(_, &excluded)| self.is_assignable(TypeId::UNDEFINED, excluded)) { continue; }
            let original_tail = row.tail;
            for (index, slot) in row.fixed.iter_mut().enumerate() {
                if let Some(excluded) = fixed.get(index).copied().or(tail) {
                    slot.ty = self.native_exclude_type(slot.ty, excluded);
                    if index < fixed.len() && self.is_assignable(TypeId::UNDEFINED, excluded) { slot.optional = false; }
                }
            }
            if row.fixed.iter().any(|slot| !slot.optional && slot.ty.is_never()) { continue; }
            if let Some(original) = original_tail {
                while row.fixed.len() < fixed.len() {
                    let index = row.fixed.len();
                    row.fixed.push(NativePackSlot { ty: self.native_exclude_type(original, fixed[index]), optional: !self.is_assignable(TypeId::UNDEFINED, fixed[index]) });
                }
                if let Some(excluded) = tail { row.tail = Some(self.native_exclude_type(original, excluded)); }
            }
            if row.tail.is_some_and(TypeId::is_never) { row.tail = None; }
            if let Some(index) = row.fixed.iter().position(|slot| slot.optional && slot.ty.is_never()) {
                row.fixed.truncate(index);
                row.tail = None;
            }
            if row.fixed.iter().any(|slot| !slot.optional && slot.ty.is_never()) { continue; }
            result.push(NativePackAlternative { fixed: row.fixed, tail: row.tail });
        }
        self.native_result_pack(&result)
    }
}
