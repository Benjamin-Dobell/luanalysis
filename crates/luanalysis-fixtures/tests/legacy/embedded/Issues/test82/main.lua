function test(arg) end

function issue82(...)
    local t = ...
    test(t)
end