// @expected-stdout-begin
// 42
// revoked
// @expected-stdout-end
let target = { x: 42 };
let { proxy, revoke } = Proxy.revocable(target, {});
console.log(proxy.x);
revoke();
try { console.log(proxy.x); } catch(e) { console.log("revoked"); }
