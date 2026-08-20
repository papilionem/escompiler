// @expected-stdout-begin
// get hello
// world
// @expected-stdout-end
// eval creates a Proxy
var target = { hello: "world" };
var p = eval("new Proxy(target, { get: function(t, prop) { console.log('get ' + prop); return t[prop]; } })");
console.log(p.hello);
