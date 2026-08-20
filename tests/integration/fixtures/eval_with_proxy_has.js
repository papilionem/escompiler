// @expected-stdout-begin
// has y
// not found
// @expected-stdout-end
// with(proxy) scope lookup triggers Proxy has trap to check if var exists
var target = { x: 1 };
var y = "not found";
var proxy = new Proxy(target, {
    has: function(t, prop) {
        console.log("has " + prop);
        return prop in t;
    }
});
with (proxy) {
    console.log(eval("y"));
}
