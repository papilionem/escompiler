// @expected-stdout-begin
// set:x:99
// 99
// @expected-stdout-end
var log = [];
var target = { x: 1 };
var handler = {
    set: function(t, prop, val) {
        log.push("set:" + prop + ":" + val);
        t[prop] = val;
        return true;
    },
    has: function(t, prop) {
        return prop in t;
    }
};
var proxy = new Proxy(target, handler);
with (proxy) {
    x = 99;
}
console.log(log[0]);
console.log(target.x);
