// @expected-stdout-begin
// 10
// revoked
// @expected-stdout-end
var target = { x: 10 };
var rv = Proxy.revocable(target, {});
var proxy = rv.proxy;
var revoke = rv.revoke;
// First access works
with (proxy) {
    console.log(x);
}
// Revoke and try again
revoke();
try {
    with (proxy) {
        var z = x;
    }
} catch (e) {
    console.log("revoked");
}
