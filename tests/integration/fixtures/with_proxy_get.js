// @expected-stdout-begin
// get:x
// 42
// @expected-stdout-end
var log = [];
var target = { x: 42 };
var handler = {
    get: function(t, prop) {
        log.push("get:" + prop);
        return t[prop];
    }
};
var proxy = new Proxy(target, handler);
with (proxy) {
    var val = x;
}
console.log(log[0]);
console.log(val);
