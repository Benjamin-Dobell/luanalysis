local a = 1
a = a <error descr="The binary operator '&' only available in Lua 5.3 and above">&</error> a
a = a <error descr="The binary operator '|' only available in Lua 5.3 and above">|</error> a
a = a <error descr="The binary operator '~' only available in Lua 5.3 and above">~</error> a
a = a <error descr="The binary operator '>>' only available in Lua 5.3 and above">>></error> a
a = a <error descr="The binary operator '<<' only available in Lua 5.3 and above"><<</error> a
a = <error descr="The unary operator '~' only available in Lua 5.3 and above">~</error>a