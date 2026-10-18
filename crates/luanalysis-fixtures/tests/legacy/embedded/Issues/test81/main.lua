local function file_contains(filename, text)
  local f = assert(io.open(filename, "r"))
  local found = f:read("a"):find(text, 1, true)
  f:close()
  return found
end