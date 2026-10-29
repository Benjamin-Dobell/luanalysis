-- TypeScript's utility types, as EmmyLua Analyzer also declares them.

---
--- `T` with every property optional.
---@alias Partial<T> { [P in keyof T]?: T[P] }

---
--- `T` with every property required.
---@alias Required<T> { [P in keyof T]-?: T[P] }

---
--- `T` with every property read-only.
---@alias Readonly<T> { readonly [P in keyof T]: T[P] }

---
--- The properties of `T` that `K` names.
---@alias Pick<T, K: keyof T> { [P in K]: T[P] }

---
--- The properties of `T` other than those `K` names.
---@alias Omit<T, K> Pick<T, Exclude<keyof T, K>>

---
--- A table with a property of type `T` for each key in `K`.
---@alias Record<K, T> { [P in K]: T }

---
--- The members of `T` that are not assignable to `U`.
---@alias Exclude<T, U> T extends U and never or T

---
--- The members of `T` that are assignable to `U`.
---@alias Extract<T, U> T extends U and T or never

---
--- `T` without `nil`.
---@alias NonNullable<T> T extends nil and never or T

---
--- What the function type `T` returns.
---@alias ReturnType<T: function> T extends fun(...): infer R and R or any

---
--- The parameters of the function type `T`, as a tuple.
---@alias Parameters<T: function> T extends fun(...: infer P...) and P or never
