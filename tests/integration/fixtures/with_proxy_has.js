// @expected-stdout-begin
// has:x
// has:y
// true
// false
// @expected-stdout-end
var hasLog = [];
var target = { x: 10 };
var handler = {
    has: function(t, prop) {
        hasLog.push("has:" + prop);
        return prop in t;
    }
};
var proxy = new Proxy(target, handler);
var foundX = false;
var foundY = false;
with (proxy) {
    foundX = typeof x !== "undefined";
}
// y is not on target, so has trap returns false and y resolves from outer scope
var y;
with (proxy) {
    foundY = typeof y !== "undefined";
}
console.log(hasLog[0]);
console.log(hasLog[1]);
console.log(foundX);
console.log(foundY);
