-- Omitted to prevent cyclic require:
-- require('ge.tts/License')

-- This module operates on tables that contain only positive consecutive integer keys starting at 1 (i.e. a plain array), as well as tables which contain no
-- array component. Behavior is undefined for tables that contain a key for [1] as well as non-consecutive integer or non-integer keys.

---@alias __ge_tts__TableUtils_IteratorKey number | string | boolean | table | function | thread | userdata
---@alias __ge_tts__TableUtils_IteratorTable table<__ge_tts__TableUtils_IteratorKey, unknown>
---@alias __ge_tts__TableUtils_PairsIterator fun(tab: __ge_tts__TableUtils_IteratorTable, k: nil | __ge_tts__TableUtils_IteratorKey): nil | (__ge_tts__TableUtils_IteratorKey, unknown)
---@alias __ge_tts__TableUtils_PackedIterator fun(tab: std__Packed<unknown>, i: number): nil | (number, unknown)
---@alias __ge_tts__TableUtils_ArrayIterator fun(tab: unknown[], i: number): nil | (number, unknown)
---@alias __ge_tts__TableUtils_ArrayPairsIterator fun(tab: unknown[], k: nil | number): nil | (number, unknown)

---@generic T
---@param length number
---@return fun(arr: std__Packed<T>, i: number): nil | (number, T | nil)
local function fixedLengthIterator(length)
    ---@type fun(arr: std__Packed<T>, i: number): nil | (number, T | nil)
    return function(arr, i)
        i = i + 1
        if i <= length then
            return i, arr[i]
        end
    end
end

-- Numeric n selects the fixed-length protocol, even on a dictionary. Negative
-- n can instead select ipairs or pairs. Keep each iterator tied to its actual
-- state and initial control; table<K, V> alone cannot distinguish these cases.
---@overload fun<V>(arr: std__Packed<V>): ((fun(arr: std__Packed<V>, i: number): nil | (number, V | nil)), std__Packed<V>, 0) | ((fun(arr: V[], i: number): nil | (number, V)), V[], 0) | (__ge_tts__TableUtils_PairsIterator, __ge_tts__TableUtils_IteratorTable, nil)
---@overload fun<V>(arr: V[]): ((fun(arr: V[], i: number): nil | (number, V)), V[], 0) | ((fun(arr: V[], k: nil | number): nil | (number, V)), V[], nil)
---@overload fun<K, V>(tab: table<K, V>): ((fun(tab: table<K, V>, k: nil | K): nil | (K, V)), table<K, V>, nil) | (__ge_tts__TableUtils_PackedIterator, std__Packed<unknown>, 0) | (__ge_tts__TableUtils_ArrayIterator, unknown[], 0)
---@param tab __ge_tts__TableUtils_IteratorTable
---@return (__ge_tts__TableUtils_PairsIterator, __ge_tts__TableUtils_IteratorTable, nil) | (__ge_tts__TableUtils_PackedIterator, std__Packed<unknown>, 0) | (__ge_tts__TableUtils_ArrayIterator, unknown[], 0) | (__ge_tts__TableUtils_ArrayPairsIterator, unknown[], nil)
local function iterate(tab)
    local fixedLength = (tab).n

    if type(fixedLength) == 'number' and fixedLength >= 0 then
        return fixedLengthIterator(fixedLength), --[[---@type std__Packed<unknown>]] tab, 0
    elseif tab[1] ~= nil then
        return ipairs(--[[---@type unknown[] ]] tab)
    else
        return pairs(tab)
    end
end

---@class ge_tts__TableUtils
local TableUtils = {}

--- Returns true if TableUtils will interpret the table as an array i.e. if tab[1] ~= nil or
--- type(tab.n) == 'number'.
---
--- If tab is an array, and it's passed to a TableUtils function that iterates over tab calling a callback, the
--- iteration over keys is guaranteed to take place in sequential order (à la ipairs).
---
--- In the case of type(tab.n) == 'number', tab.n will be treated as the length of the array and TableUtils will
--- continue iterating over "holes" (nil values) up to this length.
---@overload fun<V>(tab: V[]): true
---@overload fun<V>(tab: std__Packed<V>): true
---@overload fun<V>(tab: table<boolean, V>): false
---@overload fun<V>(tab: table<string, V>): false
---@overload fun<V>(tab: table<table, V>): false
---@overload fun<V>(tab: table<userdata, V>): false
---@param tab table
---@return boolean
function TableUtils.isArray(tab)
    return tab[1] ~= nil or type((--[[---@type std__Packed<any>]] tab).n) == 'number'
end

--- Returns the length of arr and a boolean indicating whether arr is a std__Packed<V>.
---@generic V
---@param arr V[] | std__Packed<V>
---@return number, boolean
function TableUtils.arrayLength(arr)
    local fixedLength = (--[[---@type std__Packed<V>]] arr).n
    local isFixed = type(fixedLength) == 'number'
    return isFixed and fixedLength or #arr, isFixed
end

---@overload fun<V>(arr: V[],  predicate: (fun(value: V, index: number): boolean), ...: V[]): void
---@overload fun<V, PV : V, CovariantA : PV>(arr: V[], predicate: (fun(value: PV, index: number): boolean), a: CovariantA[]): void
---@overload fun<V, PV : V, CovariantA : PV, CovariantB : PV>(arr: V[], predicate: (fun(value: PV, index: number): boolean), a: CovariantA[], b: CovariantB[]): void
---@overload fun<V, PV : V, CovariantA : PV, CovariantB : PV, CovariantC : PV>(arr: V[], predicate: (fun(value: PV, index: number): boolean), a: CovariantA[], b: CovariantB[], c: CovariantC[]): void
---@overload fun<V, PV : V, CovariantA : PV, CovariantB : PV, CovariantC : PV, CovariantD : PV>(arr: V[], predicate: (fun(value: PV, index: number): boolean), a: CovariantA[], b: CovariantB[], c: CovariantC[], d: CovariantD[]): void
---@overload fun<V, PV : V, Covariant : PV>(arr: V[], predicate: (fun(value: PV, index: number): boolean), ...: Covariant[]): void
---@overload fun<V>(arr: std__Packed<V>, predicate: (fun(value: V, index: number): boolean), ...: V[] | std__Packed<V>): void
---@generic K, V
---@param tab table<K, V>
---@param predicate fun(value: V, key: K): boolean
---@vararg table<K, V>
---@return void
function TableUtils.selectInject(tab, predicate, ...)
    local otherTables = { ... }

    ---@type nil | boolean
    local isArray = nil

    ---@type number
    local arrayLength = 0

    ---@type boolean
    local isPacked = false

    if next(tab) then
        isArray = TableUtils.isArray(tab)

        if isArray then
            arrayLength, isPacked = TableUtils.arrayLength(--[[---@type V[] | std__Packed<V>]] tab)
        end
    end

    for _, otherTable in ipairs(otherTables) do
        if isArray == nil and next(otherTable) then
            isArray = TableUtils.isArray(otherTable)

            if (--[[---@type std__Packed<V>]] otherTable).n then
                isPacked = true
            end
        end

        if isArray then
            local arr = --[[---@type V[] | std__Packed<V>]] tab

            for i, v in iterate(--[[---@type V[] ]] otherTable) do
                if predicate(v, --[[---@type K]] i) then
                    arrayLength = arrayLength + 1
                    arr[arrayLength] = v
                end
            end
        else
            for k, v in pairs(otherTable) do
                if predicate(v, k) then
                    tab[k] = v
                end
            end
        end
    end

    if isPacked then
        (--[[---@type std__Packed<V>]] tab).n = arrayLength
    end
end

return TableUtils
