// @expected-stdout-begin
// 1
// 2
// @expected-stdout-end
let target = { x: 1, y: 2 };
let proxy = new Proxy(target, {});
console.log(proxy.x);
console.log(proxy.y);
