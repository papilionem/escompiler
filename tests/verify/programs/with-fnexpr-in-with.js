var o = { a: 1 };
with (o) { var f = function () { return a; }; }
console.log(f());
