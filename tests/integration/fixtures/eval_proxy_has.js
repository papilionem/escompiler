// @expected-stdout-begin
// has x
// true
// has z
// false
// @expected-stdout-end
// eval uses `in` operator on Proxy, triggers has trap
var target = { x: 1 };
var proxy = new Proxy(target, {
    has: function(t, prop) {
        console.log("has " + prop);
        return prop in t;
    }
});
console.log(eval("'x' in proxy"));
console.log(eval("'z' in proxy"));
