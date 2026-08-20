// @expected-stdout-begin
// has x
// get x
// 100
// @expected-stdout-end
// with(proxy) { eval("x") } triggers Proxy get trap for property lookup
var target = { x: 100 };
var proxy = new Proxy(target, {
    has: function(t, prop) {
        console.log("has " + prop);
        return prop in t;
    },
    get: function(t, prop) {
        console.log("get " + prop);
        return t[prop];
    }
});
with (proxy) {
    var result = eval("x");
    console.log(result);
}
