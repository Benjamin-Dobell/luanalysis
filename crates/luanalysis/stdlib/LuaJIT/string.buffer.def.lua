--- LuaJIT's string buffers, `require("string.buffer")`: growable byte buffers and the
--- serialization of Lua values.

---
--- The options of a buffer's serialization: `dict`, the strings that often occur as table keys,
--- and `metatable`, the metatables of the tables serialized. Encoder and decoder must use the
--- same options.
---@shape string.buffer.serialization.opts
---@field dict? string[]
---@field metatable? table[]

---
--- A string buffer. Its writers append to its end and its readers consume from its front.
--- Methods that return nothing else return the buffer, so calls chain. `#buffer` is its length
--- in bytes, and `..` and `tostring` read it as a string.
---@class string.buffer: userdata
---@operator len: integer
---@operator concat(string | number | string.buffer): string
local buf = {}

---
--- Empties the buffer, keeping its allocated space.
---@return string.buffer
function buf:reset() end

---
--- Frees the buffer's space. The buffer stays usable, empty.
---@return string.buffer
function buf:free() end

---
--- Appends each string, number or object with a `__tostring` metamethod.
---@vararg string | number | table | userdata
---@return string.buffer
function buf:put(...) end

---
--- Appends its arguments formatted, as `string.format` formats them.
---@param format string
---@vararg any
---@return string.buffer
function buf:putf(format, ...) end

---
--- Appends `len` bytes of the memory that C data points to.
---@param cdata ffi.cdata*
---@param len integer
---@return string.buffer
function buf:putcdata(cdata, len) end

---
--- Makes the buffer a reference to a string, or to `len` bytes of the memory that C data points
--- to, without copying it until the buffer is written.
---@overload fun(str: string | string.buffer): string.buffer
---@overload fun(cdata: ffi.cdata*, len: integer): string.buffer
---@param str string | string.buffer | ffi.cdata*
---@param len? integer
---@return string.buffer
function buf:set(str, len) end

---
--- Reserves at least `size` bytes to write, returning a `uint8_t *` to them and their length,
--- which may be more. `commit` then appends what was written.
---@param size integer
---@return ffi.cdata*, integer
function buf:reserve(size) end

---
--- Appends the first `used` bytes reserved by `reserve`.
---@param used integer
---@return string.buffer
function buf:commit(used) end

---
--- Consumes `len` bytes from the front of the buffer, or all of them if there are fewer.
---@param len integer
---@return string.buffer
function buf:skip(len) end

---
--- Consumes and returns the buffer's data: all of it, or up to each given length in turn as a
--- string each; `nil` is all that remains.
---@overload fun(): string
---@overload fun(...: integer | nil): string...
---@vararg integer | nil
---@return string...
function buf:get(...) end

---
--- The buffer's data as a string, without consuming it.
---@return string
function buf:tostring() end

---
--- A `uint8_t *` to the buffer's data, and its length in bytes.
---@return ffi.cdata*, integer
function buf:ref() end

---
--- Appends the serialization of `obj`.
---@param obj any
---@return string.buffer
function buf:encode(obj) end

---
--- Consumes and returns one serialized object from the front of the buffer.
---@return any
function buf:decode() end

---@class string.bufferlib
local buffer = {}

---
--- A new buffer, with room for at least `size` bytes, and its serialization options.
---@overload fun(size?: integer, options?: string.buffer.serialization.opts): string.buffer
---@overload fun(options: string.buffer.serialization.opts): string.buffer
---@param size? integer | string.buffer.serialization.opts
---@param options? string.buffer.serialization.opts
---@return string.buffer
function buffer.new(size, options) end

---
--- The serialization of `obj`, a string.
---@param obj any
---@return string
function buffer.encode(obj) end

---
--- The object `str` serializes.
---@param str string
---@return any
function buffer.decode(str) end

return buffer
