async function b() { return 5; }
var p = b();
console.log(typeof p, p instanceof Promise);
