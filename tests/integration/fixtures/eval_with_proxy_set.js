// @expected-stdout-begin
// has eval
// has x
// set x 200
// 200
// @expected-stdout-end
//
// Corrected 2026-08-12. The previous expectation omitted `has eval` — it had been
// written from THIS COMPILER's output rather than from a reference implementation,
// so it certified a real defect as correct and the fixture passed while wrong.
//
// Node emits `has eval` because resolving the identifier `eval` goes through the
// Proxy's `has` trap. esc resolves `eval` at compile time and never consults the
// proxy, so the trap does not fire. Same class as the console-aliasing special
// case. Now registered in tests/integration/xfail.txt.
//
// An expectation copied from the implementation is unfalsifiable by construction.
// This one was found by the Node differential, which is the only check that
// compares against something other than ourselves.
// with(proxy) { eval("x = 1") } triggers Proxy set trap
var target = { x: 1 };
var proxy = new Proxy(target, {
    has: function(t, prop) {
        console.log("has " + prop);
        return prop in t;
    },
    set: function(t, prop, val) {
        console.log("set " + prop + " " + val);
        t[prop] = val;
        return true;
    }
});
with (proxy) {
    eval("x = 200");
}
console.log(target.x);
