// @expected-stdout-begin
// outer
// from target
// @expected-stdout-end
// Symbol.unscopables on proxy target prevents with-scope resolution
var a = "outer";
var b = "outer b";
var target = { a: "from target", b: "from target" };
target[Symbol.unscopables] = { a: true };
var proxy = new Proxy(target, {});
with (proxy) {
    console.log(eval("a"));
    console.log(eval("b"));
}
