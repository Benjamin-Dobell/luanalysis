-- Reduced examples inspired by ge_tts/Json.ttslua:84-104 and
-- prototype_inertia/src/model/CardState.ttslua:125-137.
-- Named shapes are expanded into inline records for the initial checker slice.

---@type {encodeArrayLength: nil | boolean, nullIdentification: nil | boolean}
local defaultDecodeOptions = {
    encodeArrayLength = false,
    nullIdentification = true,
}

defaultDecodeOptions.encodeArrayLength = true

---@type boolean | nil
local identifyNull = defaultDecodeOptions.nullIdentification

---@type {red: number, blue: number, indeterminate: number}
local activeEnergy = {red = 0, blue = 0, indeterminate = 0}

activeEnergy.blue = 2

---@type number
local blue = activeEnergy.blue
