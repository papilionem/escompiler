// @expected-stdout-begin
// 42
// revoked
// @expected-stdout-end
// with(revocableProxy), access works, then revoke, then eval throws
var target = { x: 42 };
var rev = Proxy.revocable(target, {});
with (rev.proxy) {
    console.log(eval("x"));
}
rev.revoke();
try {
    with (rev.proxy) {
        eval("x");
    }
} catch (e) {
    console.log("revoked");
}
