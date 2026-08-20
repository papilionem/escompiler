// @expected-stdout-begin
// has x
// get x
// 42
// @expected-stdout-end
// eval inside with(proxy) - triple combo of eval + proxy + with
var target = { x: 42 };
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
    console.log(eval("x"));
}
