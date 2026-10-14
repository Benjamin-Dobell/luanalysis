-- Deliberately mutated version of the ge_tts JSON options record idiom.
-- The string initializer is an intentional type error in this fixture.
-- This is not evidence of a defect in the original ge_tts source.

---@type {encodeArrayLength: nil | boolean, nullIdentification: nil | boolean}
local defaultDecodeOptions = {
    encodeArrayLength = "yes",
    nullIdentification = true,
}
