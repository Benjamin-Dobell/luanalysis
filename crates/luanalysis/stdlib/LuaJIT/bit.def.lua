--- LuaJIT's bit operations library, the global `bit` (also `require("bit")`). Each operation
--- takes its arguments as 32-bit integers and returns a signed 32-bit integer.
---@class bitlib
bit = {}

---
--- Normalizes a number to the numeric range of the bit operations.
---@param x number
---@return integer
function bit.tobit(x) end

---
--- Converts `x` to a hex string of `n` digits, 8 by default; a negative `n` uses upper-case
--- digits.
---@param x number
---@param n? integer
---@return string
function bit.tohex(x, n) end

---
--- The bitwise not of `x`.
---@param x number
---@return integer
function bit.bnot(x) end

---
--- The bitwise or of all its arguments.
---@param x number
---@vararg number
---@return integer
function bit.bor(x, ...) end

---
--- The bitwise and of all its arguments.
---@param x number
---@vararg number
---@return integer
function bit.band(x, ...) end

---
--- The bitwise xor of all its arguments.
---@param x number
---@vararg number
---@return integer
function bit.bxor(x, ...) end

---
--- `x` shifted left by `n` bits.
---@param x number
---@param n number
---@return integer
function bit.lshift(x, n) end

---
--- `x` shifted right by `n` bits, filling with zeros.
---@param x number
---@param n number
---@return integer
function bit.rshift(x, n) end

---
--- `x` shifted right by `n` bits, filling with its sign bit.
---@param x number
---@param n number
---@return integer
function bit.arshift(x, n) end

---
--- `x` rotated left by `n` bits.
---@param x number
---@param n number
---@return integer
function bit.rol(x, n) end

---
--- `x` rotated right by `n` bits.
---@param x number
---@param n number
---@return integer
function bit.ror(x, n) end

---
--- `x` with its bytes swapped, converting between little-endian and big-endian.
---@param x number
---@return integer
function bit.bswap(x) end

return bit
