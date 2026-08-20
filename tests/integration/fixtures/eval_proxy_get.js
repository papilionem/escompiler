// @expected-stdout-begin
// get x
// 10
// @expected-stdout-end
// eval accesses property on Proxy, triggers get trap
var target = { x: 10 };
var proxy = new Proxy(target, {
    get: function(t, prop) {
        console.log("get " + prop);
        return t[prop];
    }
});
eval("console.log(proxy.x)");
