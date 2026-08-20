// @expected-stdout-begin
// set x 99
// 99
// @expected-stdout-end
// eval sets property on Proxy, triggers set trap
var target = { x: 1 };
var proxy = new Proxy(target, {
    set: function(t, prop, val) {
        console.log("set " + prop + " " + val);
        t[prop] = val;
        return true;
    }
});
eval("proxy.x = 99");
console.log(target.x);
