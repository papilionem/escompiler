function g(o) { with (o) { return x; } }
console.log(g({ x: 42 }));
