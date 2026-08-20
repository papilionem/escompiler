var o = { a: 1 };
function outer() { with (o) { return a; } }
console.log(outer());
